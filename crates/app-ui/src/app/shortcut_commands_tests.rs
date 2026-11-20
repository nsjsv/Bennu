use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::event;
use iced::keyboard::{key, Key, Modifiers};

use super::FileBrowser;
use crate::config::{self, ViewDensityLevel};
use crate::model::{BrowserLocation, BrowserViewMode, DestructiveActionConfirmation};
use crate::shortcuts::{FileContentKey, ShortcutAction};
use file_operations::QueuedFileOperation;

fn entry(path: &Path, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        path.to_path_buf(),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

/// /workspace 里一个文件、一个文件夹，选中文件。
fn browser_in_workspace() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.tab_mut().entries = vec![
        entry(Path::new("/workspace/report.txt"), FileKind::File),
        entry(Path::new("/workspace/photos"), FileKind::Directory),
    ]
    .into();
    browser.select_path(PathBuf::from("/workspace/report.txt"));
    browser
}

fn press(browser: &mut FileBrowser, key: Key, modifiers: Modifiers) {
    drop(browser.handle_keyboard_key_pressed(key, modifiers, event::Status::Ignored));
}

fn character(value: &str) -> Key {
    Key::Character(value.into())
}

/// 文件区的键要先探测文本焦点（任务里的控件操作），测试里直接走探测之后的那一步。
fn routed(browser: &mut FileBrowser, action: ShortcutAction) {
    drop(browser.invoke_routed_file_content_key(FileContentKey::Action(action)));
}

fn open_tab_locations(browser: &FileBrowser) -> Vec<BrowserLocation> {
    browser
        .pane()
        .tabs
        .iter()
        .map(|tab| tab.location.clone())
        .collect()
}

#[test]
fn tab_keys_open_cycle_jump_and_move_tabs() {
    let mut browser = browser_in_workspace();
    press(&mut browser, character("t"), Modifiers::CTRL);
    press(&mut browser, character("t"), Modifiers::CTRL);
    assert_eq!(browser.pane().tabs.len(), 3);
    let ids = browser
        .pane()
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(browser.pane().active_tab_id, ids[2]);
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(PathBuf::from("/workspace"))
    );

    press(&mut browser, Key::Named(key::Named::Tab), Modifiers::CTRL);
    assert_eq!(browser.pane().active_tab_id, ids[0], "到头绕回第一个");
    press(
        &mut browser,
        Key::Named(key::Named::PageUp),
        Modifiers::CTRL,
    );
    assert_eq!(browser.pane().active_tab_id, ids[2]);

    press(&mut browser, character("1"), Modifiers::ALT);
    assert_eq!(browser.pane().active_tab_id, ids[0]);
    press(&mut browser, character("9"), Modifiers::ALT);
    assert_eq!(browser.pane().active_tab_id, ids[2], "Alt+9 是最后一个");
    press(&mut browser, character("5"), Modifiers::ALT);
    assert_eq!(browser.pane().active_tab_id, ids[2], "没有第 5 个就不动");

    press(
        &mut browser,
        Key::Named(key::Named::PageUp),
        Modifiers::CTRL | Modifiers::SHIFT,
    );
    let order = browser
        .pane()
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(order, [ids[0], ids[2], ids[1]]);
}

#[test]
fn application_keys_do_nothing_while_another_window_has_focus() {
    let mut browser = browser_in_workspace();
    let settings = iced::window::Id::unique();
    browser.settings_window = Some(settings);
    browser.focused_window = settings;

    press(&mut browser, character("t"), Modifiers::CTRL);
    press(&mut browser, character("2"), Modifiers::ALT);

    assert_eq!(open_tab_locations(&browser).len(), 1);
}

