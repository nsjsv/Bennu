// 大图根面板分组的单元测试:与实现/既有夹具分文件以守住单文件 800 行
// 约束。覆盖:组头流段的流高累计与 reveal 同口径、可见窗口按整段相交、
// 目录(含目录占位)置顶、None 回归、交互 path 集不含组头、展开锚点按
// path 定位不受流段插入影响、子面板不出组头。
use std::path::{Path, PathBuf};

use crate::icon_grid_geometry::{tile_visual_height, tile_width};
use crate::model::{BrowserPaneId, IconGridExpansionContext, IconGridExpansionSessionId};
use crate::transfer_placeholders::{TransferPlaceholder, TransferSortOptions};
use file_core::{DirectoryEntry, FileKind};

use super::tests::{anchor, entry, expansion, files, first_band, loaded};
use super::*;

fn placeholder(name: &str, is_directory: bool) -> TransferPlaceholder {
    TransferPlaceholder {
        name: name.to_owned(),
        is_directory,
        progress: None,
        total_bytes: None,
        enqueued_at: std::time::SystemTime::UNIX_EPOCH,
    }
}

fn grouped_entries() -> Vec<DirectoryEntry> {
    // 条目数组序(名字升序 + 目录优先):root 在前,文件按首字母分布 A/Z。
    vec![
        entry("/workspace/root", FileKind::Directory),
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/avocado.md", FileKind::File),
        entry("/workspace/z.txt", FileKind::File),
    ]
}

#[test]
fn grouped_root_flow_accumulates_header_heights_and_reveal_math() {
    // 分组开启:目录行区在前,文件段按组输出[组头条(32), 组行段];
    // 流高、reveal 落点与可见窗口全部按流段几何累计。
    let entries = grouped_entries();
    let layout = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &[],
        file_core::SortDirection::Ascending,
        500.0,
        800.0,
        96,
        None,
    );
    let flow = &layout.root().flow;
    assert_eq!(flow.len(), 5);
    let header_a_top = ICON_GRID_CONTENT_PADDING + row_height(96);
    let rows_a_top = header_a_top + ICON_GRID_GROUP_HEADER_HEIGHT;
    let header_z_top = rows_a_top + row_height(96);
    let rows_z_top = header_z_top + ICON_GRID_GROUP_HEADER_HEIGHT;
    let group_tops: Vec<(f32, f32)> = flow
        .iter()
        .filter_map(|segment| match segment {
            IconGridFlowSegment::GroupHeader(header) => {
                Some((header.title.as_str(), header.count, header.top))
            }
            _ => None,
        })
        .map(|(title, count, top)| {
            assert_eq!(title, if top < header_z_top { "A" } else { "Z" });
            (top, count as f32)
        })
        .collect();
    assert_eq!(
        group_tops,
        vec![(header_a_top, 2.0), (header_z_top, 1.0)]
    );
    // 段序列:目录行 -> 组头A -> 组行A -> 组头Z -> 组行Z。
    assert!(matches!(flow[0], IconGridFlowSegment::Rows(_)));
    assert!(matches!(flow[1], IconGridFlowSegment::GroupHeader(_)));
    assert!(matches!(flow[2], IconGridFlowSegment::Rows(_)));
    assert!(matches!(flow[3], IconGridFlowSegment::GroupHeader(_)));
    assert!(matches!(flow[4], IconGridFlowSegment::Rows(_)));
    assert_eq!(flow[3].top(), header_z_top);
    assert_eq!(flow[4].top(), rows_z_top);
    // height() 累计:内容高 = 内边距 + 3 行 + 2 条组头。
    assert_eq!(
        layout.total_height(),
        ICON_GRID_CONTENT_PADDING * 2.0
            + row_height(96) * 3.0
            + ICON_GRID_GROUP_HEADER_HEIGHT * 2.0
    );

    // reveal 落点:组头与中间行段计入偏移,z.txt 在组头Z后的行段里。
    let viewport = IconGridViewport {
        offset_y: 0.0,
        width: 500.0,
        height: 300.0,
    };
    let tile_visual = tile_visual_height(96);
    assert_eq!(
        layout.scroll_delta_to_reveal(viewport, Path::new("/workspace/z.txt")),
        rows_z_top + tile_visual - 300.0
    );
    assert_eq!(
        layout.scroll_delta_to_reveal(viewport, Path::new("/workspace/a.txt")),
        rows_a_top + tile_visual - 300.0
    );

    // 可见窗口:窗口只与组Z行段相交时,组A条目(隔一条组头)不出现。
    let shifted = IconGridViewport {
        offset_y: 1000.0,
        width: 500.0,
        height: 100.0,
    };
    let visible = layout.visible_entries(shifted);
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].entry.path, Path::new("/workspace/z.txt"));
}

