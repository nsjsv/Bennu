//! 动作在右键菜单里、在快捷键上。

use std::path::{Path, PathBuf};

use super::*;
use crate::actions::type_filter::{TypeFilter, TypePattern};
use crate::actions::{ActionNotice, ActionStep};
use crate::config;
use crate::context_menu_rows::{file_context_menu_rows, MenuRow, MenuRowKind};
use crate::model::ContextMenuState;
use crate::shortcuts::{FileContentKey, KeyArea, KeyPress, ShortcutTarget};

fn definition(stem: &str, types: TypeFilter) -> crate::actions::ActionDefinition {
    crate::actions::ActionDefinition {
        id: ActionId::from_file_stem(stem),
        name: stem.to_owned(),
        file: PathBuf::from(format!("/actions/{stem}.toml")),
        types,
        steps: vec![ActionStep::CopyPaths],
        notice: ActionNotice::Silent,
        watch: None,
    }
}

fn videos_only() -> TypeFilter {
    TypeFilter::Patterns(vec![TypePattern::parse(
        "video/*",
        &file_core::mime::MimeDatabase::system(),
    )
    .unwrap()])
}

fn browser_with(
    directory: &Path,
    definitions: Vec<crate::actions::ActionDefinition>,
) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(
        ActionLibrary::from_definitions(definitions),
    )));
    browser
}

fn menu_rows(browser: &FileBrowser) -> Vec<MenuRow> {
    let Some(ContextMenuState::FileArea(menu)) = &browser.context_menu else {
        panic!("the file menu is not open");
    };
    file_context_menu_rows(
        menu,
        &browser.user_config.context_menus,
        browser.keyboard_modifiers,
        browser.user_config.file_grouping,
    )
}

fn action_labels(rows: &[MenuRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| matches!(&row.kind, MenuRowKind::Action(Message::Action(_))))
        .map(|row| row.label.clone())
        .collect()
}

#[test]
fn up_to_four_actions_sit_in_the_menu_and_more_go_into_a_submenu() {
    let directory = tempfile::tempdir().unwrap();
    let few = (1..=4)
        .map(|index| definition(&format!("a{index}"), TypeFilter::Anything))
        .collect();
    let mut browser = browser_with(directory.path(), few);
    drop(browser.open_blank_context_menu(directory.path().to_path_buf(), iced::Point::ORIGIN));
    assert_eq!(
        action_labels(&menu_rows(&browser)),
        ["a1", "a2", "a3", "a4"]
    );

    let many = (1..=5)
        .map(|index| definition(&format!("a{index}"), TypeFilter::Anything))
        .collect();
    let mut browser = browser_with(directory.path(), many);
    drop(browser.open_blank_context_menu(directory.path().to_path_buf(), iced::Point::ORIGIN));
    let rows = menu_rows(&browser);
    assert!(action_labels(&rows).is_empty());
    let submenu = rows
        .iter()
        .find(|row| row.label == tr!("Actions"))
        .expect("an Actions submenu");
    assert_eq!(action_labels(submenu.members()).len(), 5);
}

#[test]
fn only_actions_for_the_selected_types_are_offered() {
    let directory = tempfile::tempdir().unwrap();
    let notes = directory.path().join("notes.txt");
    std::fs::write(&notes, b"text").unwrap();
    let mut browser = browser_with(
        directory.path(),
        vec![
            definition("for-videos", videos_only()),
            definition("for-anything", TypeFilter::Anything),
        ],
    );
    browser.select_path(notes.clone());
    drop(browser.open_entry_context_menu(notes, iced::Point::ORIGIN));
    assert_eq!(action_labels(&menu_rows(&browser)), ["for-anything"]);
}

#[test]
fn picking_an_action_in_the_menu_starts_it_on_the_folder() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_with(
        directory.path(),
        vec![definition("copy-paths", TypeFilter::Anything)],
    );
    drop(browser.open_blank_context_menu(directory.path().to_path_buf(), iced::Point::ORIGIN));
    let row = menu_rows(&browser)
        .into_iter()
        .find(|row| row.label == "copy-paths")
        .unwrap();
    assert_eq!(
        row.shortcut,
        Some(ShortcutTarget::Action(ActionId::from_file_stem(
            "copy-paths"
        )))
    );

    let Message::Action(message) = row.press_message() else {
        panic!("the row does not run an action");
    };
    drop(browser.handle_action_message(message));

    assert!(browser.context_menu.is_none());
    assert_eq!(browser.action_runs.runs_started(), 1);
}

#[test]
fn an_action_key_runs_it_or_says_why_not() {
    let directory = tempfile::tempdir().unwrap();
    let notes = directory.path().join("notes.txt");
    std::fs::write(&notes, b"text").unwrap();
    let mut browser = browser_with(
        directory.path(),
        vec![definition("for-videos", videos_only())],
    );
    let target = ShortcutTarget::Action(ActionId::from_file_stem("for-videos"));
    browser
        .user_config
        .shortcuts
        .set_keys(target.clone(), vec![KeyPress::ctrl_shift('E')]);
    assert_eq!(
        browser
            .keymap()
            .target_for(KeyArea::FileBrowser, &KeyPress::ctrl_shift('E')),
        Some(target),
        "the loaded library made the key live"
    );

    browser.select_path(notes);
    drop(
        browser.invoke_file_content_key(FileContentKey::UserAction(ActionId::from_file_stem(
            "for-videos",
        ))),
    );
    assert_eq!(browser.action_runs.runs_started(), 0);
    let hint = browser
        .action_hint
        .as_ref()
        .expect("a hint in the status bar");
    assert!(hint.text.contains("for-videos"), "{}", hint.text);

    // 没有选中项时对当前文件夹：视频动作同样不适用，文件夹动作就会跑。
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(
        ActionLibrary::from_definitions(vec![definition("anything", TypeFilter::Anything)]),
    )));
    browser.tab_mut().selected_paths.clear();
    browser.tab_mut().selected = None;
    drop(
        browser.invoke_file_content_key(FileContentKey::UserAction(ActionId::from_file_stem(
            "anything",
        ))),
    );
    assert_eq!(browser.action_runs.runs_started(), 1);
}

