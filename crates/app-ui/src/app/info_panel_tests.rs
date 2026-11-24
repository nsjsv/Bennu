use std::collections::HashSet;
use std::path::PathBuf;

use iced::widget::text_editor;

use super::*;
use crate::app::focused_control_keys_tests::press_key_in_focused_control;
use crate::app::info_panel_rows::InfoValueRow;
use crate::config;
use crate::model::BrowserLocation;

/// 停靠区显示着（默认布局里信息面板是当前标签），活动窗格在 /workspace。
fn browser() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.right_dock.toggle_visible();
    browser
}

fn select(browser: &mut FileBrowser, paths: &[&str]) {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    *browser.tab_mut().selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
    browser.tab_mut().selected = paths.last().cloned();
}

fn snapshot_of(path: &str) -> FilePropertiesSnapshot {
    FilePropertiesSnapshot {
        name: PathBuf::from(path).file_name().unwrap().to_owned(),
        location: PathBuf::from("/workspace"),
        created: None,
        modified: None,
        accessed: None,
        changed: None,
        size_bytes: 1,
        disk_size_bytes: 4096,
        details: crate::model::FilePropertiesKindDetails::Other,
        owner: crate::model::FilePropertiesOwner {
            user: "me".to_owned(),
            group: "me".to_owned(),
        },
        volume: Err("none".to_owned()),
        permissions: None,
    }
}

#[test]
fn the_target_is_the_folder_one_item_or_the_selection() {
    let mut browser = browser();
    drop(browser.sync_info_panel());
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Folder(PathBuf::from("/workspace")))
    );

    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Entry(PathBuf::from("/workspace/a.txt")))
    );

    let request = browser.info_panel.request;
    select(&mut browser, &["/workspace/a.txt", "/workspace/b.txt"]);
    drop(browser.sync_info_panel());
    assert_eq!(browser.info_panel.target(), Some(&InfoTarget::Selection));
    assert_eq!(
        browser.info_panel.request, request,
        "several selected read nothing"
    );
    assert!(browser.info_panel.facts().is_none());
}

#[test]
fn nothing_is_read_while_the_info_panel_is_not_showing() {
    let mut browser = browser();
    // 叠到预览那格后面：不是当前标签。
    browser
        .right_dock
        .move_panel(DockPanel::Info, crate::right_dock::DropTarget::Join(0));
    browser.right_dock.select(DockPanel::Preview);
    select(&mut browser, &["/workspace/a.txt"]);

    drop(browser.sync_info_panel());

    assert_eq!(browser.info_panel.request, 0);
    assert_eq!(browser.info_panel.target(), None);
}

#[test]
fn a_result_for_an_earlier_target_is_dropped() {
    let mut browser = browser();
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    let first = browser.info_panel.request;
    select(&mut browser, &["/workspace/b.txt"]);
    drop(browser.sync_info_panel());

    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: first,
        outcome: Ok(Box::new(snapshot_of("/workspace/a.txt"))),
    }));
    assert!(browser.info_panel.facts().unwrap().snapshot.is_none());

    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(snapshot_of("/workspace/b.txt"))),
    }));
    let facts = browser.info_panel.facts().unwrap();
    assert_eq!(facts.path, PathBuf::from("/workspace/b.txt"));
    assert!(facts.snapshot.is_some());
}

#[test]
fn a_toggled_field_is_kept_in_the_settings() {
    let mut browser = browser();
    assert!(browser.user_config.info_panel.shows(InfoField::Owner));
    drop(browser.handle_info_panel_message(InfoPanelMessage::FieldToggled(InfoField::Owner)));
    assert!(!browser.user_config.info_panel.shows(InfoField::Owner));
}

/// 选中 a.txt、快速读取到了：面板里有标题和各字段的行。
fn browser_showing_a_loaded_file() -> FileBrowser {
    let mut browser = browser();
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(snapshot_of("/workspace/a.txt"))),
    }));
    drop(browser.sync_info_panel());
    browser
}