#[test]
fn grouped_root_places_directory_cells_and_placeholders_first_without_headers() {
    // 目录占位与目录实体都置顶进行区;组头只出现在文件段之前,
    // None 模式下同一输入回到单行段平铺(分组关闭回归保护)。
    let entries = vec![
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/root", FileKind::Directory),
    ];
    let placeholders = [
        placeholder("z-incoming", false),
        placeholder("dir-incoming", true),
    ];
    let grouped = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &placeholders,
        file_core::SortDirection::Ascending,
        500.0,
        800.0,
        96,
        None,
    );
    let flow = &grouped.root().flow;
    let directory_rows = match &flow[0] {
        IconGridFlowSegment::Rows(rows) => rows,
        _ => panic!("directory cells must lead the grouped flow"),
    };
    assert!(matches!(
        &directory_rows.cells[0],
        IconGridCell::Placeholder(placeholder) if placeholder.name == "dir-incoming"
    ));
    assert!(matches!(
        &directory_rows.cells[1],
        IconGridCell::Entry(cell) if cell.entry.path == Path::new("/workspace/root")
    ));
    assert!(matches!(flow[1], IconGridFlowSegment::GroupHeader(_)));

    // None 回归:根面板流段与既有平铺一致,不含任何组头段。
    let flat = IconGridLayout::with_root_transfer_placeholders(
        Path::new("/workspace"),
        &entries,
        &placeholders,
        TransferSortOptions {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
            directories_first: true,
        },
        &ExpandedTransferPlaceholderIndex::new(),
        500.0,
        800.0,
        96,
        None,
        None,
    );
    assert!(flat
        .root()
        .flow
        .iter()
        .all(|segment| matches!(segment, IconGridFlowSegment::Rows(_))));
}

#[test]
fn grouped_flow_resolves_expansion_anchor_by_path_not_cell_order() {
    // 不变量:锚点按 path 解析、下标由布局重算。分组把 d2 从条目下标 2
    // 重排到格子下标 1,锚点(条目下标 2)必须落在 d2 的真实格子
    // (第 0 行第 1 列);若按格子顺序计数会错位到第 1 行第 0 列。
    let entries = vec![
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/d1", FileKind::Directory),
        entry("/workspace/d2", FileKind::Directory),
        entry("/workspace/d3", FileKind::Directory),
        entry("/workspace/d4", FileKind::Directory),
        entry("/workspace/z.txt", FileKind::File),
    ];
    let state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId(1),
            current_dir: PathBuf::from("/workspace"),
            session_id: IconGridExpansionSessionId::new(1),
        },
        anchor("/workspace", "/workspace/d2", 2),
        loaded(files("/workspace/d2", 1)),
    );
    let two_column_width =
        ICON_GRID_CONTENT_PADDING * 2.0 + tile_width(96) * 2.0 + crate::icon_grid_geometry::ICON_GRID_GAP;
    let layout = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &[],
        file_core::SortDirection::Ascending,
        two_column_width,
        800.0,
        96,
        Some(&state),
    );
    let band = first_band(&layout);
    assert_eq!(band.directory, Path::new("/workspace/d2"));
    assert_eq!(band.anchor_column, 1);
    assert_eq!(band.top, ICON_GRID_CONTENT_PADDING + row_height(96));
    // band 之后先是剩余的置顶目录行(d3/d4),目录区走完才进入文件组;
    // 组头A 的顶点必须严丝合缝接在目录区末尾,流高不重叠不断档。
    let segments = &layout.root().flow;
    let band_index = segments
        .iter()
        .position(|segment| matches!(segment, IconGridFlowSegment::Band(_)))
        .unwrap();
    assert!(matches!(
        segments.get(band_index + 1),
        Some(IconGridFlowSegment::Rows(_))
    ));
    let header_after_band = segments
        .iter()
        .find_map(|segment| match segment {
            IconGridFlowSegment::GroupHeader(header) => Some(header),
            _ => None,
        })
        .unwrap();
    assert_eq!(header_after_band.title, "A");
    assert_eq!(
        header_after_band.top,
        band.top + band.height + row_height(96)
    );
}

