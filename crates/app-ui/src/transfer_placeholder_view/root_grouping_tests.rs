// 根目录分组接入的单元测试:与实现分文件以守住单文件 800 行约束。
// 覆盖:组头行高计入虚拟范围/内容高/reveal 纵向界、组头断开选中连排、
// 目录(含目录占位)置顶、非根目录(展开子目录语义)拿不到划分器、
// 分组关闭(None)回归保护、大小维度未知尺寸归最末兜底组。
use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::*;
use crate::config::{self, UiLanguage};
use crate::file_entry_presentation::SelectionRunPosition;
use crate::model::{BrowserPaneId, FileGroupingMode};
use crate::transfer_placeholders::{TransferSortOptions, TransferPlaceholder};

fn file_entry(path: &str, len: u64) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata {
            len,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn directory_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn file_placeholder(name: &str) -> TransferPlaceholder {
    TransferPlaceholder {
        name: name.to_owned(),
        is_directory: false,
        progress: None,
        total_bytes: None,
        enqueued_at: SystemTime::UNIX_EPOCH,
    }
}

fn directory_placeholder(name: &str) -> TransferPlaceholder {
    TransferPlaceholder {
        is_directory: true,
        ..file_placeholder(name)
    }
}

fn grouping(mode: FileGroupingMode, direction: SortDirection) -> root_grouping::RootGrouping {
    root_grouping::RootGrouping {
        mode,
        direction,
        language: UiLanguage::English,
        now: SystemTime::UNIX_EPOCH,
    }
}

/// 划分需要一个 pane 解析显示同源元数据;测试条目无 discovery 索引,
/// metadata_for_entry 回退条目自带元数据,len 键因此确定。
fn browser_with_grouping(mode: FileGroupingMode) -> crate::app::FileBrowser {
    let mut config = config::default_user_config();
    config.file_grouping = mode;
    crate::app::FileBrowser::new(config).0
}

fn merged_name(item: &MergedTransferItem<'_>) -> String {
    match item {
        MergedTransferItem::Entry { entry, .. } => {
            entry.name().to_string_lossy().into_owned()
        }
        MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
    }
}

fn entry_row(entry: &DirectoryEntry) -> ListTransferRow<'_> {
    entry_row_with_stripe(entry, 0)
}

fn entry_row_with_stripe(entry: &DirectoryEntry, stripe_index: usize) -> ListTransferRow<'_> {
    ListTransferRow::Entry {
        visible: VisibleEntry {
            entry,
            depth: 0,
            animation_progress: 1.0,
        },
        transfer: None,
        stripe_index,
    }
}

fn group_header(title: &str, count: usize) -> ListTransferRow<'static> {
    ListTransferRow::GroupHeader {
        title: title.to_owned(),
        index_label: title.to_owned(),
        count,
        height: crate::list_view::LIST_GROUP_HEADER_HEIGHT,
    }
}

#[test]
fn group_header_height_enters_virtual_content_and_reveal_math() {
    let entries = [file_entry("/d/a.txt", 1), file_entry("/d/z.txt", 1)];
    // 行高 40 + 组头 32:内容高、初始窗口、视口范围、reveal 落点全部同口径。
    let rows = [
        group_header("A", 1),
        entry_row(&entries[0]),
        group_header("B", 1),
        entry_row(&entries[1]),
    ];

    assert_eq!(list_transfer_rows_content_height(&rows, 40.0), 144.0);

    let initial = initial_list_transfer_rows_range(&rows, 40.0, 2);
    assert_eq!(initial.start, 0);
    assert_eq!(initial.end, 2);
    assert_eq!(initial.before_height, 0.0);
    assert_eq!(initial.after_height, 144.0 - 72.0);

    let viewport = list_transfer_rows_range_for_viewport(&rows, 40.0, 30.0, 0.0, 100.0, 0);
    assert_eq!(viewport.start, 0);
    assert_eq!(viewport.end, 2);
    assert_eq!(viewport.before_height, 0.0);
    assert_eq!(viewport.after_height, 72.0);

    // reveal 数学:第二个组头(32)与首条目行(40)都计入 z.txt 之前的偏移。
    assert_eq!(
        list_transfer_rows_vertical_bounds(&rows, Path::new("/d/z.txt"), 40.0, 30.0),
        Some((30.0 + 32.0 + 40.0 + 32.0, 40.0))
    );
}

