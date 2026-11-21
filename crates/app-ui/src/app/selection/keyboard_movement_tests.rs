use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::futures::StreamExt;
use iced_runtime::Action;

use super::*;
use crate::config;
use crate::list_view::LIST_ROW_HEIGHT;
use crate::model::{BrowserLocation, ExpandedDirectory};
use crate::thumbnail_cache::ColumnViewport;

fn default_column_geometry() -> crate::three_column_view::ColumnGeometry {
    crate::three_column_view::ColumnGeometry::for_level(config::ViewDensityLevel::DEFAULT)
}

async fn widget_action_count(task: Task<Message>) -> usize {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return 0;
    };
    let mut count = 0;
    while let Some(action) = stream.next().await {
        if matches!(action, Action::Widget(_)) {
            count += 1;
        }
    }
    count
}

fn entry(index: usize) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(format!("/workspace/{index}.txt")),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn expanded_directory(entries: Vec<DirectoryEntry>, animation_progress: f32) -> ExpandedDirectory {
    ExpandedDirectory {
        entries: entries.into(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn browser_for_vertical_keyboard_navigation(
    view_mode: BrowserViewMode,
    viewport_height: f32,
) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().choose_view_mode(view_mode);
    browser.tab_mut().entries = (0..3).map(entry).collect::<Vec<_>>().into();
    let viewport = ColumnViewport {
        offset_y: 0.0,
        height: viewport_height,
    };
    if view_mode == BrowserViewMode::List {
        browser.pane_mut().list_viewport = Some(viewport);
    } else {
        let current_dir = browser.tab().current_directory().unwrap().to_path_buf();
        browser
            .pane_mut()
            .column_viewports
            .insert(current_dir, viewport);
    }
    browser.select_path(browser.tab().entries[0].path().to_path_buf());
    browser
}

#[test]
fn keyboard_reveal_uses_scroll_content_offsets() {
    let column_geometry = default_column_geometry();
    for (view_mode, viewport_height, expected_delta) in [
        (
            BrowserViewMode::List,
            LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT,
            LIST_ROW_HEIGHT,
        ),
        (
            BrowserViewMode::Columns,
            column_geometry.entries_top_padding + column_geometry.entry_height,
            column_geometry.entry_scroll_height,
        ),
    ] {
        let browser = browser_for_vertical_keyboard_navigation(view_mode, viewport_height);
        let (_, scroll_delta) = browser
            .keyboard_selection_scroll(browser.tab().entries[1].path())
            .expect("second row has a scroll target");

        assert_eq!(scroll_delta, expected_delta, "wrong delta in {view_mode:?}");
    }
}

#[test]
fn list_keyboard_reveal_counts_status_and_animation_rows() {
    let mut browser = browser_for_vertical_keyboard_navigation(BrowserViewMode::List, 500.0);
    let directory = DirectoryEntry::new(
        browser.tab().current_directory().unwrap().join("directory"),
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    );
    let sibling = entry(1);
    browser.tab_mut().entries = vec![directory.clone(), sibling.clone()].into();
    browser.tab_mut().expanded_directories.insert(
        directory.path().to_path_buf(),
        expanded_directory(Vec::new(), 1.0),
    );

    let bounds_for = |browser: &FileBrowser| {
        let pane = browser
            .pane_view(browser.active_pane_id())
            .expect("active pane");
        crate::transfer_placeholder_view::list_row_layout(browser, pane, LIST_ROW_HEIGHT)
            .vertical_bounds(pane, sibling.path(), LIST_HEADER_HEIGHT)
    };

    // 展开状态行计入:目录行 + 状态行之后才是 sibling。
    assert_eq!(
        bounds_for(&browser),
        Some((LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT * 2.0, LIST_ROW_HEIGHT))
    );

    let child = DirectoryEntry::new(
        directory.path().join("child.txt"),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    );
    browser.tab_mut().expanded_directories.insert(
        directory.path().to_path_buf(),
        expanded_directory(vec![child], 0.5),
    );

    // 动画中的子行按进度收缩:sibling 位于 1.0 + 0.5 行高处。
    assert_eq!(
        bounds_for(&browser),
        Some((LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT * 1.5, LIST_ROW_HEIGHT,))
    );
}

#[tokio::test]
async fn list_keyboard_navigation_reveals_offscreen_selection() {
    let mut browser = browser_for_vertical_keyboard_navigation(
        BrowserViewMode::List,
        LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT,
    );

    let widget_actions =
        widget_action_count(browser.move_file_selection(SelectionMove::cursor(MoveStep::Down)))
            .await;

    assert_eq!(
        browser.tab().selected,
        Some(browser.tab().entries[1].path().to_path_buf())
    );
    assert_eq!(widget_actions, 1);
}

#[tokio::test]
async fn column_keyboard_navigation_reveals_offscreen_selection() {
    let mut browser = browser_for_vertical_keyboard_navigation(
        BrowserViewMode::Columns,
        default_column_geometry().entries_top_padding + default_column_geometry().entry_height,
    );

    let widget_actions =
        widget_action_count(browser.move_file_selection(SelectionMove::cursor(MoveStep::Down)))
            .await;

    assert_eq!(
        browser.tab().selected,
        Some(browser.tab().entries[1].path().to_path_buf())
    );
    assert_eq!(widget_actions, 1);
}

#[tokio::test]
async fn visible_keyboard_selection_does_not_scroll() {
    for view_mode in [BrowserViewMode::List, BrowserViewMode::Columns] {
        let mut browser = browser_for_vertical_keyboard_navigation(view_mode, 500.0);

        let widget_actions =
            widget_action_count(browser.move_file_selection(SelectionMove::cursor(MoveStep::Down)))
                .await;

        assert_eq!(
            browser.tab().selected,
            Some(browser.tab().entries[1].path().to_path_buf())
        );
        assert_eq!(widget_actions, 0, "unexpected scroll in {view_mode:?}");
    }
}

#[test]
fn icon_grid_keyboard_navigation_uses_current_column_count() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().choose_view_mode(BrowserViewMode::Icons);
    browser.main_window_width = 500.0;
    browser.sidebar_width_when_shown = 0.0;
    browser.tab_mut().entries = (0..8).map(entry).collect::<Vec<_>>().into();
    browser.select_path(browser.tab().entries[1].path().to_path_buf());

    drop(browser.move_file_selection(SelectionMove::cursor(MoveStep::Down)));
    assert_eq!(
        browser.tab().selected,
        Some(browser.tab().entries[4].path().to_path_buf())
    );

    drop(browser.move_file_selection(SelectionMove::cursor(MoveStep::Left)));
    assert_eq!(
        browser.tab().selected,
        Some(browser.tab().entries[3].path().to_path_buf())
    );
}

#[test]
fn icon_grid_keyboard_navigation_clamps_in_incomplete_last_row() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().choose_view_mode(BrowserViewMode::Icons);
    browser.main_window_width = 500.0;
    browser.sidebar_width_when_shown = 0.0;
    browser.tab_mut().entries = (0..8).map(entry).collect::<Vec<_>>().into();
    browser.select_path(browser.tab().entries[5].path().to_path_buf());

    drop(browser.move_file_selection(SelectionMove::cursor(MoveStep::Down)));

    assert_eq!(
        browser.tab().selected,
        Some(browser.tab().entries[7].path().to_path_buf())
    );
}

