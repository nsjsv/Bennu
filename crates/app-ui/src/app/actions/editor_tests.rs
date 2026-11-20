use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::actions::editor::{EditTarget, EditorStatus, StepKind};
use crate::actions::{ActionLibrary, ActionSources};
use crate::app::task_driving_tests::drive;
use crate::config::{self, UiLanguage};
use crate::model::BrowserLocation;

fn sources(folder: &Path) -> ActionSources {
    ActionSources {
        action_folder: folder.to_path_buf(),
        data_dirs: Vec::new(),
        show_settings: folder.join("kservicemenurc"),
        language: UiLanguage::English,
    }
}

/// 动作文件夹在临时目录里，设置窗口停在“动作”页。
fn browser_with_actions(folder: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.system_focused_window = Some(iced::window::Id::unique());
    browser.action_library = ActionLibrary::load(&sources(folder));
    drop(browser.select_settings_category(SettingsCategory::Actions));
    browser
}

fn reload_library(browser: &mut FileBrowser, folder: &Path) {
    browser.action_library = ActionLibrary::load(&sources(folder));
}

async fn send(browser: &mut FileBrowser, message: EditorMessage) {
    let task = browser.update(Message::Action(ActionMessage::Editor(message)));
    drive(browser, task).await;
}

async fn edit(browser: &mut FileBrowser, edit: DraftEdit) {
    send(browser, EditorMessage::Edit(edit)).await;
}

fn editor(browser: &FileBrowser) -> &ActionEditor {
    browser.action_editor().expect("the editor is open")
}

const COMMENTED: &str =
    "# 我写的\nname = \"打包\"   # 菜单里的名字\n\n[[steps]]\ncompress = \"zip\"\n";

async fn open_commented(browser: &mut FileBrowser, folder: &Path) {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(folder.join("打包.toml"), COMMENTED).unwrap();
    reload_library(browser, folder);
    send(
        browser,
        EditorMessage::Open(ActionId::from_file_stem("打包")),
    )
    .await;
}

#[tokio::test]
async fn a_new_action_is_saved_privately_loaded_and_runs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let folder = root.join("actions");
    let mut browser = browser_with_actions(&folder);

    send(&mut browser, EditorMessage::New(ActionTemplate::Blank)).await;
    assert!(!editor(&browser).can_save(), "a blank action has no steps");
    edit(&mut browser, DraftEdit::Name("整理".to_owned())).await;
    edit(&mut browser, DraftEdit::StepAdded).await;
    edit(&mut browser, DraftEdit::StepKind(0, StepKind::MoveTo)).await;
    edit(
        &mut browser,
        DraftEdit::StepText(0, "{dir}/归档".to_owned()),
    )
    .await;
    send(&mut browser, EditorMessage::Save).await;

    let file = folder.join("整理.toml");
    assert!(matches!(editor(&browser).status, EditorStatus::Saved));
    assert_eq!(editor(&browser).file(), Some(file.as_path()));
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
        0o700
    );

    reload_library(&mut browser, &folder);
    let definition = browser
        .action_library
        .definitions()
        .iter()
        .find(|definition| definition.name == "整理")
        .cloned()
        .expect("the saved action is loaded");
    assert_eq!(definition.id, ActionId::from_file_stem("整理"));

    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let note = work.join("note.txt");
    std::fs::write(&note, b"hello").unwrap();
    browser.tab_mut().location = BrowserLocation::Directory(work.clone());
    let run = browser.start_action_run(Arc::clone(&definition), vec![note.clone()]);
    drive(&mut browser, run).await;
    assert_eq!(std::fs::read(work.join("归档/note.txt")).unwrap(), b"hello");
}

#[tokio::test]
async fn an_edit_keeps_the_comments_and_leaving_with_changes_asks_first() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;
    assert!(!editor(&browser).has_unsaved_changes());

    edit(&mut browser, DraftEdit::Name("打成 zip".to_owned())).await;
    drop(browser.update(Message::SettingsSubpageClosed));
    assert_eq!(editor(&browser).leaving, Some(SettingsCategory::Actions));
    drop(browser.update(Message::SettingsCategorySelected(SettingsCategory::General)));
    assert_eq!(editor(&browser).leaving, Some(SettingsCategory::General));
    assert_eq!(
        browser.selected_settings_category,
        SettingsCategory::Actions
    );
    send(&mut browser, EditorMessage::LeaveCanceled).await;
    assert_eq!(editor(&browser).leaving, None);

    // 关掉设置窗口不算离开：草稿还在。
    drop(browser.close_settings_window());
    assert!(editor(&browser).has_unsaved_changes());

    send(&mut browser, EditorMessage::Save).await;
    assert_eq!(
        std::fs::read_to_string(folder.join("打包.toml")).unwrap(),
        COMMENTED.replace("\"打包\"", "\"打成 zip\"")
    );
    drop(browser.update(Message::SettingsSubpageClosed));
    assert!(browser.settings_subpage.is_none(), "nothing left unsaved");
}

