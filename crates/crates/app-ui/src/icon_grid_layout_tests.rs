use std::path::PathBuf;

use super::*;
use crate::icon_grid_geometry::visible_entry_range;
use crate::model::{
    BrowserPaneId, ExpandedDirectory, IconGridExpansionAnchor, IconGridExpansionContext,
    IconGridExpansionSessionId,
};
use crate::transfer_placeholder_view::root_grouping;
use file_core::{EntryMetadata, FileKind};
use tokio_util::sync::CancellationToken;

pub(super) fn entry(path: impl Into<PathBuf>, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        path.into(),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

impl<'a> IconGridLayout<'a> {
    /// 无占位、未分组的布局构造:测试夹具统一入口(生产路径统一走
    /// with_root_transfer_placeholders,空占位且分组关闭时行为一致)。
    pub(crate) fn new(
        root_directory: &'a Path,
        root_entries: &'a [DirectoryEntry],
        viewport_width: f32,
        height_bound: f32,
        icon_edge: u32,
        expansion: Option<&'a IconGridExpansionState>,
    ) -> Self {
        Self::with_root_transfer_placeholders(
            root_directory,
            root_entries,
            &[],
            TransferSortOptions {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
                directories_first: true,
            },
            &ExpandedTransferPlaceholderIndex::new(),
            viewport_width,
            height_bound,
            icon_edge,
            expansion,
            None,
        )
    }

    /// 分组开启的布局构造:按名字首字母划分根文件段(测试统一入口)。
    pub(crate) fn new_grouped_by_name_initial(
        root_directory: &'a Path,
        root_entries: &'a [DirectoryEntry],
        root_placeholders: &[TransferPlaceholder],
        direction: file_core::SortDirection,
        viewport_width: f32,
        height_bound: f32,
        icon_edge: u32,
        expansion: Option<&'a IconGridExpansionState>,
    ) -> Self {
        // 大小/日期维度才依赖元数据;名字维度用缺省元数据即可,划分键确定。
        let metadata_for_entry = |_entry: &DirectoryEntry| EntryMetadata::default();
        let grouping = root_grouping::RootGrouping {
            mode: crate::model::FileGroupingMode::NameInitial,
            direction,
            language: crate::config::UiLanguage::English,
            now: std::time::SystemTime::UNIX_EPOCH,
        };
        Self::with_root_transfer_placeholders(
            root_directory,
            root_entries,
            root_placeholders,
            TransferSortOptions {
                field: file_core::SortField::Name,
                direction,
                directories_first: true,
            },
            &ExpandedTransferPlaceholderIndex::new(),
            viewport_width,
            height_bound,
            icon_edge,
            expansion,
            Some(crate::icon_grid_layout::IconGridRootGrouping {
                grouping: &grouping,
                metadata_for_entry: &metadata_for_entry,
            }),
        )
    }
}

pub(super) fn files(directory: &str, count: usize) -> Vec<DirectoryEntry> {
    (0..count)
        .map(|index| entry(format!("{directory}/item-{index:03}"), FileKind::File))
        .collect()
}

pub(super) fn loaded(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 0,
        load_context: None,
        load_cancel: Some(CancellationToken::new()),
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

pub(super) fn anchor(parent: &str, path: &str, index: usize) -> IconGridExpansionAnchor {
    IconGridExpansionAnchor {
        parent_directory: PathBuf::from(parent),
        path: PathBuf::from(path),
        index,
    }
}

pub(super) fn expansion(root_entries: Vec<DirectoryEntry>) -> IconGridExpansionState {
    IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/root", 0),
        loaded(root_entries),
    )
}

pub(super) fn first_band<'a>(layout: &'a IconGridLayout<'a>) -> &'a IconGridBandLayout<'a> {
    layout
        .root()
        .flow
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::Band(band) => Some(band),
            IconGridFlowSegment::Rows(_) | IconGridFlowSegment::GroupHeader(_) => None,
        })
        .expect("layout should include expansion band")
}

#[test]
fn flat_layout_matches_existing_virtual_range() {
    let entries = files("/workspace", 44);
    let viewport = IconGridViewport {
        offset_y: ICON_GRID_CONTENT_PADDING + row_height(96) * 10.0,
        width: 500.0,
        height: row_height(96) * 2.0,
    };
    let old_range = visible_entry_range(viewport, entries.len(), 800.0, 96);
    let layout = IconGridLayout::new(Path::new("/workspace"), &entries, 500.0, 800.0, 96, None);
    let visible = layout.visible_entries(viewport);

    assert_eq!(layout.root().flow.len(), 1);
    assert_eq!(visible.len(), old_range.end_entry - old_range.start_entry);
    assert_eq!(
        visible.first().unwrap().entry.path,
        entries[old_range.start_entry].path
    );
    assert_eq!(
        visible.last().unwrap().entry.path,
        entries[old_range.end_entry - 1].path
    );
    assert_eq!(
        layout.total_height(),
        ICON_GRID_CONTENT_PADDING * 2.0
            + row_count_for_entries(entries.len(), 3) as f32 * row_height(96)
    );
}