#[test]
fn keyboard_parent_column_from_archive_inner_returns_to_archive_root() {
    // 包根在文件系统中是文件:从包内栏按 ← 必须能回到包根栏,
    // 与普通目录栏行为一致。
    let archive = PathBuf::from("/workspace/bundle.zip");
    let inner = archive.join("inner");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().choose_view_mode(BrowserViewMode::Columns);
    browser.tab_mut().entries = vec![DirectoryEntry::new(
        archive.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )]
    .into();
    browser.tab_mut().expanded_directories.insert(
        archive.clone(),
        expanded_directory(
            vec![DirectoryEntry::new(
                inner.clone(),
                FileKind::Directory,
                EntryMetadata::default(),
                false,
                false,
                false,
            )],
            1.0,
        ),
    );
    browser.tab_mut().deepest_open_column_directory = Some(inner.clone());
    browser.select_path(inner.clone());

    drop(browser.move_file_selection_to_parent_column());

    assert_eq!(browser.tab().selected.as_deref(), Some(archive.as_path()));
}

fn browser_with_entries(view_mode: BrowserViewMode, count: usize) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().choose_view_mode(view_mode);
    browser.tab_mut().entries = (0..count).map(entry).collect::<Vec<_>>().into();
    browser
}

fn selected_index(browser: &FileBrowser) -> Option<usize> {
    let selected = browser.tab().selected.as_deref()?;
    browser
        .tab()
        .entries
        .iter()
        .position(|entry| entry.path() == selected)
}