#[tokio::test]
async fn discarding_goes_where_the_user_was_heading() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;
    edit(&mut browser, DraftEdit::StepAdded).await;
    drop(browser.update(Message::SettingsCategorySelected(
        SettingsCategory::Shortcuts,
    )));
    send(&mut browser, EditorMessage::LeaveConfirmed).await;
    assert!(browser.settings_subpage.is_none());
    assert_eq!(
        browser.selected_settings_category,
        SettingsCategory::Shortcuts
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("打包.toml")).unwrap(),
        COMMENTED
    );
}

#[tokio::test]
async fn a_file_changed_elsewhere_is_kept_until_the_user_chooses() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;
    edit(&mut browser, DraftEdit::Name("我的".to_owned())).await;
    let theirs = "name = \"别人的\"\n[[steps]]\ncopy_paths = true\n";
    std::fs::write(folder.join("打包.toml"), theirs).unwrap();

    send(&mut browser, EditorMessage::Save).await;
    assert!(matches!(
        &editor(&browser).status,
        EditorStatus::Changed { found: Some(found) } if found == theirs
    ));
    assert_eq!(
        std::fs::read_to_string(folder.join("打包.toml")).unwrap(),
        theirs
    );

    send(&mut browser, EditorMessage::Reload).await;
    assert_eq!(editor(&browser).draft.name, "别人的");
    assert!(!editor(&browser).has_unsaved_changes());

    edit(&mut browser, DraftEdit::Name("我的".to_owned())).await;
    std::fs::write(folder.join("打包.toml"), COMMENTED).unwrap();
    send(&mut browser, EditorMessage::Save).await;
    send(&mut browser, EditorMessage::SaveAnyway).await;
    assert!(matches!(editor(&browser).status, EditorStatus::Saved));
    assert!(std::fs::read_to_string(folder.join("打包.toml"))
        .unwrap()
        .contains("name = \"我的\""));
}

#[tokio::test]
async fn deleting_queues_a_move_to_the_trash() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;

    // 不跑队列：测试不往真的回收站里放东西。
    drop(browser.update(Message::Action(ActionMessage::Editor(
        EditorMessage::Delete,
    ))));
    assert!(browser.settings_subpage.is_none());
    let task = browser.operation_queue.tasks()[0].id;
    assert!(matches!(
        browser.operation_queue.operation(task),
        Some(QueuedFileOperation::Trash { paths }) if *paths == [folder.join("打包.toml")]
    ));
}

#[tokio::test]
async fn a_duplicate_waits_for_saving_and_is_a_new_file() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;
    edit(&mut browser, DraftEdit::StepAdded).await;
    send(&mut browser, EditorMessage::Duplicate).await;
    assert!(matches!(editor(&browser).target, EditTarget::File { .. }));

    edit(&mut browser, DraftEdit::StepRemoved(1)).await;
    send(&mut browser, EditorMessage::Duplicate).await;
    assert!(matches!(editor(&browser).target, EditTarget::New));
    send(&mut browser, EditorMessage::Save).await;
    let copy = folder.join(format!("{}.toml", tr!("%1 Copy", "打包")));
    assert!(
        std::fs::read_to_string(copy)
            .unwrap()
            .starts_with("# 我写的\n"),
        "comments are copied too"
    );
}

#[tokio::test]
async fn a_file_read_after_the_user_moved_on_does_not_open() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("打包.toml"), COMMENTED).unwrap();
    let mut browser = browser_with_actions(&folder);
    let open = browser.update(Message::Action(ActionMessage::Editor(EditorMessage::Open(
        ActionId::from_file_stem("打包"),
    ))));
    drop(browser.update(Message::SettingsCategorySelected(SettingsCategory::General)));
    drive(&mut browser, open).await;
    assert!(browser.settings_subpage.is_none());
}

#[tokio::test]
async fn saving_while_asked_to_discard_saves_and_then_leaves() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    let mut browser = browser_with_actions(&folder);
    open_commented(&mut browser, &folder).await;
    edit(&mut browser, DraftEdit::Name("新名字".to_owned())).await;
    drop(browser.update(Message::SettingsCategorySelected(SettingsCategory::Search)));
    send(&mut browser, EditorMessage::Save).await;
    assert!(browser.settings_subpage.is_none());
    assert_eq!(browser.selected_settings_category, SettingsCategory::Search);
    assert!(std::fs::read_to_string(folder.join("打包.toml"))
        .unwrap()
        .contains("新名字"));
}
