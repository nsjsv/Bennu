use std::path::Path;
use std::sync::Arc;

use super::super::task_driving_tests::drive;
use super::super::FileBrowser;
use super::super::SuperBoxMessage;
use crate::actions::type_filter::TypeFilter;
use crate::actions::{
    ActionDefinition, ActionId, ActionLibrary, ActionNotice, ActionStep, PathTemplate,
};
use crate::model::{BrowserLocation, Message, NavigationMode, SettingsSubpage};
use crate::shortcuts::ShortcutAction;
use crate::super_box::BoxRow;

/// 列出这个文件夹（通配看列出来的条目），窗口在前台（普通完成通知不弹）。复制、移动要有任务的
/// 状态库才肯开始，放在 `state` 里（不放进列出来的文件夹）。
async fn browser_listing(directory: &Path, state: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    file_operations::test_persistence::attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(state.join("state.sqlite")).unwrap(),
    );
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    let load = browser.navigate_to(directory.to_path_buf(), NavigationMode::RecordHistory);
    drive(&mut browser, load).await;
    browser
}

async fn type_in_box(browser: &mut FileBrowser, text: &str) {
    if browser.super_box.is_none() {
        drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    }
    let typed = browser.update(Message::SuperBox(SuperBoxMessage::InputChanged(
        text.to_owned(),
    )));
    drive(browser, typed).await;
}

async fn press(
    browser: &mut FileBrowser,
    key: iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) {
    let pressed = browser
        .super_box_key(&key, modifiers)
        .expect("the box takes the key");
    drive(browser, pressed).await;
}

fn enter() -> iced::keyboard::Key {
    iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
}

fn tidy_action(folder: &str) -> Arc<ActionDefinition> {
    Arc::new(ActionDefinition {
        id: ActionId::from_file_stem("tidy"),
        name: "整理".to_owned(),
        file: "/actions/tidy.toml".into(),
        types: TypeFilter::Anything,
        steps: vec![ActionStep::MoveTo(PathTemplate::parse(folder).unwrap())],
        notice: ActionNotice::Silent,
        watch: None,
    })
}

#[tokio::test]
async fn a_name_pattern_picks_files_here_and_the_steps_run_and_undo_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for name in ["a.txt", "b.txt", "c.md", ".hidden.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let mut browser = browser_listing(&root, state.path()).await;

    type_in_box(&mut browser, "*.txt | 复制到 {dir}/备份").await;
    let super_box = browser.super_box.as_ref().unwrap();
    assert_eq!(super_box.rows(), [BoxRow::Pipe]);
    assert_eq!(
        super_box.picked().map(<[_]>::len),
        Some(2),
        "* skips dot files"
    );
    press(&mut browser, enter(), iced::keyboard::Modifiers::empty()).await;

    assert!(browser.super_box.is_none());
    assert_eq!(std::fs::read(root.join("备份/a.txt")).unwrap(), b"a.txt");
    assert!(root.join("备份/b.txt").exists() && !root.join("备份/c.md").exists());
    assert_eq!(
        browser.operation_history.undo_depth(),
        1,
        "one entry for the pipe"
    );
    assert_eq!(
        browser.user_config.super_box_history.entries()[0],
        "*.txt | 复制到 {dir}/备份"
    );
}

#[tokio::test]
async fn an_action_by_name_runs_on_the_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut browser = browser_listing(&root, state.path()).await;
    browser.action_library =
        ActionLibrary::from_definitions(vec![(*tidy_action("{dir}/整理好")).clone()]);
    browser.select_path(root.join("a.txt"));

    type_in_box(&mut browser, "| 整理").await;
    press(&mut browser, enter(), iced::keyboard::Modifiers::empty()).await;

    assert!(root.join("整理好/a.txt").exists());
    assert!(root.join("b.txt").exists(), "only the selection");
}

#[tokio::test]
async fn ctrl_s_drafts_an_action_with_the_steps_in_the_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut browser = browser_listing(&root, state.path()).await;
    browser.action_library =
        ActionLibrary::from_definitions(vec![(*tidy_action("{dir}/整理好")).clone()]);

    type_in_box(&mut browser, "*.txt | 压缩 zip | 整理").await;
    press(
        &mut browser,
        iced::keyboard::Key::Character("s".into()),
        iced::keyboard::Modifiers::CTRL,
    )
    .await;

    assert!(browser.super_box.is_none());
    let Some(SettingsSubpage::ActionEditor(editor)) = &browser.settings_subpage else {
        panic!("the action editor is open");
    };
    assert_eq!(editor.draft.name, "Compress to zip → 整理");
    assert_eq!(
        editor.draft.steps.len(),
        2,
        "the action's steps are copied in"
    );
    assert!(!root.join("整理好").exists(), "nothing ran");
}