fn selected_indices(browser: &FileBrowser) -> Vec<usize> {
    let mut indices = browser
        .tab()
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| browser.tab().selected_paths.contains(entry.path()))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices
}

fn press(browser: &mut FileBrowser, movement: SelectionMove) {
    drop(browser.move_file_selection(movement));
}

#[test]
fn list_page_keys_move_by_the_visible_rows_minus_one() {
    let mut browser = browser_with_entries(BrowserViewMode::List, 30);
    let row_height =
        crate::list_view::ListGeometry::for_level(browser.user_config.list_view_density).row_height;
    browser.pane_mut().list_viewport = Some(ColumnViewport {
        offset_y: 0.0,
        height: LIST_HEADER_HEIGHT + row_height * 10.0,
    });
    browser.select_path(browser.tab().entries[0].path().to_path_buf());

    press(&mut browser, SelectionMove::cursor(MoveStep::PageDown));
    assert_eq!(selected_index(&browser), Some(9));
    press(&mut browser, SelectionMove::cursor(MoveStep::Last));
    assert_eq!(selected_index(&browser), Some(29));
    press(&mut browser, SelectionMove::cursor(MoveStep::PageUp));
    assert_eq!(selected_index(&browser), Some(20));
    press(&mut browser, SelectionMove::cursor(MoveStep::First));
    assert_eq!(selected_index(&browser), Some(0));
    // 到头之后再往上翻停在第一项。
    press(&mut browser, SelectionMove::cursor(MoveStep::PageUp));
    assert_eq!(selected_index(&browser), Some(0));
}

#[test]
fn shift_movement_extends_from_the_same_anchor_as_shift_click() {
    let mut browser = browser_with_entries(BrowserViewMode::List, 8);
    browser.select_path(browser.tab().entries[2].path().to_path_buf());

    press(&mut browser, SelectionMove::extend(MoveStep::Down));
    press(&mut browser, SelectionMove::extend(MoveStep::Down));
    assert_eq!(selected_indices(&browser), [2, 3, 4]);
    assert_eq!(selected_index(&browser), Some(4));

    // 越过锚点往回收：范围翻到锚点另一边。
    for _ in 0..3 {
        press(&mut browser, SelectionMove::extend(MoveStep::Up));
    }
    assert_eq!(selected_indices(&browser), [1, 2]);

    press(&mut browser, SelectionMove::extend(MoveStep::Last));
    assert_eq!(selected_indices(&browser), [2, 3, 4, 5, 6, 7]);

    // 不按 Shift 的移动回到单选，锚点跟着走。
    press(&mut browser, SelectionMove::cursor(MoveStep::Up));
    assert_eq!(selected_indices(&browser), [6]);
    press(&mut browser, SelectionMove::extend(MoveStep::Up));
    assert_eq!(selected_indices(&browser), [5, 6]);
}