#[test]
fn nested_panel_keeps_the_full_width_column_count() {
    let root_entries = vec![
        entry("/workspace/root/child", FileKind::Directory),
        entry("/workspace/root/file-a", FileKind::File),
        entry("/workspace/root/file-b", FileKind::File),
    ];
    let state = expansion(root_entries);
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        440.0,
        800.0,
        96,
        Some(&state),
    );
    let band = first_band(&layout);
    let nested_columns = band
        .panel
        .flow
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::Rows(rows) => Some(rows.column_count),
            IconGridFlowSegment::Band(_) | IconGridFlowSegment::GroupHeader(_) => None,
        })
        .unwrap();

    assert_eq!(nested_columns, column_count_for_width(440.0, 96));
    let matching = layout.interactive_paths_matching(&[
        PathBuf::from("/workspace/root"),
        PathBuf::from("/workspace/root/child"),
        PathBuf::from("/workspace/missing"),
    ]);
    assert_eq!(matching.len(), 2);
    assert!(matching.contains(Path::new("/workspace/root")));
    assert!(matching.contains(Path::new("/workspace/root/child")));
}

#[test]
fn opening_band_does_not_schedule_thumbnail_entries() {
    let child = entry("/workspace/root/child.png", FileKind::File);
    let mut opening = loaded(vec![child.clone()]);
    opening.animation_progress = 0.5;
    let state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/root", 0),
        opening,
    );
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let visible = layout.visible_entries(IconGridViewport {
        offset_y: 0.0,
        width: 500.0,
        height: 800.0,
    });

    assert!(visible
        .iter()
        .all(|visible| visible.entry.path != child.path));
}

#[test]
fn root_band_is_inserted_after_its_complete_visual_row() {
    let root_entries = vec![
        entry("/workspace/a", FileKind::File),
        entry("/workspace/root", FileKind::Directory),
        entry("/workspace/c", FileKind::File),
        entry("/workspace/d", FileKind::File),
    ];
    let state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/root", 1),
        loaded(files("/workspace/root", 2)),
    );
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let band = first_band(&layout);

    assert_eq!(band.top, ICON_GRID_CONTENT_PADDING + row_height(96));
    assert_eq!(band.anchor_column, 1);
    let rows_after = layout
        .root()
        .flow
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::Rows(rows) if rows.start_row == 1 => Some(rows),
            _ => None,
        })
        .unwrap();
    assert_eq!(rows_after.top, band.top + band.height);
}

#[test]
fn panel_layout_contains_only_the_active_sibling_band() {
    let root_entries = vec![
        entry("/workspace/alpha", FileKind::Directory),
        entry("/workspace/beta", FileKind::Directory),
        entry("/workspace/tail", FileKind::File),
    ];
    let mut state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/alpha", 0),
        loaded(files("/workspace/alpha", 1)),
    );
    assert!(!state.insert_directory(
        anchor("/workspace", "/workspace/beta", 1),
        loaded(files("/workspace/beta", 2)),
    ));
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let bands = layout
        .root()
        .flow
        .iter()
        .filter_map(|segment| match segment {
            IconGridFlowSegment::Band(band) => Some(band),
            IconGridFlowSegment::Rows(_) | IconGridFlowSegment::GroupHeader(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(bands.len(), 1);
    assert_eq!(bands[0].directory, Path::new("/workspace/alpha"));
}

#[test]
fn nested_band_contributes_to_parent_natural_height() {
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let mut state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/root", 0),
        loaded(vec![entry("/workspace/root/nested", FileKind::Directory)]),
    );
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/nested", 0,),
        loaded(files("/workspace/root/nested", 2)),
    ));
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let root_band = first_band(&layout);
    let nested_band = root_band
        .panel
        .flow
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::Band(band) => Some(band),
            IconGridFlowSegment::Rows(_) | IconGridFlowSegment::GroupHeader(_) => None,
        })
        .unwrap();

    assert_eq!(root_band.height, root_band.natural_height);
    assert!(root_band.natural_height > nested_band.natural_height);
}

#[test]
fn collapse_fraction_clips_band_height_without_flattening_entries() {
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let mut state = expansion(files("/workspace/root", 20));
    let root = state.directory_mut(Path::new("/workspace/root")).unwrap();
    root.contents.is_expanded = false;
    root.contents.is_collapsing = true;
    root.contents.animation_progress = 0.5;
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let band = first_band(&layout);

    assert_eq!(band.height, band.natural_height * 0.5);
    assert!(!band.interactive);
}

#[test]
fn keyboard_navigation_crosses_from_root_row_into_expansion_panel() {
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let state = expansion(vec![
        entry("/workspace/root/a", FileKind::File),
        entry("/workspace/root/b", FileKind::File),
    ]);
    let layout = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );

    let down = layout
        .keyboard_target(Some(Path::new("/workspace/root")), IconGridDirection::Down)
        .unwrap();
    assert_eq!(down.entry.path, Path::new("/workspace/root/a"));
    assert_eq!(down.directory, Path::new("/workspace/root"));
    let left = layout
        .keyboard_target(Some(&down.entry.path), IconGridDirection::Left)
        .unwrap();
    assert_eq!(left.entry.path, Path::new("/workspace/root"));
}

