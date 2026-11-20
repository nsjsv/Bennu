use std::path::Path;

use super::task_driving_tests::drive;
use super::{FileBrowser, SuperBoxMessage};
use crate::model::{BrowserLocation, Message, NavigationMode};
use crate::shortcuts::ShortcutAction;
use crate::super_box::BoxRow;

/// 列出这个文件夹（改名要知道这里已经有哪些名字），窗口在前台。
async fn browser_listing(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
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

async fn enter(browser: &mut FileBrowser) {
    let pressed = browser
        .super_box_key(
            &iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            iced::keyboard::Modifiers::empty(),
        )
        .expect("the box takes the key");
    drive(browser, pressed).await;
}

#[tokio::test]
async fn one_selected_item_is_renamed_and_its_name_is_the_suggestion() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("草稿.txt"), b"x").unwrap();
    let mut browser = browser_listing(&root).await;
    browser.select_path(root.join("草稿.txt"));

    type_in_box(&mut browser, "r:").await;
    assert_eq!(
        browser.super_box.as_ref().unwrap().suggestion(),
        Some("草稿.txt")
    );
    type_in_box(&mut browser, "r:定稿.txt").await;
    enter(&mut browser).await;

    assert!(browser.super_box.is_none());
    assert!(root.join("定稿.txt").exists() && !root.join("草稿.txt").exists());
    assert_eq!(browser.operation_history.undo_depth(), 1);
}

#[tokio::test]
async fn several_items_follow_the_pattern_and_undo_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for name in ["b.jpg", "a.jpg", "c.png"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let mut browser = browser_listing(&root).await;
    browser.select_operation_result_paths(vec![root.join("a.jpg"), root.join("b.jpg")]);

    type_in_box(&mut browser, "r:照片-{n}.{ext}").await;
    let rows = browser.super_box.as_ref().unwrap().rows().to_vec();
    let [BoxRow::Rename(renamed)] = rows.as_slice() else {
        panic!("{rows:?}");
    };
    assert_eq!(renamed.len(), 2);
    enter(&mut browser).await;

    assert_eq!(std::fs::read(root.join("照片-1.jpg")).unwrap(), b"a.jpg");
    assert_eq!(std::fs::read(root.join("照片-2.jpg")).unwrap(), b"b.jpg");
    assert!(root.join("c.png").exists(), "only the selection");
    assert_eq!(
        browser.operation_history.undo_depth(),
        1,
        "one entry for the batch"
    );
    let undo = browser.undo_file_operation();
    drive(&mut browser, undo).await;
    assert!(root.join("a.jpg").exists() && root.join("b.jpg").exists());
}

#[tokio::test]
async fn a_sum_is_worked_out_and_not_remembered() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_listing(directory.path()).await;
    type_in_box(&mut browser, "=1+2*3").await;
    assert_eq!(
        browser.super_box.as_ref().unwrap().rows(),
        [BoxRow::Answer("7".to_owned())]
    );
    enter(&mut browser).await;
    assert!(browser.super_box.is_none());
    assert!(browser.user_config.super_box_history.entries().is_empty());
}

#[tokio::test]
async fn the_box_asked_for_from_outside_opens_and_stays_open() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_listing(directory.path()).await;
    let asked = || {
        Message::DesktopActivationReceived(desktop_linux::DesktopActivationEvent::OpenSuperBox(
            desktop_linux::DesktopStartupId::none(),
        ))
    };
    drop(browser.update(asked()));
    assert!(browser.super_box.is_some());
    type_in_box(&mut browser, "留着").await;
    drop(browser.update(asked()));
    assert_eq!(
        browser
            .super_box
            .as_ref()
            .map(|super_box| super_box.input()),
        Some("留着")
    );
}