#[test]
fn column_extension_does_not_open_the_directories_it_passes() {
    let mut browser = browser_with_entries(BrowserViewMode::Columns, 0);
    let folder = PathBuf::from("/workspace/folder");
    browser.tab_mut().entries = vec![
        entry(0),
        DirectoryEntry::new(
            folder.clone(),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        ),
        entry(2),
    ]
    .into();
    browser.select_path(browser.tab().entries[0].path().to_path_buf());

    press(&mut browser, SelectionMove::extend(MoveStep::Down));

    assert_eq!(browser.tab().selected.as_deref(), Some(folder.as_path()));
    assert_eq!(selected_indices(&browser), [0, 1]);
    assert_eq!(browser.tab().deepest_open_column_directory, None);

    press(&mut browser, SelectionMove::cursor(MoveStep::Last));
    assert_eq!(selected_indices(&browser), [2]);
}

#[test]
fn icon_grid_page_keys_move_by_whole_rows_and_stop_at_the_ends() {
    let mut browser = browser_with_entries(BrowserViewMode::Icons, 20);
    browser.main_window_width = 500.0;
    browser.sidebar_width_when_shown = 0.0;
    let row_height = crate::icon_grid_geometry::row_height(browser.user_config.icons_icon_edge());
    browser.icon_grid_viewports.insert(
        crate::model::BrowserPaneId::PRIMARY,
        crate::app::PaneIconGridViewport {
            location: browser.tab().location.key(),
            viewport: crate::model::IconGridViewport {
                offset_y: 0.0,
                width: 500.0,
                height: row_height * 3.0,
            },
        },
    );
    // 500 宽是 3 列；一屏 3 行，翻页走 2 行 = 6 项，同一列。
    browser.select_path(browser.tab().entries[1].path().to_path_buf());

    press(&mut browser, SelectionMove::cursor(MoveStep::PageDown));
    assert_eq!(selected_index(&browser), Some(7));
    press(&mut browser, SelectionMove::cursor(MoveStep::PageDown));
    press(&mut browser, SelectionMove::cursor(MoveStep::PageDown));
    assert_eq!(selected_index(&browser), Some(19));
    press(&mut browser, SelectionMove::cursor(MoveStep::PageDown));
    assert_eq!(selected_index(&browser), Some(19));

    press(&mut browser, SelectionMove::extend(MoveStep::First));
    assert_eq!(selected_indices(&browser), (0..20).collect::<Vec<_>>());
}

#[test]
fn escape_clears_the_selection_only_when_nothing_else_is_open() {
    let mut browser = browser_with_entries(BrowserViewMode::List, 3);
    browser.select_path(browser.tab().entries[1].path().to_path_buf());
    browser.operation_queue.open_panel();

    drop(browser.handle_escape_shortcut());
    assert!(!browser.operation_queue.is_panel_open());
    assert_eq!(selected_index(&browser), Some(1));

    drop(browser.handle_escape_shortcut());
    assert!(browser.tab().selected.is_none());
    assert!(browser.tab().selected_paths.is_empty());

    // 选择清空后，方向键从第一项开始。
    press(&mut browser, SelectionMove::cursor(MoveStep::Down));
    assert_eq!(selected_index(&browser), Some(0));
}

#[test]
fn invert_selection_flips_the_select_all_scope() {
    let mut browser = browser_with_entries(BrowserViewMode::List, 4);
    browser.select_path(browser.tab().entries[1].path().to_path_buf());

    drop(browser.invert_selection_in_file_selection_scope());
    assert_eq!(selected_indices(&browser), [0, 2, 3]);

    drop(browser.invert_selection_in_file_selection_scope());
    assert_eq!(selected_indices(&browser), [1]);
}

