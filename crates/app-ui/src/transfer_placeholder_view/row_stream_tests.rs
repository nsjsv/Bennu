// 行流几何与 hover 命中的单元测试:与实现分文件以守住单文件 800 行约束。
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::Point;

use super::*;

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

fn placeholder_row(name: &str) -> ListTransferRow<'_> {
    ListTransferRow::Placeholder {
        placeholder: TransferPlaceholder {
            name: name.to_owned(),
            is_directory: false,
            progress: None,
            total_bytes: None,
            enqueued_at: std::time::SystemTime::UNIX_EPOCH,
        },
        stripe_index: 0,
    }
}

#[test]
fn vertical_bounds_count_placeholder_rows_above_the_target() {
    // reveal 数学与渲染同流:占位行在选中项上方时,落点必须计入。
    let entries = [entry("/d/a.txt"), entry("/d/z.txt")];
    let rows = [
        ListTransferRow::Entry {
            visible: VisibleEntry {
                entry: &entries[0],
                depth: 0,
                animation_progress: 1.0,
            },
            stripe_index: 0,
        },
        placeholder_row("incoming.txt"),
        ListTransferRow::Entry {
            visible: VisibleEntry {
                entry: &entries[1],
                depth: 0,
                animation_progress: 1.0,
            },
            stripe_index: 1,
        },
    ];
    assert_eq!(
        list_transfer_rows_vertical_bounds(&rows, Path::new("/d/z.txt"), 40.0, 30.0),
        Some((30.0 + 40.0 * 2.0, 40.0))
    );
}

#[test]
fn content_height_counts_placeholder_and_animated_rows() {
    let entry = entry("/d/a.txt");
    let rows = [
        ListTransferRow::Entry {
            visible: VisibleEntry {
                entry: &entry,
                depth: 0,
                animation_progress: 0.5,
            },
            stripe_index: 0,
        },
        placeholder_row("incoming.txt"),
        ListTransferRow::DirectoryStatusRow {
            message: "No items",
            depth: 1,
            height: 40.0,
            stripe_index: 1,
        },
    ];
    assert_eq!(list_transfer_rows_content_height(&rows, 40.0), 100.0);
}

#[test]
fn path_at_point_hits_entry_rows_and_skips_chrome_rows() {
    // hover 补偿命中与渲染同口径:表头留量、占位行高度都计入累计,
    // 命中Entry 行返回路径,组头/占位/表尾空白返回 None。
    let entries = [entry("/d/a.txt"), entry("/d/z.txt")];
    let rows = [
        ListTransferRow::GroupHeader {
            title: "A".to_owned(),
            index_label: "A".to_owned(),
            count: 2,
            height: 32.0,
        },
        ListTransferRow::Entry {
            visible: VisibleEntry {
                entry: &entries[0],
                depth: 0,
                animation_progress: 1.0,
            },
            stripe_index: 0,
        },
        placeholder_row("incoming.txt"),
        ListTransferRow::Entry {
            visible: VisibleEntry {
                entry: &entries[1],
                depth: 0,
                animation_progress: 1.0,
            },
            stripe_index: 1,
        },
    ];
    let at = |y: f32| list_entry_path_at_point(&rows, 40.0, 30.0, Point::new(0.0, y));
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
}