#[test]
fn selection_run_breaks_at_group_header_rows() {
    let entries = [file_entry("/d/a.txt", 1), file_entry("/d/z.txt", 1)];
    let rows = [
        entry_row(&entries[0]),
        group_header("A", 1),
        entry_row(&entries[1]),
    ];
    let selected: HashSet<PathBuf> = ["/d/a.txt", "/d/z.txt"]
        .map(PathBuf::from)
        .into_iter()
        .collect();

    // 组头行自身不是条目,不参与连排;两侧条目被它断开成独立单选连排。
    assert_eq!(
        selection_run_position_in_transfer_rows(&rows, 1, &selected),
        None
    );
    assert_eq!(
        selection_run_position_in_transfer_rows(&rows, 0, &selected),
        Some(SelectionRunPosition::Single)
    );
    assert_eq!(
        selection_run_position_in_transfer_rows(&rows, 2, &selected),
        Some(SelectionRunPosition::Single)
    );
}

#[test]
fn directories_and_directory_placeholders_lead_grouped_sections() {
    // 合并流按名字升序且未开 directories_first:目录实体/目录占位与文件
    // 交错;划分后两段稳定前置,文件段保持原相对顺序。
    // merge 契约要求条目按既有排序传入,这里按名字升序给入。
    let entries = [
        directory_entry("/d/alpha-dir"),
        file_entry("/d/mid.txt", 1),
        file_entry("/d/zeta.txt", 1),
    ];
    let placeholders = [
        file_placeholder("zeta-incoming"),
        directory_placeholder("alpha-incoming-dir"),
    ];
    let merged = merge_entries_with_placeholders(
        &entries,
        &placeholders,
        TransferSortOptions {
            field: SortField::Name,
            direction: SortDirection::Ascending,
            directories_first: false,
        },
    );

    let (directories, files) = root_grouping::split_directories_first(merged);
    let directory_names: Vec<String> = directories.iter().map(merged_name).collect();
    assert_eq!(directory_names, ["alpha-dir", "alpha-incoming-dir"]);
    let file_names: Vec<String> = files.iter().map(merged_name).collect();
    assert_eq!(file_names, ["mid.txt", "zeta-incoming", "zeta.txt"]);
}

#[test]
fn partition_emits_header_counts_and_reverses_sections_on_descending() {
    let entries = [
        file_entry("/d/apple.txt", 1),
        file_entry("/d/1file", 1),
        file_entry("/d/avocado.md", 1),
    ];
    let files: Vec<MergedTransferItem<'_>> = entries
        .iter()
        .map(|entry| MergedTransferItem::Entry {
            entry,
            transfer: None,
        })
        .collect();

    let browser = browser_with_grouping(FileGroupingMode::NameInitial);
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    // 划分结果借用划分器与文件段,先绑定再分区避免临时值提前释放。
    let ascending_grouping = grouping(FileGroupingMode::NameInitial, SortDirection::Ascending);
    let ascending =
        ascending_grouping.partition(&files, |entry| pane.metadata_for_entry(entry));
    let ascending_titles: Vec<&str> = ascending
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(ascending_titles, ["#", "A"]);
    let ascending_counts: Vec<usize> =
        ascending.iter().map(|section| section.items.len()).collect();
    assert_eq!(ascending_counts, [1, 2]);

    // 降序只反转组序列,组内保持输入序(沿用现有排序)。
    let descending_grouping = grouping(FileGroupingMode::NameInitial, SortDirection::Descending);
    let descending =
        descending_grouping.partition(&files, |entry| pane.metadata_for_entry(entry));
    let descending_titles: Vec<&str> = descending
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(descending_titles, ["A", "#"]);
    assert_eq!(merged_name(descending[0].items[0]), "apple.txt");
    assert_eq!(merged_name(descending[0].items[1]), "avocado.md");
}

