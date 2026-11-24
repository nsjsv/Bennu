use std::path::Path;

use super::*;
use crate::actions::type_filter::TypeFilter;
use crate::actions::{ActionDefinition, ActionId, ActionLibrary, ActionNotice, ActionStep};
use crate::app::actions::{ActionMessage, ActionSubject};
use crate::config;
use crate::context_menu_rows::{file_context_menu_rows, quick_action_rows, MenuRow};
use crate::model::{ContextMenuState, FileAreaMenuItem, Message};

fn definition(stem: &str) -> ActionDefinition {
    ActionDefinition {
        id: ActionId::from_file_stem(stem),
        name: stem.to_owned(),
        file: PathBuf::from(format!("/actions/{stem}.toml")),
        types: TypeFilter::Anything,
        steps: vec![ActionStep::CopyPaths],
        notice: ActionNotice::Silent,
        watch: None,
    }
}

fn load_actions(browser: &mut FileBrowser, stems: &[&str]) {
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(
        ActionLibrary::from_definitions(stems.iter().map(|stem| definition(stem)).collect()),
    )));
}

/// 快捷操作面板显示着，位置在 `directory`。
fn browser_in(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    drop(browser.toggle_dock_panel(DockPanel::QuickActions));
    load_actions(&mut browser, &["for-anything"]);
    browser
}

/// 摊开子菜单后的所有行名。
fn labels(rows: &[MenuRow]) -> Vec<String> {
    rows.iter()
        .flat_map(|row| {
            std::iter::once(row.label.clone())
                .chain(row.members().iter().map(|member| member.label.clone()))
        })
        .collect()
}

fn panel_labels(browser: &mut FileBrowser) -> Vec<String> {
    drop(browser.sync_right_dock_panels());
    let menu = browser.quick_actions.menu().expect("the panel has a menu");
    labels(&quick_action_rows(menu, &browser.user_config.context_menus))
}

fn right_click_labels(browser: &mut FileBrowser, path: PathBuf) -> Vec<String> {
    drop(browser.open_entry_context_menu(path, iced::Point::ORIGIN));
    let Some(ContextMenuState::FileArea(menu)) = &browser.context_menu else {
        panic!("the file menu is not open");
    };
    labels(&file_context_menu_rows(
        menu,
        &browser.user_config.context_menus,
        browser.keyboard_modifiers,
        browser.user_config.file_grouping,
    ))
}

#[test]
fn the_panel_shows_what_the_right_click_menu_offers_for_the_selection() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("photos.zip");
    let notes = directory.path().join("notes.txt");
    let mut browser = browser_in(directory.path());
    let extract = FileAreaMenuItem::SmartExtractHere.label();
    let checksum = FileAreaMenuItem::FileChecksum.label();

    for (path, offers_extract) in [(archive, true), (notes, false)] {
        browser.select_path(path.clone());
        let panel = panel_labels(&mut browser);
        let menu = right_click_labels(&mut browser, path.clone());

        for label in &panel {
            assert!(
                menu.contains(label),
                "{label} is not in the menu for {path:?}"
            );
        }
        assert_eq!(panel.contains(&extract), offers_extract, "{path:?}");
        assert!(panel.contains(&checksum), "{path:?}");
        assert!(panel.contains(&"for-anything".to_owned()), "{path:?}");
    }
}

#[test]
fn the_panel_follows_the_selection_and_the_actions_without_being_told() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("photos.zip");
    let notes = directory.path().join("notes.txt");
    let mut browser = browser_in(directory.path());
    let extract = FileAreaMenuItem::SmartExtractHere.label();

    // 没选中：对当前文件夹，没有只给文件的项。
    let folder = panel_labels(&mut browser);
    assert_eq!(folder, ["for-anything"]);

    browser.select_path(notes);
    assert!(!panel_labels(&mut browser).contains(&extract));

    // 焦点项没变、只是多选了一个压缩包（不经过任何通知）：面板也跟着变。
    browser.tab_mut().selected_paths.insert(archive);
    assert!(panel_labels(&mut browser).contains(&extract));

    // 动作重新读过（编辑器里加了一个）也跟着变。
    load_actions(&mut browser, &["for-anything", "added-later"]);
    assert!(panel_labels(&mut browser).contains(&"added-later".to_owned()));
}

#[test]
fn an_action_in_the_panel_runs_on_the_selection_like_the_menu() {
    let directory = tempfile::tempdir().unwrap();
    let notes = directory.path().join("notes.txt");
    let mut browser = browser_in(directory.path());
    browser.select_path(notes);
    drop(browser.sync_right_dock_panels());

    let rows = quick_action_rows(
        browser.quick_actions.menu().unwrap(),
        &browser.user_config.context_menus,
    );
    let row = rows.iter().find(|row| row.label == "for-anything").unwrap();
    assert!(matches!(
        row.press_message(),
        Message::Action(ActionMessage::Invoked {
            subject: ActionSubject::Selection,
            ..
        })
    ));
}

#[test]
fn the_trash_has_no_quick_actions() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    browser.tab_mut().location = BrowserLocation::Trash;
    drop(browser.sync_right_dock_panels());
    assert!(browser.quick_actions.menu().is_none());
}
