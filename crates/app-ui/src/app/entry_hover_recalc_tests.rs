// hover 滚动补偿的单元测试:hovered_entry 恒等于光标正下方的条目。
// 覆盖:滚动后 hover 跟随光标所在行、落点在组头/表尾空白 → None、
// 无光标记录(光标不在视口内)不动作、索引栏进入清空且屏障存续期间
// 滚动补偿不命中、大图网格同构。
use std::path::PathBuf;
use std::sync::Arc;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::{Point, Rectangle, Size};

use super::super::FileBrowser;
use crate::config;
use crate::icon_grid_geometry::{
    column_count_for_width, grid_gap, row_height, tile_visual_height, tile_width,
    ICON_GRID_CONTENT_PADDING,
};
use crate::list_view::{ListGeometry, LIST_HEADER_HEIGHT, LIST_ROW_HEIGHT};
use crate::model::{BrowserPaneId, BrowserViewMode, FileGroupingMode, Message};

const PANE_ID: BrowserPaneId = BrowserPaneId::PRIMARY;

fn list_viewport() -> Rectangle {
    Rectangle::new(Point::new(200.0, 120.0), Size::new(600.0, 500.0))
}

fn file_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn list_browser(paths: &[&str]) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(paths.iter().map(|path| file_entry(path)).collect());
    browser.view_mode = BrowserViewMode::List;
    browser
}

/// 列表内容坐标(含表头留量)对应的窗口光标位置:行流 y + 视口原点。
fn cursor_at_content_y(content_y: f32) -> Point {
    let viewport = list_viewport();
    Point::new(viewport.x + 100.0, viewport.y + content_y)
}

/// 网格内容坐标 → 窗口光标位置。
fn cursor_at(viewport: Rectangle, x: f32, y: f32) -> Point {
    Point::new(viewport.x + x, viewport.y + y)
}

#[test]
fn list_scroll_moves_hover_to_the_entry_under_the_cursor() {
    let mut browser = list_browser(&["/workspace/a.txt", "/workspace/b.txt", "/workspace/c.txt"]);
    // 光标静止在第 2 行(b.txt)中部:滚动不产生指针事件,补偿必须
    // 把高亮带过来。
    browser.cursor_position = cursor_at_content_y(LIST_HEADER_HEIGHT + 1.5 * LIST_ROW_HEIGHT);

    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));

    assert_eq!(
        browser.hovered_entry,
        Some(PathBuf::from("/workspace/b.txt"))
    );
}

#[test]
fn list_scroll_over_header_or_trailing_blank_clears_hover() {
    let mut browser = list_browser(&["/workspace/a.txt", "/workspace/b.txt"]);
    browser.hovered_entry = Some(PathBuf::from("/workspace/a.txt"));
    // 表头行上:不是条目。
    browser.cursor_position = cursor_at_content_y(LIST_HEADER_HEIGHT / 2.0);
    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));
    assert_eq!(browser.hovered_entry, None);

    // 表尾空白(内容总高之外):同样无处可指。
    let content_height = LIST_HEADER_HEIGHT + 2.0 * LIST_ROW_HEIGHT;
    browser.hovered_entry = Some(PathBuf::from("/workspace/a.txt"));
    browser.cursor_position = cursor_at_content_y(content_height + LIST_ROW_HEIGHT);
    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));
    assert_eq!(browser.hovered_entry, None);
}

#[test]
fn list_scroll_over_group_header_clears_hover() {
    let mut browser = list_browser(&["/workspace/apple.txt", "/workspace/zebra.txt"]);
    browser.user_config.file_grouping = FileGroupingMode::NameInitial;
    // 行流:[组头A, apple, 组头Z, zebra];光标压在组头 Z 上。
    let row_height = ListGeometry::for_level(browser.user_config().list_view_density).row_height;
    browser.hovered_entry = Some(PathBuf::from("/workspace/apple.txt"));
    browser.cursor_position = cursor_at_content_y(
        LIST_HEADER_HEIGHT + row_height + crate::list_view::LIST_GROUP_HEADER_HEIGHT + 10.0,
    );

    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));

    assert_eq!(browser.hovered_entry, None);
}

#[test]
fn list_scroll_without_cursor_record_keeps_event_channel_hover() {
    let mut browser = list_browser(&["/workspace/a.txt", "/workspace/b.txt"]);
    browser.hovered_entry = Some(PathBuf::from("/workspace/a.txt"));
    // 光标在视口之外(陈旧初值或在别的窗格):等值守卫语义,不动作。
    browser.cursor_position = Point::new(10.0, 10.0);

    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));

    assert_eq!(
        browser.hovered_entry,
        Some(PathBuf::from("/workspace/a.txt"))
    );
}

#[test]
fn grouping_rail_enter_clears_hover_and_blocks_scroll_recalc() {
    let mut browser = list_browser(&["/workspace/a.txt", "/workspace/b.txt"]);
    // 分组开启且组数 ≥2,索引栏真实可见(生产中屏障只在该状态下入账)。
    browser.user_config.file_grouping = FileGroupingMode::NameInitial;
    browser.hovered_entry = Some(PathBuf::from("/workspace/a.txt"));
    // 行流:[组头A, a, 组头B, b];光标压在 a 行上。
    browser.cursor_position = cursor_at_content_y(
        LIST_HEADER_HEIGHT + crate::list_view::LIST_GROUP_HEADER_HEIGHT + 0.5 * LIST_ROW_HEIGHT,
    );

    // 进入索引栏:栏上无条目可指,显式清空。
    drop(browser.update(Message::FileGroupingRailCursorEntered(PANE_ID)));
    assert_eq!(browser.hovered_entry, None);

    // 屏障存续期间滚动:补偿不得越过屏障命中栏下层的行。
    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));
    assert_eq!(browser.hovered_entry, None);

    // 离开索引栏:屏障解除,滚动补偿恢复工作。
    drop(browser.update(Message::FileGroupingRailCursorExited(PANE_ID)));
    drop(browser.update(Message::ListScrolled(PANE_ID, 0.0, list_viewport())));
    assert_eq!(
        browser.hovered_entry,
        Some(PathBuf::from("/workspace/a.txt"))
    );
}

