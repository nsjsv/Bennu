use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, InvalidEntryName};

use super::*;
use crate::config;
use crate::operation_queue::test_persistence::queued_task_id;

fn file_entry(path: &PathBuf) -> DirectoryEntry {
    DirectoryEntry::new(
        path.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn browser_showing(paths: &[&PathBuf]) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.entries = paths
        .iter()
        .map(|path| file_entry(path))
        .collect::<Vec<_>>()
        .into();
    browser
}

fn queue_rename(browser: &mut FileBrowser, path: &PathBuf, new_name: &str) -> u64 {
    let rename = QueuedFileOperation::Rename {
        path: path.clone(),
        new_name: new_name.into(),
    };
    queued_task_id(browser.operation_queue.enqueue(rename))
}

fn rejected_rename(path: &PathBuf, attempted_name: &str) -> FileOperationCompletion {
    FileOperationCompletion::RenameRejected {
        path: path.clone(),
        attempted_name: attempted_name.into(),
        reason: RenameRejection::NameTaken,
    }
}

#[test]
fn rename_input_naming_a_loaded_sibling_keeps_editing_without_queueing() {
    let report = PathBuf::from("/workspace/report.txt");
    let notes = PathBuf::from("/workspace/notes.txt");
    let mut browser = browser_showing(&[&report, &notes]);

    drop(browser.begin_rename(report.clone()));
    drop(browser.apply_rename_input_change(String::from("notes.txt")));
    assert_eq!(browser.rename_hint(), Some(RenameRejection::NameTaken));

    drop(browser.commit_rename());

    assert_eq!(browser.renaming, Some(report));
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn rename_input_with_invalid_name_keeps_editing_without_queueing() {
    let report = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_showing(&[&report]);
    drop(browser.begin_rename(report.clone()));

    for (input, reason) in [
        ("archive/report.txt", InvalidEntryName::ContainsSlash),
        ("..", InvalidEntryName::ParentDirectory),
        (".", InvalidEntryName::CurrentDirectory),
    ] {
        drop(browser.apply_rename_input_change(String::from(input)));
        assert_eq!(
            browser.rename_hint(),
            Some(RenameRejection::InvalidName(reason))
        );
        drop(browser.commit_rename());
        assert_eq!(browser.renaming, Some(report.clone()));
    }
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn rename_input_with_unused_name_has_no_hint() {
    let report = PathBuf::from("/workspace/report.txt");
    let notes = PathBuf::from("/workspace/notes.txt");
    let mut browser = browser_showing(&[&report, &notes]);
    drop(browser.begin_rename(report));

    drop(browser.apply_rename_input_change(String::from("summary.txt")));

    assert_eq!(browser.rename_hint(), None);
}

#[test]
fn rejected_user_rename_reopens_editor_with_attempted_name_and_reason() {
    let report = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_showing(&[&report]);
    let task_id = queue_rename(&mut browser, &report, "notes.txt");

    drop(browser.accept_file_operation_finished(task_id, rejected_rename(&report, "notes.txt")));

    assert_eq!(browser.renaming, Some(report.clone()));
    assert_eq!(browser.selected, Some(report));
    assert_eq!(browser.rename_input, "notes.txt");
    assert_eq!(browser.rename_hint(), Some(RenameRejection::NameTaken));
    assert_eq!(browser.current_error(), None);
    assert!(browser.operation_history.take_undo_operation().is_none());

    drop(browser.apply_rename_input_change(String::from("summary.txt")));
    assert_eq!(browser.rename_hint(), None);
}

/// 文件操作错误的归属位置是任务行，不复制到全局错误条。
fn assert_rejection_only_on_task_row(browser: &FileBrowser, task_id: u64) {
    let task = browser
        .operation_queue
        .tasks()
        .iter()
        .find(|task| task.id == task_id)
        .expect("finished task stays in the queue");
    let message = RenameRejection::NameTaken.message(crate::localization::current_language());
    assert_eq!(task.error.as_deref(), Some(message.as_str()));
    assert_eq!(browser.current_error(), None);
}

#[test]
fn rejected_rename_of_entry_no_longer_shown_leaves_reason_on_task_row() {
    let report = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_showing(&[]);
    let task_id = queue_rename(&mut browser, &report, "notes.txt");

    drop(browser.accept_file_operation_finished(task_id, rejected_rename(&report, "notes.txt")));

    assert_eq!(browser.renaming, None);
    assert_rejection_only_on_task_row(&browser, task_id);
}

#[test]
fn rejected_undo_rename_leaves_reason_on_task_row_without_opening_editor() {
    let report = PathBuf::from("/workspace/report.txt");
    let notes = PathBuf::from("/workspace/notes.txt");
    let mut browser = browser_showing(&[&notes]);
    let task_id = queue_rename(&mut browser, &report, "notes.txt");
    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
            from: report,
            to: notes.clone(),
        }),
    ));

    drop(browser.undo_file_operation());
    let undo_task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued undo")
        .id;
    let undo_completion = rejected_rename(&notes, "report.txt");
    drop(browser.accept_file_operation_finished(undo_task_id, undo_completion));

    assert_eq!(browser.renaming, None);
    assert_rejection_only_on_task_row(&browser, undo_task_id);
}

#[test]
fn new_rename_session_drops_previous_rejection() {
    let report = PathBuf::from("/workspace/report.txt");
    let notes = PathBuf::from("/workspace/notes.txt");
    let mut browser = browser_showing(&[&report, &notes]);
    let task_id = queue_rename(&mut browser, &report, "taken.txt");
    drop(browser.accept_file_operation_finished(task_id, rejected_rename(&report, "taken.txt")));
    // 按 Esc 等离开编辑的入口只清 `renaming`。
    browser.renaming = None;

    drop(browser.begin_rename(notes));

    assert_eq!(browser.rename_hint(), None);
}
