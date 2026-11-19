use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::event;
use iced::keyboard::{key, Key, Modifiers};

use super::*;
use crate::config;
use crate::model::FilePropertiesMessage;
use crate::shortcuts::ShortcutAction;

fn browser_with_selected_file() -> (FileBrowser, PathBuf) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let file = PathBuf::from("/workspace/report.txt");
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = vec![DirectoryEntry::new(
        file.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )]
    .into();
    browser.select_path(file.clone());
    (browser, file)
}

fn press(browser: &mut FileBrowser, key: Key) {
    drop(browser.handle_keyboard_key_pressed(key, Modifiers::default(), event::Status::Ignored));
}

fn named(named: key::Named) -> Key {
    Key::Named(named)
}

fn menu(browser: &FileBrowser) -> &crate::model::FileContextMenuState {
    match &browser.context_menu {
        Some(ContextMenuState::FileArea(menu)) => menu,
        other => panic!("expected the file menu, got {other:?}"),
    }
}

fn rows(browser: &FileBrowser) -> Vec<MenuRow> {
    file_context_menu_rows(
        menu(browser),
        &browser.user_config.context_menus,
        Modifiers::default(),
        browser.user_config.file_grouping,
    )
}

#[test]
fn a_keyboard_opened_menu_starts_on_the_first_row_and_wraps() {
    let (mut browser, _) = browser_with_selected_file();
    drop(browser.open_context_menu_at_measured_selection(&[]));
    let count = rows(&browser).len();
    assert!(count > 2);
    assert_eq!(
        menu(&browser).keyboard_highlight,
        Some(MenuHighlight {
            row: 0,
            member: None
        })
    );

    press(&mut browser, named(key::Named::ArrowDown));
    assert_eq!(menu(&browser).keyboard_highlight.map(|h| h.row), Some(1));
    press(&mut browser, named(key::Named::ArrowUp));
    press(&mut browser, named(key::Named::ArrowUp));
    assert_eq!(
        menu(&browser).keyboard_highlight.map(|h| h.row),
        Some(count - 1)
    );
    press(&mut browser, named(key::Named::Home));
    assert_eq!(menu(&browser).keyboard_highlight.map(|h| h.row), Some(0));
}

#[test]
fn enter_runs_the_same_message_as_clicking_the_row() {
    let (browser, file) = browser_with_selected_file();
    let mut browser = browser;
    drop(browser.open_context_menu_at_measured_selection(&[]));
    let rows = rows(&browser);
    let properties = rows
        .iter()
        .position(|row| row.shortcut == Some(ShortcutAction::FileProperties))
        .expect("the file menu has Properties");

    let (_, message) = navigate(
        &rows,
        Some(MenuHighlight {
            row: properties,
            member: None,
        }),
        MenuKey::Enter,
    )
    .expect("Enter belongs to the menu");
    assert!(matches!(
        message,
        Some(Message::FileProperties(FilePropertiesMessage::Requested(path))) if path == file
    ));
}

#[test]
fn right_opens_a_submenu_left_and_escape_leave_it() {
    let (mut browser, _) = browser_with_selected_file();
    drop(browser.open_context_menu_at_measured_selection(&[]));
    let submenu_row = rows(&browser)
        .iter()
        .position(|row| !row.members().is_empty())
        .expect("the file menu has a submenu");
    browser.context_menu.as_mut().map(|state| match state {
        ContextMenuState::FileArea(menu) => {
            menu.keyboard_highlight = Some(MenuHighlight {
                row: submenu_row,
                member: None,
            })
        }
        _ => unreachable!(),
    });

    press(&mut browser, named(key::Named::ArrowRight));
    assert_eq!(
        menu(&browser).keyboard_highlight,
        Some(MenuHighlight {
            row: submenu_row,
            member: Some(0)
        })
    );
    assert_ne!(menu(&browser).expansion, FileContextMenuExpansion::None);

    press(&mut browser, named(key::Named::ArrowLeft));
    assert_eq!(
        menu(&browser).keyboard_highlight.and_then(|h| h.member),
        None
    );
    assert_eq!(menu(&browser).expansion, FileContextMenuExpansion::None);

    press(&mut browser, named(key::Named::Enter));
    press(&mut browser, named(key::Named::Escape));
    assert!(browser.context_menu.is_some(), "第一下 Esc 只退出子菜单");
    press(&mut browser, named(key::Named::Escape));
    assert!(browser.context_menu.is_none());
}

#[test]
fn keys_do_not_reach_the_files_while_the_menu_is_open() {
    let (mut browser, _) = browser_with_selected_file();
    drop(browser.open_context_menu_at_measured_selection(&[]));
    press(&mut browser, named(key::Named::Delete));
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn the_mouse_takes_over_from_the_keyboard_highlight() {
    let (mut browser, _) = browser_with_selected_file();
    drop(browser.open_context_menu_at_measured_selection(&[]));
    drop(browser.update_file_context_menu_expansion(FileContextMenuExpansion::None));
    assert_eq!(menu(&browser).keyboard_highlight, None);
}

#[test]
fn rows_carry_the_shortcut_of_what_they_do() {
    let (mut browser, _) = browser_with_selected_file();
    drop(browser.open_context_menu_at_measured_selection(&[]));
    let state = menu(&browser).clone();
    let shortcuts = |modifiers| {
        file_context_menu_rows(
            &state,
            &browser.user_config.context_menus,
            modifiers,
            browser.user_config.file_grouping,
        )
        .into_iter()
        .filter_map(|row| row.shortcut)
        .collect::<Vec<_>>()
    };
    let plain = shortcuts(Modifiers::default());
    assert!(plain.contains(&ShortcutAction::Copy));
    assert!(plain.contains(&ShortcutAction::Delete));
    // 按住 Alt / Shift 时行变成复制路径 / 永久删除，显示的键跟着变。
    assert!(shortcuts(Modifiers::ALT).contains(&ShortcutAction::CopyPath));
    assert!(shortcuts(Modifiers::SHIFT).contains(&ShortcutAction::DeletePermanently));
}
