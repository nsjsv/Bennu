// 根目录分组接入的单元测试:与实现分文件以守住单文件 800 行约束。
// 覆盖:组头行高计入虚拟范围/内容高/reveal 纵向界、组头断开选中连排、
// 目录(含目录占位)置顶、
// 分组关闭(None)回归保护、大小维度未知尺寸归最末兜底组。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::list_row_layout::ListRowLayout;
use super::*;
use crate::config::{self, UiLanguage};
use crate::file_entry_presentation::SelectionRunPosition;
use crate::model::{BrowserLocation, BrowserPaneId, FileGroupingMode};
use crate::transfer_placeholders::{TransferPlaceholder, TransferSortOptions};

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

fn merged_name(item: &MergedTransferItem<'_>) -> String {
    match item {
        MergedTransferItem::Entry { entry, .. } => entry.name().to_string_lossy().into_owned(),
        MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
    }
}

/// 根层条目为 /d/a.txt、/d/z.txt 的窗格,和一份行高 40 的空行流(0 号段是根层)。
fn browser_with_a_and_z() -> crate::app::FileBrowser {
    let (mut browser, _) = crate::app::FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/d"));
    browser.tab_mut().entries = vec![file_entry("/d/a.txt", 1), file_entry("/d/z.txt", 1)].into();
    browser
}

fn layout() -> ListRowLayout {
    let mut layout = ListRowLayout::new(0, 40.0);
    layout.add_segment(None, 0, 1.0);
    layout
}

fn header(rows: &mut ListRowLayout, title: &str) {
    rows.push_group_header(title.to_owned(), title.to_owned(), 1);
}

#[test]
fn group_header_height_enters_virtual_content_and_reveal_math() {
    let browser = browser_with_a_and_z();
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    // 行高 40 + 组头 32:内容高、初始窗口、视口范围、reveal 落点全部同口径。
    let mut rows = layout();
    header(&mut rows, "A");
    rows.push_entry(0, 0, 0.0, None);
    header(&mut rows, "B");
    rows.push_entry(0, 1, 0.0, None);

    assert_eq!(rows.content_height(), 144.0);

    let initial = rows.initial_range(2);
    assert_eq!(initial.start, 0);
    assert_eq!(initial.end, 2);
    assert_eq!(initial.before_height, 0.0);
    assert_eq!(initial.after_height, 144.0 - 72.0);

    let viewport = rows.range_for_viewport(30.0, 0.0, 100.0, 0);
    assert_eq!(viewport.start, 0);
    assert_eq!(viewport.end, 2);
    assert_eq!(viewport.before_height, 0.0);
    assert_eq!(viewport.after_height, 72.0);

    // reveal 数学:第二个组头(32)与首条目行(40)都计入 z.txt 之前的偏移。
    assert_eq!(
        rows.vertical_bounds(pane, Path::new("/d/z.txt"), 30.0),
        Some((30.0 + 32.0 + 40.0 + 32.0, 40.0))
    );
}