fn named_entry(name: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from("/workspace").join(name),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn browser_with_names(view_mode: BrowserViewMode, names: &[&str]) -> FileBrowser {
    let mut browser = browser_with_entries(view_mode, 0);
    browser.tab_mut().entries = names
        .iter()
        .map(|name| named_entry(name, FileKind::File))
        .collect::<Vec<_>>()
        .into();
    browser
}

fn selected_name(browser: &FileBrowser) -> Option<String> {
    browser
        .tab()
        .selected
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
}

fn type_text(browser: &mut FileBrowser, typed: &str) {
    for character in typed.chars() {
        drop(browser.jump_to_typed_name(character));
    }
}

#[test]
fn typing_jumps_to_the_first_name_starting_with_it() {
    let mut browser = browser_with_names(
        BrowserViewMode::List,
        &["apple.txt", "notes.txt", "report.pdf", "readme.md"],
    );
    type_text(&mut browser, "re");
    assert_eq!(selected_name(&browser).as_deref(), Some("report.pdf"));
    type_text(&mut browser, "a");
    assert_eq!(selected_name(&browser).as_deref(), Some("readme.md"));
    assert_eq!(browser.type_ahead.typed(), "rea");

    // 没有匹配就不动，缓冲留着（可能还没打完）。
    type_text(&mut browser, "z");
    assert_eq!(selected_name(&browser).as_deref(), Some("readme.md"));
}

#[test]
fn typing_the_same_letter_again_cycles_through_its_names() {
    let mut browser = browser_with_names(
        BrowserViewMode::List,
        &["apple.txt", "notes.txt", "april.txt", "avocado.txt"],
    );
    type_text(&mut browser, "a");
    assert_eq!(selected_name(&browser).as_deref(), Some("apple.txt"));
    type_text(&mut browser, "a");
    assert_eq!(selected_name(&browser).as_deref(), Some("april.txt"));
    type_text(&mut browser, "aa");
    assert_eq!(selected_name(&browser).as_deref(), Some("apple.txt"));
}

#[test]
fn typing_pinyin_initials_finds_chinese_names() {
    let mut browser = browser_with_names(BrowserViewMode::Icons, &["文档", "下载", "音乐"]);
    type_text(&mut browser, "xz");
    assert_eq!(selected_name(&browser).as_deref(), Some("下载"));
}

#[test]
fn typing_in_columns_stays_in_the_current_column() {
    let mut browser = browser_with_entries(BrowserViewMode::Columns, 0);
    let projects = PathBuf::from("/workspace/projects");
    browser.tab_mut().entries = vec![
        named_entry("projects", FileKind::Directory),
        named_entry("zebra.txt", FileKind::File),
    ]
    .into();
    browser.tab_mut().expanded_directories.insert(
        projects.clone(),
        expanded_directory(
            vec![
                DirectoryEntry::new(
                    projects.join("alpha"),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                ),
                DirectoryEntry::new(
                    projects.join("zulu"),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                ),
            ],
            1.0,
        ),
    );
    browser.tab_mut().deepest_open_column_directory = Some(projects.clone());
    browser.select_path(projects.join("alpha"));
    browser.focused_column_directory = Some(projects.clone());

    type_text(&mut browser, "z");
    assert_eq!(browser.tab().selected, Some(projects.join("zulu")));
}

#[test]
fn escape_clears_what_was_typed_before_the_selection() {
    let mut browser = browser_with_names(BrowserViewMode::List, &["apple.txt", "notes.txt"]);
    type_text(&mut browser, "n");
    assert_eq!(selected_name(&browser).as_deref(), Some("notes.txt"));

    drop(browser.handle_escape_shortcut());
    assert_eq!(browser.type_ahead.typed(), "");
    assert_eq!(selected_name(&browser).as_deref(), Some("notes.txt"));

    drop(browser.handle_escape_shortcut());
    assert!(browser.tab().selected.is_none());
}

#[test]
fn letters_captured_by_a_text_field_never_jump() {
    let mut browser = browser_with_names(BrowserViewMode::List, &["apple.txt", "notes.txt"]);
    drop(browser.handle_keyboard_key_pressed(
        iced::keyboard::Key::Character("n".into()),
        iced::keyboard::Key::Character("n".into()),
        iced::keyboard::Modifiers::default(),
        iced::event::Status::Captured,
    ));
    assert!(browser.tab().selected.is_none());
    assert_eq!(browser.type_ahead.typed(), "");
}