#[test]
fn grouped_root_interaction_targets_skip_group_headers() {
    // 组头不是格子:交互 path 集合只含真实条目;键盘向下越过组头直接
    // 落到下一组的条目上。
    let entries = grouped_entries();
    let layout = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &[],
        file_core::SortDirection::Ascending,
        500.0,
        800.0,
        96,
        None,
    );
    assert_eq!(
        layout.interactive_entry_paths(),
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>()
    );
    let down = layout
        .keyboard_target(Some(Path::new("/workspace/a.txt")), IconGridDirection::Down)
        .unwrap();
    assert_eq!(down.entry.path, Path::new("/workspace/z.txt"));
}

#[test]
fn expanded_child_panels_stay_flat_when_grouping_is_enabled() {
    // 分组只作用根面板:展开子面板(band)内容平铺,不出组头。
    let entries = grouped_entries();
    let state = expansion(vec![
        entry("/workspace/root/child-a.txt", FileKind::File),
        entry("/workspace/root/child-z.txt", FileKind::File),
    ]);
    let layout = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &[],
        file_core::SortDirection::Ascending,
        500.0,
        800.0,
        96,
        Some(&state),
    );
    let band = first_band(&layout);
    assert!(band
        .panel
        .flow
        .iter()
        .all(|segment| matches!(segment, IconGridFlowSegment::Rows(_))));
    assert_eq!(band.panel.flow.len(), 1);
}

#[test]
fn rail_entries_derive_from_group_header_flow_segments_and_gate_by_count() {
    // 索引定位直接读根面板 flow 的组头条段:top 即组头顶点,与渲染/
    // reveal 同一事实源,不重算分组也不重算高度。
    let entries = grouped_entries();
    let layout = IconGridLayout::new_grouped_by_name_initial(
        Path::new("/workspace"),
        &entries,
        &[],
        file_core::SortDirection::Ascending,
        500.0,
        800.0,
        96,
        None,
    );
    let rail = layout.root().file_group_rail_entries();
    let header_a_top = ICON_GRID_CONTENT_PADDING + row_height(96);
    let header_z_top = header_a_top + ICON_GRID_GROUP_HEADER_HEIGHT + row_height(96);
    assert_eq!(rail.len(), 2);
    assert_eq!(rail[0].index_label, "A");
    assert_eq!(rail[0].top_offset, header_a_top);
    assert_eq!(rail[1].index_label, "Z");
    assert_eq!(rail[1].top_offset, header_z_top);
    assert!(crate::model::file_group_rail_visible(&rail));

    // None 回归:平铺流没有组头段,索引栏无数据、不显示。
    let flat_entries = vec![
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/z.txt", FileKind::File),
    ];
    let flat = IconGridLayout::with_root_transfer_placeholders(
        Path::new("/workspace"),
        &flat_entries,
        &[],
        TransferSortOptions {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
            directories_first: true,
        },
        &ExpandedTransferPlaceholderIndex::new(),
        500.0,
        800.0,
        96,
        None,
        None,
    );
    let flat_rail = flat.root().file_group_rail_entries();
    assert!(flat_rail.is_empty());
    assert!(!crate::model::file_group_rail_visible(&flat_rail));
}