#[test]
fn view_and_zoom_keys_change_the_active_view() {
    let mut browser = browser_in_workspace();
    press(&mut browser, character("2"), Modifiers::CTRL);
    assert_eq!(browser.tab().view_mode(), BrowserViewMode::List);

    let start = browser.user_config.list_view_density;
    press(&mut browser, character("="), Modifiers::CTRL);
    assert_eq!(
        browser.user_config.list_view_density,
        start.step(config::ViewDensityStep::Increase)
    );
    // 主键盘上的 + 是 Shift+=。
    press(
        &mut browser,
        character("="),
        Modifiers::CTRL | Modifiers::SHIFT,
    );
    press(&mut browser, character("0"), Modifiers::CTRL);
    assert_eq!(
        browser.user_config.list_view_density,
        ViewDensityLevel::DEFAULT
    );
    press(&mut browser, character("-"), Modifiers::CTRL);
    assert_eq!(
        browser.user_config.list_view_density,
        ViewDensityLevel::DEFAULT.step(config::ViewDensityStep::Decrease)
    );
}

#[test]
fn navigation_keys_toggle_hidden_files_and_go_home() {
    let mut browser = browser_in_workspace();
    let shown = browser.options.include_hidden;
    press(&mut browser, character("h"), Modifiers::CTRL);
    assert_eq!(browser.options.include_hidden, !shown);

    press(&mut browser, Key::Named(key::Named::Home), Modifiers::ALT);
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(file_core::app_dirs::home_dir())
    );
}

#[test]
fn new_folder_and_new_file_go_into_the_current_folder() {
    let mut browser = browser_in_workspace();
    routed(&mut browser, ShortcutAction::NewFolder);
    routed(&mut browser, ShortcutAction::NewFile);

    let operations = browser
        .operation_queue
        .tasks()
        .iter()
        .map(|task| task.operation.clone())
        .collect::<Vec<_>>();
    assert!(matches!(
        operations.as_slice(),
        [
            QueuedFileOperation::CreateDirectory { parent: folder_parent },
            QueuedFileOperation::CreateEmptyFile { parent: file_parent },
        ] if folder_parent == Path::new("/workspace") && file_parent == Path::new("/workspace")
    ));
}

#[test]
fn shift_delete_asks_before_deleting_permanently() {
    let mut browser = browser_in_workspace();
    routed(&mut browser, ShortcutAction::DeletePermanently);

    assert!(matches!(
        browser.destructive_action_confirmation,
        Some(DestructiveActionConfirmation::DeletePermanently { ref paths })
            if paths == &vec![PathBuf::from("/workspace/report.txt")]
    ));
}

#[test]
fn ctrl_enter_opens_only_folders_in_a_new_tab() {
    let mut browser = browser_in_workspace();
    routed(&mut browser, ShortcutAction::OpenInNewTab);
    assert_eq!(open_tab_locations(&browser).len(), 1);

    browser.select_path(PathBuf::from("/workspace/photos"));
    routed(&mut browser, ShortcutAction::OpenInNewTab);
    assert_eq!(
        open_tab_locations(&browser),
        [
            BrowserLocation::Directory(PathBuf::from("/workspace")),
            BrowserLocation::Directory(PathBuf::from("/workspace/photos")),
        ]
    );
}

#[test]
fn ctrl_b_adds_the_selected_folder_or_else_the_current_one() {
    let favorites = |browser: &FileBrowser| {
        browser
            .sidebar_locations
            .iter()
            .filter(|location| location.kind.is_user_favorite())
            .map(|location| location.path.clone())
            .collect::<HashSet<_>>()
    };

    let mut browser = browser_in_workspace();
    routed(&mut browser, ShortcutAction::AddToFavorites);
    assert!(favorites(&browser).contains(Path::new("/workspace")));

    browser.select_path(PathBuf::from("/workspace/photos"));
    routed(&mut browser, ShortcutAction::AddToFavorites);
    assert!(favorites(&browser).contains(Path::new("/workspace/photos")));
}

#[test]
fn every_default_key_reaches_its_action() {
    let config = config::default_user_config();
    for action in ShortcutAction::all() {
        for press in action.default_keys() {
            let area = if action.group() == crate::shortcuts::ShortcutGroup::Terminal {
                crate::shortcuts::KeyArea::Terminal
            } else {
                crate::shortcuts::KeyArea::FileBrowser
            };
            assert_eq!(
                config.shortcuts.live(|_| false).target_for(area, press),
                Some((*action).into()),
                "{}",
                press.config_value()
            );
            assert!(
                crate::shortcuts::fixed_key(press).is_none(),
                "{action:?} default {} is a fixed key",
                press.config_value()
            );
        }
    }
}