fn row_keyed_by<'a>(browser: &'a FileBrowser, label: &str) -> &'a InfoValueRow {
    browser
        .info_panel
        .rows()
        .values()
        .iter()
        .find(|row| row.label == label)
        .unwrap_or_else(|| panic!("no row labelled {label}"))
}

fn keys(browser: &FileBrowser) -> Vec<(String, u64)> {
    browser
        .info_panel
        .rows()
        .values()
        .iter()
        .map(|row| (row.label.clone(), row.key))
        .collect()
}

fn act_on(browser: &mut FileBrowser, row_key: u64, action: text_editor::Action) {
    drop(browser.handle_info_panel_message(InfoPanelMessage::ValueAction { row_key, action }));
}

#[test]
fn rows_are_stored_when_the_facts_arrive() {
    let mut browser = browser();
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    assert_eq!(browser.info_panel.rows().title(), Some("a.txt"));
    assert!(
        browser.info_panel.rows().values().is_empty(),
        "nothing is known before the read finishes"
    );

    let browser = browser_showing_a_loaded_file();
    let location = row_keyed_by(&browser, &tr!("Location"));
    assert_eq!(location.value.text(), "/workspace");
    assert!(browser.info_panel.rows().values().len() > 1);
}

#[test]
fn an_unrelated_update_keeps_the_rows_and_the_selection_in_them() {
    let mut browser = browser_showing_a_loaded_file();
    let location = row_keyed_by(&browser, &tr!("Location")).key;
    act_on(&mut browser, location, text_editor::Action::SelectAll);
    let before = keys(&browser);

    drop(browser.sync_info_panel());
    drop(browser.sync_info_panel());

    assert_eq!(keys(&browser), before);
    assert_eq!(
        row_keyed_by(&browser, &tr!("Location"))
            .value
            .selection()
            .as_deref(),
        Some("/workspace")
    );
}

#[test]
fn new_facts_replace_only_the_rows_whose_text_changed() {
    let mut browser = browser_showing_a_loaded_file();
    let before = keys(&browser);
    let location = row_keyed_by(&browser, &tr!("Location")).key;
    let size = row_keyed_by(&browser, &tr!("Size")).key;
    act_on(&mut browser, location, text_editor::Action::SelectAll);
    act_on(&mut browser, size, text_editor::Action::SelectAll);

    let mut bigger = snapshot_of("/workspace/a.txt");
    bigger.size_bytes = 5000;
    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(bigger)),
    }));
    drop(browser.sync_info_panel());

    let after = keys(&browser);
    assert_eq!(after.len(), before.len());
    for ((label, old_key), (_, new_key)) in before.iter().zip(&after) {
        if *label == tr!("Size") {
            assert_ne!(old_key, new_key, "the changed row is a new row");
        } else {
            assert_eq!(old_key, new_key, "{label} did not change");
        }
    }
    assert!(row_keyed_by(&browser, &tr!("Size"))
        .value
        .selection()
        .is_none());
    assert!(row_keyed_by(&browser, &tr!("Location"))
        .value
        .selection()
        .is_some());
}

/// 换了条目，哪怕两个文件各行的文字一模一样，也不能沿用旧行：旧行里的焦点和选区属于上一个文件。
#[test]
fn a_new_target_gets_new_rows_even_when_the_text_is_the_same() {
    let mut browser = browser_showing_a_loaded_file();
    let before = keys(&browser);

    select(&mut browser, &["/workspace/b.txt"]);
    drop(browser.sync_info_panel());
    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(snapshot_of("/workspace/b.txt"))),
    }));
    drop(browser.sync_info_panel());

    let after = keys(&browser);
    assert_eq!(browser.info_panel.rows().title(), Some("b.txt"));
    assert_eq!(after.len(), before.len());
    assert!(after
        .iter()
        .zip(&before)
        .all(|((_, new_key), (_, old_key))| new_key != old_key));
}

