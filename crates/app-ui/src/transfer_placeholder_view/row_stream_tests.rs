// 行流几何与 hover 命中的单元测试:与实现分文件以守住单文件 800 行约束。
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::Point;

use super::list_row_layout::ListRowLayout;
use super::*;
use crate::model::{BrowserLocation, BrowserPaneId};
use crate::visible_entries::VisibleEntryStatusRow;

fn entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn placeholder(name: &str) -> TransferPlaceholder {
    TransferPlaceholder {
        name: name.to_owned(),
        is_directory: false,
        progress: None,
        total_bytes: None,
        enqueued_at: std::time::SystemTime::UNIX_EPOCH,
    }
}

/// 根层条目为 /d/a.txt、/d/z.txt 的浏览器:手排的行流按下标从这里取条目。
fn browser_with_a_and_z() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/d"));
    browser.tab_mut().entries = vec![entry("/d/a.txt"), entry("/d/z.txt")].into();
    browser
}

fn layout() -> ListRowLayout {
    let mut layout = ListRowLayout::new(0, 40.0, 0.0);
    layout.add_segment(None, 0, 1.0);
    layout
}

#[test]
fn vertical_bounds_count_placeholder_rows_above_the_target() {
    // reveal 数学与渲染同流:占位行在选中项上方时,落点必须计入。
    let browser = browser_with_a_and_z();
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let mut rows = layout();
    rows.push_entry(0, 0, 0.0, None);
    rows.push_placeholder(placeholder("incoming.txt"));
    rows.push_entry(0, 1, 0.0, None);

    assert_eq!(
        rows.vertical_bounds(pane, Path::new("/d/z.txt"), 30.0),
        Some((30.0 + 40.0 * 2.0, 40.0))
    );
}

#[test]
fn content_height_counts_placeholder_and_animated_rows() {
    let mut rows = ListRowLayout::new(0, 40.0, 0.0);
    let half_open = rows.add_segment(None, 0, 0.5);
    rows.push_entry(half_open, 0, 0.0, None);
    rows.push_placeholder(placeholder("incoming.txt"));
    rows.push_status(VisibleEntryStatusRow::Empty, 1, 40.0);

    assert_eq!(rows.content_height(), 100.0);
}

#[test]
fn path_at_point_hits_entry_rows_and_skips_chrome_rows() {
    // hover 补偿命中与渲染同口径:表头留量、占位行高度都计入累计,
    // 命中Entry 行返回路径,组头/占位/表尾空白返回 None。
    let browser = browser_with_a_and_z();
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let mut rows = layout();
    rows.push_group_header("A".to_owned(), "A".to_owned(), 2);
    rows.push_entry(0, 0, 0.0, None);
    rows.push_placeholder(placeholder("incoming.txt"));
    rows.push_entry(0, 1, 0.0, None);

    let at = |y: f32| rows.entry_path_at(pane, 30.0, Point::new(0.0, y));
    // 组头行(30..62)不是条目。
    assert_eq!(at(30.0), None);
    // 第一条目行(62..102)。
    assert_eq!(at(62.0), Some(Path::new("/d/a.txt")));
    assert_eq!(at(101.9), Some(Path::new("/d/a.txt")));
    // 占位行(102..142)不是条目。
    assert_eq!(at(120.0), None);
    // 第二条目行(142..182)。
    assert_eq!(at(160.0), Some(Path::new("/d/z.txt")));
    // 表尾空白。
    assert_eq!(at(182.0), None);
    // 表头里。
    assert_eq!(at(10.0), None);
}

#[test]
fn viewport_range_keeps_collapsed_rows_inside_the_window() {
    // 折叠到 0 高的行落在窗口里照样排出来(动画中的行要有控件状态);
    // 窗口之外的行不排,上下空白补齐总高。
    let mut rows = ListRowLayout::new(0, 40.0, 0.0);
    let open = rows.add_segment(None, 0, 1.0);
    let collapsed = rows.add_segment(Some(PathBuf::from("/d/dir")), 1, 0.0);
    rows.push_entry(open, 0, 0.0, None); // 0..40
    rows.push_entry(collapsed, 0, 0.0, None); // 40..40
    rows.push_entry(open, 1, 0.0, None); // 40..80
    rows.push_entry(open, 2, 0.0, None); // 80..120
    rows.push_entry(open, 3, 0.0, None); // 120..160

    let range = rows.range_for_viewport(0.0, 50.0, 40.0, 0);
    assert_eq!((range.start, range.end), (2, 4));
    assert_eq!((range.before_height, range.after_height), (40.0, 40.0));

    let range = rows.range_for_viewport(0.0, 30.0, 20.0, 0);
    assert_eq!((range.start, range.end), (0, 3));
    assert_eq!((range.before_height, range.after_height), (0.0, 80.0));

    // 窗口在内容之下:什么都不排,上空白就是总高。
    let range = rows.range_for_viewport(0.0, 500.0, 40.0, 0);
    assert_eq!((range.start, range.end), (5, 5));
    assert_eq!((range.before_height, range.after_height), (160.0, 0.0));
}
