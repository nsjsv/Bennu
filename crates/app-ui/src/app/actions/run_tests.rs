use std::path::Path;

use super::*;
use crate::actions::{ActionNotice, PathTemplate};
use crate::app::task_driving_tests::drive;
use crate::config;
use crate::model::BrowserLocation;

fn action(steps: Vec<ActionStep>) -> RunPlan {
    RunPlan {
        name: "测试动作".to_owned(),
        steps,
        notice: ActionNotice::Silent,
        then: None,
        rule: None,
    }
}

fn move_to(template: &str) -> ActionStep {
    ActionStep::MoveTo(PathTemplate::parse(template).unwrap())
}

/// 窗口在前台：普通操作的完成通知不弹，测试不发真的系统通知。
fn browser_in(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    browser
}

#[tokio::test]
async fn steps_chain_and_the_whole_action_is_undone_and_redone_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let note = root.join("note.txt");
    std::fs::write(&note, b"hello").unwrap();
    let mut browser = browser_in(&root);

    let start = browser.start_action_run(
        action(vec![move_to("{dir}/a"), move_to("{dir}/b")]),
        vec![note.clone()],
    );
    drive(&mut browser, start).await;

    let moved = root.join("a/b/note.txt");
    assert_eq!(std::fs::read(&moved).unwrap(), b"hello");
    assert!(browser.action_runs.runs.is_empty(), "the run finished");
    assert_eq!(
        browser.operation_history.undo_depth(),
        1,
        "one entry for the action"
    );
    assert!(
        browser.tab().selected_paths.is_empty(),
        "results are not in this folder"
    );

    let undo = browser.undo_file_operation();
    drive(&mut browser, undo).await;
    assert_eq!(std::fs::read(&note).unwrap(), b"hello");
    assert!(!root.join("a").exists(), "both new folders are gone");
    assert_eq!(browser.operation_history.undo_depth(), 0);

    let redo = browser.redo_file_operation();
    drive(&mut browser, redo).await;
    assert_eq!(std::fs::read(&moved).unwrap(), b"hello");
    assert!(!note.exists());
}

#[tokio::test]
async fn a_failing_step_stops_the_rest_and_keeps_what_was_done() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let note = root.join("note.txt");
    std::fs::write(&note, b"hello").unwrap();
    // 和目标文件夹同名的是个文件：第二步没法做。
    std::fs::create_dir(root.join("a")).unwrap();
    std::fs::write(root.join("a/blocked"), b"file").unwrap();
    let mut browser = browser_in(&root);
    file_operations::test_persistence::attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(root.join("state.sqlite")).unwrap(),
    );

    let start = browser.start_action_run(
        action(vec![
            move_to("{dir}/a"),
            move_to("{dir}/blocked"),
            move_to("{dir}/never"),
        ]),
        vec![note.clone()],
    );
    drive(&mut browser, start).await;

    assert_eq!(std::fs::read(root.join("a/note.txt")).unwrap(), b"hello");
    assert!(!root.join("a/never").exists());
    assert!(browser.action_runs.runs.is_empty());
    // 做成的第一步照样能撤销。
    assert_eq!(browser.operation_history.undo_depth(), 1);
}

#[tokio::test]
async fn moving_into_an_existing_folder_never_overwrites_and_selects_results_here() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let inbox = root.join("inbox");
    std::fs::create_dir(&inbox).unwrap();
    std::fs::write(inbox.join("note.txt"), b"mine").unwrap();
    let note = inbox.join("incoming").join("note.txt");
    std::fs::create_dir(inbox.join("incoming")).unwrap();
    std::fs::write(&note, b"new").unwrap();
    let mut browser = browser_in(&inbox);
    file_operations::test_persistence::attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(root.join("state.sqlite")).unwrap(),
    );

    let start =
        browser.start_action_run(action(vec![move_to(inbox.to_str().unwrap())]), vec![note]);
    drive(&mut browser, start).await;

    assert_eq!(std::fs::read(inbox.join("note.txt")).unwrap(), b"mine");
    assert_eq!(std::fs::read(inbox.join("note 2.txt")).unwrap(), b"new");
    assert!(browser
        .tab()
        .selected_paths
        .contains(&inbox.join("note 2.txt")));
}

#[tokio::test]
async fn copying_into_a_new_folder_copies_into_the_folder_actually_made() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let note = root.join("note.txt");
    std::fs::write(&note, b"hello").unwrap();
    let mut browser = browser_in(&root);
    file_operations::test_persistence::attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(root.join("state.sqlite")).unwrap(),
    );

    let start = browser.start_action_run(
        action(vec![ActionStep::CopyTo(
            PathTemplate::parse("{dir}/backup/{date}").unwrap(),
        )]),
        vec![note.clone()],
    );
    drive(&mut browser, start).await;

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(
        std::fs::read(root.join("backup").join(date).join("note.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(std::fs::read(&note).unwrap(), b"hello");
    assert_eq!(browser.operation_history.undo_depth(), 1);
}

#[test]
fn action_tasks_are_labeled_in_the_queue_panel() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    drop(browser.start_action_run(
        action(vec![ActionStep::Trash(crate::actions::TrashScope::Current)]),
        vec![directory.path().join("a")],
    ));
    drop(browser.accept_action_step(
        0,
        PreparedStep::Ready(StepPlan {
            work: vec![super::super::steps::StepWork {
                operation: file_operations::QueuedFileOperation::Trash {
                    paths: vec![directory.path().join("a")],
                },
                then: None,
            }],
            passed_through: Vec::new(),
            skipped: Vec::new(),
        }),
    ));
    let task_id = browser.operation_queue.tasks()[0].id;

    assert!(browser.action_runs.owns(task_id));
    let label = browser.action_runs.label_for(task_id).unwrap();
    assert!(label.starts_with("测试动作 · "), "{label}");
    assert!(label.contains("1/1"), "{label}");
}