#[test]
fn an_edit_never_changes_what_a_value_says() {
    let mut browser = browser_showing_a_loaded_file();
    let location = row_keyed_by(&browser, &tr!("Location")).key;
    act_on(&mut browser, location, text_editor::Action::SelectAll);

    for edit in [
        text_editor::Edit::Insert('x'),
        text_editor::Edit::Paste(std::sync::Arc::new("pasted".to_owned())),
        text_editor::Edit::Enter,
        text_editor::Edit::Backspace,
        text_editor::Edit::Delete,
    ] {
        act_on(&mut browser, location, text_editor::Action::Edit(edit));
    }

    let value = &row_keyed_by(&browser, &tr!("Location")).value;
    assert_eq!(value.text(), "/workspace");
    assert_eq!(value.selection().as_deref(), Some("/workspace"));
}

#[test]
fn an_action_for_a_row_that_was_replaced_is_dropped() {
    let mut browser = browser_showing_a_loaded_file();
    let old_size = row_keyed_by(&browser, &tr!("Size")).key;
    let mut bigger = snapshot_of("/workspace/a.txt");
    bigger.size_bytes = 5000;
    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(bigger)),
    }));
    drop(browser.sync_info_panel());

    act_on(&mut browser, old_size, text_editor::Action::SelectAll);
    act_on(&mut browser, u64::MAX, text_editor::Action::SelectAll);

    assert!(browser
        .info_panel
        .rows()
        .values()
        .iter()
        .all(|row| row.value.selection().is_none()));
}

#[test]
fn toggling_a_field_changes_the_rows_and_keeps_the_others() {
    let mut browser = browser_showing_a_loaded_file();
    let before = keys(&browser);
    assert!(before.iter().any(|(label, _)| *label == tr!("Owner")));

    drop(browser.handle_info_panel_message(InfoPanelMessage::FieldToggled(InfoField::Owner)));
    drop(browser.sync_info_panel());

    let after = keys(&browser);
    assert!(!after.iter().any(|(label, _)| *label == tr!("Owner")));
    let without_owner: Vec<_> = before
        .into_iter()
        .filter(|(label, _)| *label != tr!("Owner"))
        .collect();
    assert_eq!(after, without_owner);
}

#[test]
fn a_changed_folder_count_replaces_only_its_row() {
    let mut browser = browser();
    drop(browser.sync_info_panel());
    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(snapshot_of("/workspace"))),
    }));
    drop(browser.sync_info_panel());
    let before = keys(&browser);
    let items = row_keyed_by(&browser, &tr!("Items"));
    assert_eq!(items.value.text(), "0");

    browser.tab_mut().entries = vec![file_entry("/workspace/a.txt")].into();
    drop(browser.sync_info_panel());

    assert_eq!(row_keyed_by(&browser, &tr!("Items")).value.text(), "1");
    for ((label, old_key), (_, new_key)) in before.iter().zip(keys(&browser).iter()) {
        assert_eq!(
            label == &tr!("Items"),
            old_key != new_key,
            "only the changed row is replaced ({label})"
        );
    }
}

fn file_entry(path: &str) -> file_core::DirectoryEntry {
    file_core::DirectoryEntry::new(
        PathBuf::from(path),
        file_core::FileKind::File,
        file_core::EntryMetadata::default(),
        false,
        false,
        false,
    )
}

/// 文件区被按键动过的痕迹：选中的、排进队的任务、待粘贴的、要确认的删除。
fn file_area_trace(browser: &FileBrowser) -> (Option<PathBuf>, usize, usize, bool, bool) {
    (
        browser.tab().selected.clone(),
        browser.tab().selected_paths.len(),
        browser.operation_queue.tasks().len(),
        browser.pending_operation.is_some(),
        browser.destructive_action_confirmation.is_some(),
    )
}

fn browser_with_two_files() -> FileBrowser {
    let mut browser = browser();
    browser.tab_mut().entries = vec![
        file_entry("/workspace/report.txt"),
        file_entry("/workspace/notes.txt"),
    ]
    .into();
    select(&mut browser, &["/workspace/report.txt"]);
    browser
}

