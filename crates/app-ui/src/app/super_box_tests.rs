use std::path::Path;

use super::*;
use crate::actions::type_filter::TypeFilter;
use crate::actions::{
    ActionDefinition, ActionId, ActionLibrary, ActionNotice, ActionStep, PathTemplate,
};
use crate::app::task_driving_tests::drive;
use crate::config;
use crate::model::BrowserLocation;
use crate::super_box::BoxMode;

/// 窗口在前台：普通操作的完成通知不弹。
fn browser_in(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    browser
}

async fn send(browser: &mut FileBrowser, message: SuperBoxMessage) {
    let task = browser.update(Message::SuperBox(message));
    drive(browser, task).await;
}

async fn type_and_run(browser: &mut FileBrowser, text: &str) {
    if browser.super_box.is_none() {
        drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    }
    send(browser, SuperBoxMessage::InputChanged(text.to_owned())).await;
    send(browser, SuperBoxMessage::Submitted).await;
}

#[test]
fn the_shortcut_opens_and_closes_the_box_in_the_default_mode() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    browser.user_config.super_box_mode = BoxMode::Path;
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::Path)
    );
    assert!(
        browser.main_window_dialog_is_open(),
        "file-area keys are off"
    );

    // 框开着时 Tab 换模式，打开它的键关掉它。
    let tab = iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab);
    drop(browser.super_box_key(&tab, iced::keyboard::Modifiers::empty()));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::New)
    );
    drop(browser.super_box_key(&tab, iced::keyboard::Modifiers::SHIFT));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::Path)
    );
    let space = iced::keyboard::Key::Named(iced::keyboard::key::Named::Space);
    drop(browser.super_box_key(&space, iced::keyboard::Modifiers::ALT));
    assert!(browser.super_box.is_none());

    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    drop(browser.update(Message::DismissFloating));
    assert!(browser.super_box.is_none());
}

#[tokio::test]
async fn a_feature_runs_like_its_shortcut_and_is_remembered() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    let visible = browser.sidebar_visible();

    type_and_run(&mut browser, "toggle sidebar").await;

    assert!(browser.super_box.is_none());
    assert_ne!(browser.sidebar_visible(), visible);
    assert_eq!(
        browser.user_config.super_box_history.entries(),
        ["toggle sidebar"]
    );
}

#[tokio::test]
async fn an_action_runs_on_the_current_folder() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let mut browser = browser_in(&work);
    browser.action_library = ActionLibrary::from_definitions(vec![ActionDefinition {
        id: ActionId::from_file_stem("tidy"),
        name: "收进归档".to_owned(),
        file: root.join("tidy.toml"),
        types: TypeFilter::Anything,
        steps: vec![ActionStep::MoveTo(
            PathTemplate::parse("{dir}/归档").unwrap(),
        )],
        notice: ActionNotice::Silent,
    }]);

    type_and_run(&mut browser, "sjgd").await;

    assert!(
        root.join("归档/work").is_dir(),
        "the folder was moved by the action"
    );
}

#[tokio::test]
async fn a_settings_page_opens_the_settings_window_there() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    type_and_run(&mut browser, "shortcuts").await;
    assert!(browser.settings_window.is_some());
    assert_eq!(
        browser.selected_settings_category,
        SettingsCategory::Shortcuts
    );
}

#[tokio::test]
async fn going_to_a_folder_opens_it_and_going_to_a_file_selects_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("资料")).unwrap();
    std::fs::write(root.join("资料/报告.pdf"), b"%PDF").unwrap();
    let mut browser = browser_in(&root);

    type_and_run(&mut browser, "g:资料").await;
    assert!(browser.super_box.is_none());
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(root.join("资料"))
    );
    assert_eq!(browser.user_config.super_box_history.entries(), ["g:资料"]);

    type_and_run(&mut browser, &format!("{}/资料/报告.pdf", root.display())).await;
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(root.join("资料"))
    );
    assert!(browser.is_path_selected(&root.join("资料/报告.pdf")));
}

#[tokio::test]
async fn a_path_that_is_not_there_keeps_the_box_open_and_says_so() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);
    type_and_run(&mut browser, "g:没有这个").await;
    let super_box = browser.super_box.as_ref().expect("the box stays open");
    assert!(super_box.notice().is_some());
    assert_eq!(browser.tab().location, BrowserLocation::Directory(root));
    assert!(browser.user_config.super_box_history.entries().is_empty());
}

#[tokio::test]
async fn new_items_are_made_with_their_names_selected_and_undoable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);

    type_and_run(&mut browser, "n:笔记.md").await;
    assert!(root.join("笔记.md").is_file());
    assert!(browser.is_path_selected(&root.join("笔记.md")));
    assert!(browser.renaming.is_none(), "the name was already given");

    type_and_run(&mut browser, "n：素材/").await;
    assert!(root.join("素材").is_dir());
    assert_eq!(browser.operation_history.undo_depth(), 2);
    assert_eq!(
        browser.user_config.super_box_history.entries(),
        ["n:素材/", "n:笔记.md"]
    );
}

#[tokio::test]
async fn a_taken_name_is_refused_before_anything_is_queued() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("已有.txt"), b"x").unwrap();
    let mut browser = browser_in(&root);
    let load = browser.navigate_to(root.clone(), crate::model::NavigationMode::RecordHistory);
    drive(&mut browser, load).await;

    type_and_run(&mut browser, "n:已有.txt").await;
    assert!(browser.operation_queue.tasks().is_empty());
    assert!(browser.super_box.is_some());
}

#[tokio::test]
async fn a_history_row_fills_the_box_instead_of_running() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);
    browser
        .user_config
        .super_box_history
        .record_submission("n:不要马上建.txt");
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    send(&mut browser, SuperBoxMessage::ModePressed).await;
    send(&mut browser, SuperBoxMessage::ModePressed).await;
    send(&mut browser, SuperBoxMessage::Submitted).await;
    let super_box = browser.super_box.as_ref().unwrap();
    assert_eq!(super_box.mode(), BoxMode::New);
    assert_eq!(super_box.input(), "不要马上建.txt");
    assert!(!root.join("不要马上建.txt").exists());
}