#[test]
fn resize_reflows_anchor_to_new_row_without_changing_state_index() {
    let root_entries = vec![
        entry("/workspace/a", FileKind::File),
        entry("/workspace/b", FileKind::File),
        entry("/workspace/root", FileKind::Directory),
    ];
    let state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/root", 2),
        loaded(files("/workspace/root", 1)),
    );
    let wide = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let narrow = IconGridLayout::new(
        Path::new("/workspace"),
        &root_entries,
        200.0,
        800.0,
        96,
        Some(&state),
    );

    assert_eq!(
        first_band(&wide).top,
        ICON_GRID_CONTENT_PADDING + row_height(96)
    );
    assert_eq!(
        first_band(&narrow).top,
        ICON_GRID_CONTENT_PADDING + row_height(96) * 3.0
    );
}

#[test]
fn large_flat_directory_stays_one_compressed_segment() {
    let entries = files("/workspace", 100_000);
    let layout = IconGridLayout::new(Path::new("/workspace"), &entries, 500.0, 800.0, 96, None);

    assert_eq!(layout.root().flow.len(), 1);
}

#[test]
fn flat_layout_paths_and_membership_preserve_entry_identity() {
    let entries = files("/workspace", 4);
    let layout = IconGridLayout::new(Path::new("/workspace"), &entries, 500.0, 800.0, 96, None);

    assert_eq!(
        layout.interactive_entry_paths(),
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>()
    );
    let matching = layout.interactive_paths_matching(&[
        entries[1].path.clone(),
        entries[3].path.clone(),
        PathBuf::from("/workspace/missing"),
    ]);
    assert_eq!(matching.len(), 2);
    assert!(matching.contains(&entries[1].path));
    assert!(matching.contains(&entries[3].path));
}

#[test]
fn expansion_band_merges_transfer_placeholders_for_its_directory() {
    // 拖放进展开中的目录:band 内占位与列表/多栏同一事实源,按该目录
    // 排序合入;空目录但有占位时按有内容渲染,不落「No items」空态。
    let root_entries = vec![entry("/workspace/root", FileKind::Directory)];
    let state = expansion(Vec::new());
    let mut expanded_placeholders = ExpandedTransferPlaceholderIndex::new();
    expanded_placeholders.insert(
        PathBuf::from("/workspace/root"),
        (
            vec![TransferPlaceholder {
                name: "incoming.txt".to_owned(),
                is_directory: false,
                progress: None,
                total_bytes: Some(1),
                enqueued_at: std::time::SystemTime::UNIX_EPOCH,
            }],
            TransferSortOptions {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
                directories_first: true,
            },
        ),
    );
    let layout = IconGridLayout::with_root_transfer_placeholders(
        Path::new("/workspace"),
        &root_entries,
        &[],
        TransferSortOptions {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
            directories_first: true,
        },
        &expanded_placeholders,
        500.0,
        800.0,
        96,
        Some(&state),
        None,
    );

    let band = first_band(&layout);
    assert_eq!(band.panel.status, IconGridPanelStatus::Loaded);
    let band_rows = band
        .panel
        .flow
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::Rows(rows) => Some(rows),
            IconGridFlowSegment::Band(_) | IconGridFlowSegment::GroupHeader(_) => None,
        })
        .unwrap();
    assert_eq!(band_rows.cells.len(), 1);
    assert!(matches!(band_rows.cells[0], IconGridCell::Placeholder(_)));
    // 占位格子不产生交互事实。
    let matching = layout.interactive_paths_matching(&[PathBuf::from("/workspace/root")]);
    assert_eq!(matching.len(), 1);
    assert!(matching.contains(Path::new("/workspace/root")));
}

#[test]
fn unmeasured_viewport_fallback_covers_window_bound() {
    // 未测量 viewport（从未滚动）时，初始窗口必须由窗口高度上界推导，
    // 而不是固定行数：30 条目目录在 600px 上界下只渲染可见窗口，
    // 上界足够覆盖全部内容时最后一行也必须出现。
    let entries = files("/workspace", 30);
    let layout = IconGridLayout::new(Path::new("/workspace"), &entries, 500.0, 600.0, 96, None);
    let visible = layout.visible_entries(IconGridViewport {
        offset_y: 0.0,
        width: 0.0,
        height: 0.0,
    });
    assert_eq!(visible.len(), 21);

    let full = IconGridLayout::new(Path::new("/workspace"), &entries, 500.0, 2600.0, 96, None);
    let all = full.visible_entries(IconGridViewport {
        offset_y: 0.0,
        width: 0.0,
        height: 0.0,
    });
    assert_eq!(all.len(), 30);
    assert_eq!(all.last().unwrap().entry.path, entries[29].path);
}