/// 按键先落到聚焦着的信息面板值上（空渲染器里的真实控件，用跟面板一样的消息），再把
/// 控件有没有捕获照实带进快捷键路由。
fn press_in_focused_value(
    browser: &mut FileBrowser,
    key: iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    text: Option<&str>,
) -> iced::event::Status {
    use bennu_theme::controls::{selectable_text, SelectableTextContent};

    let content = SelectableTextContent::<()>::new("/workspace");
    let value = selectable_text(&content, |action| {
        Message::InfoPanel(InfoPanelMessage::ValueAction { row_key: 1, action })
    });
    let (status, messages) = press_key_in_focused_control(value, key, modifiers, text);
    // 控件发出的消息（编辑动作等）也照常进应用。
    for message in messages {
        drop(browser.update(message));
    }
    status
}

/// 聚焦着的值吞掉的键：文件区一点没动、也没排出要去探测焦点的后续任务；同样的键要是没被
/// 捕获，文件区会动，或者排出探测任务（字母、Ctrl+A、Ctrl+C 走探测，Delete 直接动手）。
#[test]
fn keys_typed_in_a_focused_value_do_not_reach_the_file_area() {
    use iced::keyboard::{key::Named, Key, Modifiers};

    let keys = [
        (
            "Delete",
            Key::Named(Named::Delete),
            Modifiers::default(),
            None,
        ),
        (
            "a letter",
            Key::Character("n".into()),
            Modifiers::default(),
            Some("n"),
        ),
        ("Ctrl+A", Key::Character("a".into()), Modifiers::CTRL, None),
        ("Ctrl+C", Key::Character("c".into()), Modifiers::CTRL, None),
    ];
    for (name, key, modifiers, text) in keys {
        let mut browser = browser_with_two_files();
        let before = file_area_trace(&browser);
        let status = press_in_focused_value(&mut browser, key.clone(), modifiers, text);
        assert_eq!(status, iced::event::Status::Captured, "{name}");
        let follow_up =
            browser.handle_keyboard_key_pressed(key.clone(), key.clone(), modifiers, status);
        assert_eq!(follow_up.units(), 0, "{name} queued work");
        assert_eq!(
            file_area_trace(&browser),
            before,
            "{name} reached the files"
        );

        let mut browser = browser_with_two_files();
        let follow_up = browser.handle_keyboard_key_pressed(
            key.clone(),
            key,
            modifiers,
            iced::event::Status::Ignored,
        );
        assert!(
            follow_up.units() > 0 || file_area_trace(&browser) != before,
            "{name} does nothing in the file area, the check above proves nothing"
        );
    }
}

/// Esc 在聚焦着的值里只是离开这个值：控件捕获它，文件选择和面板的目标都不动。没被捕获的
/// Esc 才是应用的 Esc，会清掉选择。
#[test]
fn escape_in_a_focused_value_leaves_the_selection_and_the_panel_target_alone() {
    use iced::keyboard::{key::Named, Key, Modifiers};
    let escape = Key::Named(Named::Escape);

    let mut browser = browser_with_two_files();
    drop(browser.sync_info_panel());
    let target = browser.info_panel.target().cloned();
    assert!(matches!(target, Some(InfoTarget::Entry(_))));

    let status = press_in_focused_value(&mut browser, escape.clone(), Modifiers::default(), None);
    assert_eq!(status, iced::event::Status::Captured);
    drop(browser.handle_keyboard_key_pressed(
        escape.clone(),
        escape.clone(),
        Modifiers::default(),
        status,
    ));
    drop(browser.sync_info_panel());

    assert_eq!(
        browser.tab().selected,
        Some(PathBuf::from("/workspace/report.txt"))
    );
    assert_eq!(browser.info_panel.target(), target.as_ref());

    // 对照：同一个 Esc 要是没人捕获，就是应用的 Esc，选择会被清掉。
    let mut browser = browser_with_two_files();
    drop(browser.handle_keyboard_key_pressed(
        escape.clone(),
        escape,
        Modifiers::default(),
        iced::event::Status::Ignored,
    ));
    assert_eq!(browser.tab().selected, None);
}