#[test]
fn size_grouping_sends_unknown_placeholder_size_to_last_bucket() {
    let entries = [
        file_entry("/d/a.bin", 100),
        file_entry("/d/b.bin", 300),
        file_entry("/d/c.bin", 700),
    ];
    let placeholder = file_placeholder("incoming.bin");
    let files = vec![
        MergedTransferItem::Entry {
            entry: &entries[0],
            transfer: None,
        },
        MergedTransferItem::Entry {
            entry: &entries[1],
            transfer: None,
        },
        MergedTransferItem::Entry {
            entry: &entries[2],
            transfer: None,
        },
        MergedTransferItem::Placeholder(placeholder),
    ];

    let browser = browser_with_grouping(FileGroupingMode::Size);
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let size_grouping = grouping(FileGroupingMode::Size, SortDirection::Ascending);
    let sections = size_grouping.partition(&files, |entry| pane.metadata_for_entry(entry));

    // 已知尺寸 100/300/700 各成一组;未知尺寸占位归最末兜底组。
    assert_eq!(sections.len(), 3);
    assert_eq!(sections.iter().map(|s| s.items.len()).collect::<Vec<_>>(), [1, 1, 2]);
    assert_eq!(merged_name(sections[2].items[1]), "incoming.bin");
}

#[test]
fn grouping_applies_only_to_pane_root_directory_and_only_when_enabled() {
    // 分组开启:当前目录拿到划分器;展开子目录(路径≠current_dir)拿不到,
    // 递归摊平走平铺分支——展开内容不出组头的边界保护。
    let browser = browser_with_grouping(FileGroupingMode::NameInitial);
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    assert!(
        root_grouping::RootGrouping::for_pane_root(&browser, pane, pane.current_dir.as_path())
            .is_some()
    );
    assert!(
        root_grouping::RootGrouping::for_pane_root(&browser, pane, Path::new("/some/child/dir"))
            .is_none()
    );

    // 分组关闭(默认 None):即使当前目录也不划分,行流与既有平铺一致。
    let browser = browser_with_grouping(FileGroupingMode::None);
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    assert!(
        root_grouping::RootGrouping::for_pane_root(&browser, pane, pane.current_dir.as_path())
            .is_none()
    );
}

#[test]
fn rail_entries_derive_from_row_stream_with_render_height_accumulation() {
    // 索引定位与渲染高度累计同口径:组头顶点偏移 = 之前所有行高之和,
    // 末组顶点 + 组头自身高 + 末行 = 内容总高。
    let entries = [file_entry("/d/a.txt", 1), file_entry("/d/z.txt", 1)];
    let rows = [
        group_header("A", 1),
        entry_row(&entries[0]),
        group_header("Z", 1),
        entry_row(&entries[1]),
    ];

    let rail = list_file_group_rail_entries(&rows, 40.0);
    assert_eq!(rail.len(), 2);
    assert_eq!(rail[0].index_label, "A");
    assert_eq!(rail[0].top_offset, 0.0);
    assert_eq!(rail[1].index_label, "Z");
    assert_eq!(rail[1].top_offset, 32.0 + 40.0);
    let content_height = list_transfer_rows_content_height(&rows, 40.0);
    assert_eq!(
        rail.last().unwrap().top_offset + 32.0 + 40.0,
        content_height
    );

    // 门控:两组才显示索引栏。
    assert!(crate::model::file_group_rail_visible(&rail));
}

#[test]
fn rail_entries_stay_empty_without_group_header_rows() {
    // 分组关闭(None)/展开子目录平铺时行流没有组头行,索引栏无数据,
    // 渲染层按可见门控直接不挂叠层。
    let entries = [file_entry("/d/a.txt", 1), file_entry("/d/z.txt", 1)];
    let rows = [entry_row(&entries[0]), entry_row(&entries[1])];
    let rail = list_file_group_rail_entries(&rows, 40.0);
    assert!(rail.is_empty());
    assert!(!crate::model::file_group_rail_visible(&rail));
}