#[test]
fn icon_grid_scroll_moves_hover_to_the_cell_under_the_cursor() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(vec![
        file_entry("/workspace/a.txt"),
        file_entry("/workspace/b.txt"),
        file_entry("/workspace/c.txt"),
    ]);
    browser.view_mode = BrowserViewMode::Icons;
    let viewport = Rectangle::new(Point::new(200.0, 120.0), Size::new(600.0, 500.0));
    assert!(column_count_for_width(viewport.width, 96) >= 2);
    let edge = browser.user_config.icons_icon_edge();
    // 第 2 格(row 0, col 1)的中心:列位与瓦片几何全部取自共享几何源。
    browser.cursor_position = cursor_at(
        viewport,
        ICON_GRID_CONTENT_PADDING + tile_width(edge) + grid_gap(edge) + tile_width(edge) / 2.0,
        ICON_GRID_CONTENT_PADDING + tile_visual_height(edge) / 2.0,
    );
    browser.hovered_entry = Some(PathBuf::from("/workspace/a.txt"));

    drop(browser.update(Message::IconGridScrolled(
        PANE_ID,
        0.0,
        viewport,
    )));

    assert_eq!(
        browser.hovered_entry,
        Some(PathBuf::from("/workspace/b.txt"))
    );

    // 滚回内容内边距上(没有格子):hover 清空。
    browser.cursor_position = cursor_at(viewport, 4.0, 4.0);
    drop(browser.update(Message::IconGridScrolled(
        PANE_ID,
        0.0,
        viewport,
    )));
    assert_eq!(browser.hovered_entry, None);
}

#[test]
fn icon_grid_scroll_over_group_header_clears_hover() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(vec![
        file_entry("/workspace/apple.txt"),
        file_entry("/workspace/zebra.txt"),
    ]);
    browser.view_mode = BrowserViewMode::Icons;
    browser.user_config.file_grouping = FileGroupingMode::NameInitial;
    let viewport = Rectangle::new(Point::new(200.0, 120.0), Size::new(600.0, 500.0));
    let edge = browser.user_config.icons_icon_edge();
    browser.hovered_entry = Some(PathBuf::from("/workspace/apple.txt"));
    // flow:[组头A, A行段, 组头Z, Z行段];光标压在组头 A 上。
    browser.cursor_position = cursor_at(
        viewport,
        ICON_GRID_CONTENT_PADDING + tile_width(edge) / 2.0,
        ICON_GRID_CONTENT_PADDING
            + crate::icon_grid_layout::ICON_GRID_GROUP_HEADER_HEIGHT / 2.0,
    );

    drop(browser.update(Message::IconGridScrolled(
        PANE_ID,
        0.0,
        viewport,
    )));

    assert_eq!(browser.hovered_entry, None);
}

#[test]
fn scrolled_grid_hover_tracks_scroll_offset_from_message() {
    // 同一光标位置,不同滚动偏移落在不同格子上:补偿用消息里的
    // offset_y 换算内容落点,而不是视口内的固定行。
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    // 条目足够多:窗格宽推导的列数下,row 1 / row 2 都真实存在。
    browser.entries = Arc::new((0..36)
        .map(|index| file_entry(&format!("/workspace/item-{index:02}.txt")))
        .collect());
    browser.view_mode = BrowserViewMode::Icons;
    let viewport = Rectangle::new(Point::new(200.0, 120.0), Size::new(600.0, 500.0));
    let edge = browser.user_config.icons_icon_edge();
    // 布局的列数由窗格内容宽推导(icon_grid_viewport_for 会把测量宽
    // 钉回窗格宽),测试必须用同一来源,不能拿消息里的视口宽自算。
    let columns = column_count_for_width(browser.pane_content_width_for(PANE_ID), edge);
    // 光标停在 row 1(col 0)格中心;滚动一行后同一光标下是 row 2。
    let pitch = row_height(edge);
    browser.cursor_position = cursor_at(
        viewport,
        ICON_GRID_CONTENT_PADDING + tile_width(edge) / 2.0,
        ICON_GRID_CONTENT_PADDING + pitch + tile_visual_height(edge) / 2.0,
    );

    drop(browser.update(Message::IconGridScrolled(PANE_ID, 0.0, viewport)));
    let before = browser.hovered_entry.clone().expect("hover on row 1");

    drop(browser.update(Message::IconGridScrolled(
        PANE_ID,
        pitch,
        viewport,
    )));
    let after = browser.hovered_entry.clone().expect("hover on row 2");

    let index_of = |path: &PathBuf| {
        browser
            .entries
            .iter()
            .position(|entry| &entry.path == path)
            .expect("hovered path is in the directory")
    };
    assert_eq!(index_of(&before) / columns, 1);
    assert_eq!(index_of(&after) / columns, 2);
}