#[test]
fn selection_run_breaks_at_group_header_rows() {
    let browser = browser_with_a_and_z();
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let mut rows = layout();
    rows.push_entry(0, 0, 0.0, None);
    header(&mut rows, "A");
    rows.push_entry(0, 1, 0.0, None);
    let selected: HashSet<PathBuf> = ["/d/a.txt", "/d/z.txt"]
        .map(PathBuf::from)
        .into_iter()
        .collect();

    // 组头行自身不是条目,不参与连排;两侧条目被它断开成独立单选连排。
    assert_eq!(rows.selection_run_position(pane, 1, &selected), None);
    assert_eq!(
        rows.selection_run_position(pane, 0, &selected),
        Some(SelectionRunPosition::Single)
    );
    assert_eq!(
        rows.selection_run_position(pane, 2, &selected),
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
        .enumerate()
        .map(|(index, entry)| MergedTransferItem::Entry {
            entry,
            index,
            transfer: None,
        })
        .collect();

    // 划分结果借用划分器与文件段,先绑定再分区避免临时值提前释放。
    let ascending_grouping = grouping(FileGroupingMode::NameInitial, SortDirection::Ascending);
    let ascending = ascending_grouping.partition(&files);
    let ascending_titles: Vec<&str> = ascending
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(ascending_titles, ["#", "A"]);
    let ascending_counts: Vec<usize> = ascending
        .iter()
        .map(|section| section.items.len())
        .collect();
    assert_eq!(ascending_counts, [1, 2]);

    // 降序只反转组序列,组内保持输入序(沿用现有排序)。
    let descending_grouping = grouping(FileGroupingMode::NameInitial, SortDirection::Descending);
    let descending = descending_grouping.partition(&files);
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
            index: 0,
            transfer: None,
        },
        MergedTransferItem::Entry {
            entry: &entries[1],
            index: 1,
            transfer: None,
        },
        MergedTransferItem::Entry {
            entry: &entries[2],
            index: 2,
            transfer: None,
        },
        MergedTransferItem::Placeholder(placeholder),
    ];

    let size_grouping = grouping(FileGroupingMode::Size, SortDirection::Ascending);
    let sections = size_grouping.partition(&files);

    // 已知尺寸 100/300/700 各成一组;未知尺寸占位归最末兜底组。
    assert_eq!(sections.len(), 3);
    assert_eq!(
        sections.iter().map(|s| s.items.len()).collect::<Vec<_>>(),
        [1, 1, 2]
    );
    assert_eq!(merged_name(sections[2].items[1]), "incoming.bin");
}

/// 分组开关来自用户设置：按给定模式建一个浏览器。
fn browser_with_grouping(mode: FileGroupingMode) -> crate::app::FileBrowser {
    let mut config = config::default_user_config();
    config.file_grouping = mode;
    crate::app::FileBrowser::new(config).0
}

#[test]
fn grouping_applies_only_when_enabled() {
    // 展开的子目录平铺由行流构建器保证(见 stripe_index_tests),这里只管开关。
    let browser = browser_with_grouping(FileGroupingMode::NameInitial);
    let folder = BrowserLocation::Directory(PathBuf::from("/workspace"));
    assert!(root_grouping::RootGrouping::for_pane_root(&browser, &folder).is_some());

    // 分组关闭(默认 None):根层也不划分,行流与既有平铺一致。
    let browser = browser_with_grouping(FileGroupingMode::None);
    assert!(root_grouping::RootGrouping::for_pane_root(&browser, &folder).is_none());
}

#[test]
fn rail_entries_derive_from_row_stream_with_render_height_accumulation() {
    // 索引定位与渲染高度累计同口径:组头顶点偏移 = 之前所有行高之和,
    // 末组顶点 + 组头自身高 + 末行 = 内容总高。
    let mut rows = layout();
    header(&mut rows, "A");
    rows.push_entry(0, 0, 0.0, None);
    header(&mut rows, "Z");
    rows.push_entry(0, 1, 0.0, None);

    let rail = rows.rail_entries();
    assert_eq!(rail.len(), 2);
    assert_eq!(rail[0].index_label, "A");
    assert_eq!(rail[0].top_offset, 0.0);
    assert_eq!(rail[1].index_label, "Z");
    assert_eq!(rail[1].top_offset, 32.0 + 40.0);
    assert_eq!(
        rail.last().unwrap().top_offset + 32.0 + 40.0,
        rows.content_height()
    );

    // 门控:两组才显示索引栏。
    assert!(crate::model::file_group_rail_visible(rail));
}

#[test]
fn rail_entries_stay_empty_without_group_header_rows() {
    // 分组关闭(None)/展开子目录平铺时行流没有组头行,索引栏无数据,
    // 渲染层按可见门控直接不挂叠层。
    let mut rows = layout();
    rows.push_entry(0, 0, 0.0, None);
    rows.push_entry(0, 1, 0.0, None);
    assert!(rows.rail_entries().is_empty());
    assert!(!crate::model::file_group_rail_visible(rows.rail_entries()));
}