/// 动作文件夹先读完、设置后读进来（启动时就是这样，别的窗口改了设置也是这样）：
/// 动作的键照样起作用。
#[test]
fn action_keys_work_whichever_arrives_first_the_actions_or_the_settings() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_with(
        directory.path(),
        vec![definition("anything", TypeFilter::Anything)],
    );
    let target = ShortcutTarget::Action(ActionId::from_file_stem("anything"));
    let mut settings = config::default_user_config();
    settings
        .shortcuts
        .set_keys(target.clone(), vec![KeyPress::ctrl_shift('E')]);

    browser.user_config = settings;

    assert_eq!(
        browser
            .keymap()
            .target_for(KeyArea::FileBrowser, &KeyPress::ctrl_shift('E')),
        Some(target)
    );
}

fn service_menu(
    key: &str,
    name: &str,
    priority: crate::actions::service_menu::Priority,
    submenu: Option<&str>,
) -> crate::actions::ServiceMenuAction {
    crate::actions::ServiceMenuAction {
        priority,
        submenu: submenu.map(str::to_owned),
        ..crate::actions::ServiceMenuAction::for_tests(key, name, "true %f")
    }
}

/// 菜单里"动作"那一块：每一行的名字，子菜单行再带上它里面的名字。
fn action_area(rows: &[MenuRow]) -> Vec<(String, Vec<String>)> {
    rows.iter()
        .filter_map(|row| match &row.kind {
            MenuRowKind::Action(Message::Action(_)) => Some((row.label.clone(), Vec::new())),
            MenuRowKind::Submenu {
                expansion, members, ..
            } if matches!(
                expansion,
                crate::model::FileContextMenuExpansion::ServiceSubmenu(_)
            ) || *expansion
                == crate::model::FileContextMenuExpansion::Group(
                    crate::model::FileAreaMenuItem::Actions,
                ) =>
            {
                Some((
                    row.label.clone(),
                    members.iter().map(|member| member.label.clone()).collect(),
                ))
            }
            MenuRowKind::Action(_) | MenuRowKind::Submenu { .. } => None,
        })
        .collect()
}

#[test]
fn service_menus_sit_with_the_actions_in_the_order_dolphin_uses() {
    use crate::actions::service_menu::Priority::{Important, Normal, TopLevel};
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_with(
        directory.path(),
        vec![
            definition("a1", TypeFilter::Anything),
            definition("a2", TypeFilter::Anything),
        ],
    );
    let menus = vec![
        service_menu("top", "top", TopLevel, None),
        service_menu("k2", "alpha", Normal, None),
        service_menu("k1", "zeta", Normal, None),
        service_menu("c-b", "convert b", Normal, Some("Convert")),
        service_menu("c-a", "convert a", Normal, Some("Convert")),
        service_menu("x", "x", TopLevel, Some("Alpha")),
        service_menu("imp", "important", Important, None),
    ];
    let library = ActionLibrary::from_definitions(vec![
        definition("a1", TypeFilter::Anything),
        definition("a2", TypeFilter::Anything),
    ])
    .with_service_menus(menus);
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(library)));
    drop(browser.open_blank_context_menu(directory.path().to_path_buf(), iced::Point::ORIGIN));

    let owned = |items: &[&str]| {
        items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
    };
    // 自己的动作、Important、普通的项共 5 个，收进"动作"；同组里按动作键排，不按名字。
    // 子菜单不收起来，按优先级再按名字；TopLevel 的项放最后。
    assert_eq!(
        action_area(&menu_rows(&browser)),
        [
            (
                tr!("Actions"),
                owned(&["a1", "a2", "important", "zeta", "alpha"])
            ),
            ("Convert".to_owned(), owned(&["convert a", "convert b"])),
            ("Alpha".to_owned(), owned(&["x"])),
            ("top".to_owned(), Vec::new()),
        ]
    );
}

#[test]
fn a_few_service_menus_sit_inline_and_their_rows_carry_their_shortcut() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_with(directory.path(), Vec::new());
    let library =
        ActionLibrary::from_definitions(Vec::new()).with_service_menus(vec![service_menu(
            "one",
            "one",
            crate::actions::service_menu::Priority::Normal,
            None,
        )]);
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(library)));
    drop(browser.open_blank_context_menu(directory.path().to_path_buf(), iced::Point::ORIGIN));

    let rows = menu_rows(&browser);
    let row = rows.iter().find(|row| row.label == "one").expect("inline");
    assert_eq!(
        row.shortcut,
        Some(ShortcutTarget::Action(ActionId::service_menu(
            "test.desktop",
            "one"
        )))
    );
}
