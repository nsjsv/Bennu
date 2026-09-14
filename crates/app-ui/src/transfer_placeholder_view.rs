//! 传输占位行的渲染层:占位不进入目录数据模型,只在视图把真实条目流
//! 与队列派生的占位合并时插入。所有行都是纯静态容器——不包 button/
//! mouse_area、不发 Message、不注册拖拽命中,因此天然不可选中、不可
//! 交互,键盘导航按路径匹配时自动跳过。

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::appearance::list_row_style;
use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::file_drag_spring_ring::file_drag_spring_ring;
use crate::file_entry_view::{file_entry_symbol_icon, FileEntryIconDensity, FileEntryIconTone};
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::list_view::ListGeometry;
use crate::measured_middle_ellipsized_text::{
    measured_middle_ellipsized_centered_text, measured_middle_ellipsized_text,
};
use crate::model::{
    DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryStatus, FileEntryContentModifier,
    ListColumnConfig, ListColumnKind, Message,
};
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, transfer_placeholders_for_directory,
    MergedTransferItem, TransferPlaceholder, TransferSortOptions,
};
use crate::typography::readable_text;
use crate::virtual_range::VirtualRange;
use crate::visible_entries::{VisibleEntry, VisibleEntryStatusRow};

/// 占位图标槽:类型图标 + 外圈传输进度环。空进度画纯 track 环,与
/// spring ring 的 progress=0 形态一致。
pub(crate) fn transfer_placeholder_icon(
    placeholder: &TransferPlaceholder,
    density: FileEntryIconDensity,
) -> Element<'static, Message> {
    let symbol = if placeholder.is_directory {
        IconSymbol::Folder
    } else {
        file_entry_icon_symbol(file_core::FileKind::File, OsStr::new(&placeholder.name))
    };
    let icon_size = density.icon_size();
    let slot_size = density.thumbnail_size();
    let icon_slot: Element<'static, Message> = container(file_entry_symbol_icon(
        symbol,
        FileEntryIconTone::Normal,
        icon_size,
        FileEntryContentModifier::None,
    ))
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size))
    .center_x(Length::Fixed(slot_size))
    .center_y(Length::Fixed(slot_size))
    .into();
    iced::widget::Stack::with_children([
        icon_slot,
        file_drag_spring_ring(placeholder.progress.unwrap_or(0.0)),
    ])
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size))
    .into()
}

fn transfer_placeholder_name_text(
    placeholder: &TransferPlaceholder,
    text_size: u32,
) -> Element<'static, Message> {
    measured_middle_ellipsized_text(placeholder.name.clone(), text_size)
}

/// 列表视图的占位行:名字列放图标+环+名字,其余列显示缺省占位,
/// 行高与普通行一致(计入虚拟滚动)。
pub(crate) fn transfer_placeholder_list_row(
    placeholder: &TransferPlaceholder,
    geometry: &ListGeometry,
    visible_columns: &[ListColumnConfig],
    row_index: usize,
) -> Element<'static, Message> {
    let mut cells = row![]
        .spacing(0)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for (index, column) in visible_columns.iter().enumerate() {
        if index > 0 {
            cells = cells.push(list_column_gap());
        }
        let cell: Element<'static, Message> = match column.kind {
            ListColumnKind::Name => row![
                Space::new().width(Length::Fixed(geometry.toggle_width)),
                transfer_placeholder_icon(placeholder, geometry.icon_density),
                transfer_placeholder_name_text(placeholder, geometry.text_size as u32),
            ]
            .spacing(geometry.row_spacing)
            .align_y(Alignment::Center)
            .width(list_column_width(column.width))
            .into(),
            _ => container(readable_text("-").size(geometry.text_size))
                .width(list_column_width(column.width))
                .clip(true)
                .into(),
        };
        cells = cells.push(cell);
    }
    cells = cells.push(list_column_gap());

    container(cells)
        .padding(geometry.row_padding)
        .height(Length::Fixed(geometry.row_height))
        .center_y(Length::Fixed(geometry.row_height))
        .width(Length::Fill)
        .style(list_row_style(0, row_index))
        .into()
}

fn list_column_gap() -> Element<'static, Message> {
    Space::new()
        .width(Length::Fixed(crate::list_view::LIST_COLUMN_GAP_WIDTH))
        .into()
}

fn list_column_width(width: f32) -> Length {
    Length::FillPortion(width.round().clamp(1.0, u16::MAX as f32) as u16)
}

/// 多栏视图的占位行:图标+环+名字,末尾留出目录箭头槽位。
pub(crate) fn transfer_placeholder_column_row(
    placeholder: &TransferPlaceholder,
    geometry: crate::three_column_view::ColumnGeometry,
) -> Element<'static, Message> {
    let content = row![
        transfer_placeholder_icon(placeholder, geometry.icon_density),
        container(transfer_placeholder_name_text(
            placeholder,
            geometry.text_size as u32
        )),
        Space::new().width(Length::Fixed(geometry.chevron_icon_size)),
    ]
    .spacing(geometry.entry_spacing)
    .align_y(Alignment::Center);
    container(content)
        .padding(geometry.entry_padding)
        .height(Length::Fixed(geometry.entry_height))
        .center_y(Length::Fixed(geometry.entry_height))
        .width(Length::Fill)
        .into()
}

/// 图标网格的占位格子:与条目瓦片同几何,静态无交互。
pub(crate) fn transfer_placeholder_grid_cell(
    placeholder: &TransferPlaceholder,
    icon_edge: u32,
) -> Element<'static, Message> {
    use crate::icon_grid_geometry::{
        icon_label_spacing, label_height, label_line_height, label_size, tile_padding_horizontal,
        tile_padding_vertical, tile_visual_height, tile_width,
    };
    let label = container(measured_middle_ellipsized_centered_text(
        placeholder.name.clone(),
        label_size(icon_edge),
        label_line_height(icon_edge),
    ))
    .height(Length::Fixed(label_height(icon_edge)))
    .clip(true)
    .width(Length::Fill);
    container(
        column![
            transfer_placeholder_icon(placeholder, FileEntryIconDensity::Grid(icon_edge)),
            label,
        ]
        .align_x(Alignment::Center)
        .spacing(icon_label_spacing(icon_edge)),
    )
    .padding(iced::Padding {
        top: tile_padding_vertical(icon_edge),
        right: tile_padding_horizontal(icon_edge),
        bottom: tile_padding_vertical(icon_edge),
        left: tile_padding_horizontal(icon_edge),
    })
    .width(Length::Fixed(tile_width(icon_edge)))
    .height(Length::Fixed(tile_visual_height(icon_edge)))
    .into()
}

/// 根目录(当前视图目录)的排序配置:与扫描侧使用的选项同源。
pub(crate) fn transfer_sort_for_root(browser: &FileBrowser) -> TransferSortOptions {
    TransferSortOptions {
        field: browser.options.sort_field,
        direction: browser.options.sort_direction,
        directories_first: browser.options.directories_first,
    }
}

/// 展开目录的排序配置:条目顺序由各自 directory_order_phase 记录,
/// directories_first 沿用全局扫描选项。
pub(crate) fn transfer_sort_for_expanded(
    browser: &FileBrowser,
    expanded: &ExpandedDirectory,
) -> TransferSortOptions {
    let (field, direction) = match expanded.directory_order_phase {
        DirectoryOrderPhase::Ready { field, direction } => (field, direction),
        DirectoryOrderPhase::WaitingForMetadata {
            field, direction, ..
        } => (field, direction),
    };
    TransferSortOptions {
        field,
        direction,
        directories_first: browser.options.directories_first,
    }
}

/// 列表视图的合并行流:真实条目行、展开状态行与传输占位行交错,
/// 一次性摊平后供虚拟范围计算与行渲染共用。
pub(crate) enum ListTransferRow<'a> {
    Entry(VisibleEntry<'a>),
    DirectoryStatusRow {
        message: &'static str,
        depth: usize,
        height: f32,
    },
    Placeholder(TransferPlaceholder),
}

impl ListTransferRow<'_> {
    /// 与渲染同口径:条目行按展开动画进度收缩(list_entry_row 的容器
    /// 高度),状态行按各自动画高度,占位行恒定整行高。
    fn height(&self, row_height: f32) -> f32 {
        match self {
            Self::DirectoryStatusRow { height, .. } => *height,
            Self::Entry(visible) => row_height * visible.animation_progress.clamp(0.0, 1.0),
            Self::Placeholder(_) => row_height,
        }
    }

    /// 选中连排(selection run)计算只看真实条目;占位与状态行天然断开。
    fn entry_path(&self) -> Option<&Path> {
        match self {
            Self::Entry(visible) => Some(visible.entry.path.as_path()),
            Self::DirectoryStatusRow { .. } | Self::Placeholder(_) => None,
        }
    }
}

/// 构建列表合并行流:根目录与各展开目录分别派生占位并按各自排序合入。
pub(crate) fn build_list_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    row_height: f32,
) -> Vec<ListTransferRow<'a>> {
    let mut placeholder_cache: HashMap<PathBuf, Vec<TransferPlaceholder>> = HashMap::new();
    let mut rows = Vec::new();
    push_directory_transfer_rows(
        browser,
        pane,
        pane.entries,
        pane.current_dir.as_path(),
        transfer_sort_for_root(browser),
        0,
        1.0,
        row_height,
        &mut placeholder_cache,
        &mut rows,
    );
    rows
}

#[allow(clippy::too_many_arguments)]
fn push_directory_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    entries: &'a [DirectoryEntry],
    directory: &Path,
    sort: TransferSortOptions,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    let placeholders = placeholder_cache
        .entry(directory.to_path_buf())
        .or_insert_with(|| {
            transfer_placeholders_for_directory(&browser.operation_queue, directory)
        })
        .clone();
    let merged = merge_entries_with_placeholders(entries, &placeholders, sort);
    for item in merged {
        match item {
            MergedTransferItem::Entry(entry) => {
                rows.push(ListTransferRow::Entry(VisibleEntry {
                    entry,
                    depth,
                    animation_progress,
                }));
                push_expansion_transfer_rows(
                    browser,
                    pane,
                    entry,
                    depth,
                    animation_progress,
                    row_height,
                    placeholder_cache,
                    rows,
                );
            }
            MergedTransferItem::Placeholder(placeholder) => {
                rows.push(ListTransferRow::Placeholder(placeholder));
            }
        }
    }
}

/// 展开目录的状态行与子行:与 visible_entries 的可见性规则一致
/// (折叠中仍显示,Loading 不暴露子条目,Error/空目录显示状态行)。
#[allow(clippy::too_many_arguments)]
fn push_expansion_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    entry: &'a DirectoryEntry,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    let Some(expanded) = pane
        .expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return;
    };
    if let Some(status) = crate::visible_entries::visible_entry_status_row(expanded) {
        let message = match status {
            VisibleEntryStatusRow::Error => "Could not load",
            VisibleEntryStatusRow::Empty => "No items",
        };
        rows.push(ListTransferRow::DirectoryStatusRow {
            message,
            depth: depth + 1,
            height: row_height * expanded.animation_progress.clamp(0.0, 1.0),
        });
    }
    if !matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
        return;
    }
    let child_progress = animation_progress * expanded.animation_progress.clamp(0.0, 1.0);
    push_directory_transfer_rows(
        browser,
        pane,
        &expanded.entries,
        entry.path.as_path(),
        transfer_sort_for_expanded(browser, expanded),
        depth + 1,
        child_progress,
        row_height,
        placeholder_cache,
        rows,
    );
}

/// 虚拟范围:与列表条目范围同口径(状态行高度按动画比例计入)。
pub(crate) fn list_transfer_rows_range_for_viewport(
    rows: &[ListTransferRow],
    row_height: f32,
    header_height: f32,
    viewport_offset: f32,
    viewport_height: f32,
    overscan_rows: usize,
) -> VirtualRange {
    if row_height <= f32::EPSILON || viewport_height <= f32::EPSILON {
        return VirtualRange::empty();
    }
    let viewport_top = viewport_offset.max(0.0);
    let overscan_height = overscan_rows as f32 * row_height;
    transfer_rows_height_range(
        rows,
        row_height,
        (viewport_top - header_height - overscan_height).max(0.0),
        (viewport_top + viewport_height - header_height).max(0.0) + overscan_height,
    )
}

pub(crate) fn initial_list_transfer_rows_range(
    rows: &[ListTransferRow],
    row_height: f32,
    initial_rows: usize,
) -> VirtualRange {
    if row_height <= f32::EPSILON || initial_rows == 0 {
        return VirtualRange::empty();
    }
    let mut total_height = 0.0;
    let mut initial_height = 0.0;
    for (index, row) in rows.iter().enumerate() {
        let height = row.height(row_height);
        total_height += height;
        if index < initial_rows {
            initial_height += height;
        }
    }
    VirtualRange {
        start: 0,
        end: initial_rows.min(rows.len()),
        before_height: 0.0,
        after_height: (total_height - initial_height).max(0.0),
    }
}

fn transfer_rows_height_range(
    rows: &[ListTransferRow],
    row_height: f32,
    top: f32,
    bottom: f32,
) -> VirtualRange {
    let mut row_count = 0;
    let mut total_height = 0.0;
    let mut start = None;
    let mut end = 0;
    let mut before_height = 0.0;
    let mut rendered_end_height = 0.0;

    for row in rows {
        let index = row_count;
        row_count += 1;
        let block_top = total_height;
        total_height += row.height(row_height);

        let include = total_height > top && block_top < bottom;
        if include {
            if start.is_none() {
                start = Some(index);
                before_height = block_top;
            }
            end = index + 1;
            rendered_end_height = total_height;
        }
    }

    let start = start.unwrap_or(row_count);
    if start == row_count {
        before_height = total_height;
        rendered_end_height = total_height;
        end = row_count;
    }
    VirtualRange {
        start,
        end,
        before_height,
        after_height: (total_height - rendered_end_height).max(0.0),
    }
}

/// 选中连排位置:邻居行只在相邻真实条目同被选中时延续连排。
pub(crate) fn selection_run_position_in_transfer_rows(
    rows: &[ListTransferRow],
    index: usize,
    selected_paths: &std::collections::HashSet<PathBuf>,
) -> Option<crate::file_entry_presentation::SelectionRunPosition> {
    let row = rows.get(index)?;
    let entry_path = row.entry_path()?;
    if !selected_paths.contains(entry_path) {
        return None;
    }
    let previous_selected = index
        .checked_sub(1)
        .and_then(|previous| rows.get(previous))
        .and_then(|neighbor| neighbor.entry_path())
        .is_some_and(|path| selected_paths.contains(path));
    let next_selected = rows
        .get(index + 1)
        .and_then(|neighbor| neighbor.entry_path())
        .is_some_and(|path| selected_paths.contains(path));
    Some(crate::file_entry_presentation::SelectionRunPosition::from_neighbors(
        previous_selected,
        next_selected,
    ))
}

/// 任一 pane 目录的排序配置:当前目录用扫描选项,其余目录用各自展开
/// 目录的 directory_order_phase。三栏视图与 reveal 数学共用此解析,
/// 保证插入排序与渲染同源。
pub(crate) fn transfer_sort_for_pane_directory(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    directory: &Path,
) -> TransferSortOptions {
    if directory == pane.current_dir.as_path() {
        return transfer_sort_for_root(browser);
    }
    pane.expanded_directories
        .get(directory)
        .map(|expanded| transfer_sort_for_expanded(browser, expanded))
        .unwrap_or_else(|| transfer_sort_for_root(browser))
}

/// 条目在列表合并行流中的纵向位置:reveal/键盘跟随数学与列表渲染
/// 消费同一条行流,占位行与展开状态行都计入,落点才与实际渲染一致。
pub(crate) fn list_transfer_rows_vertical_bounds(
    rows: &[ListTransferRow],
    path: &Path,
    row_height: f32,
    header_height: f32,
) -> Option<(f32, f32)> {
    let mut offset = header_height;
    for row in rows {
        let height = row.height(row_height);
        if let ListTransferRow::Entry(visible) = row {
            if visible.entry.path == path {
                return Some((offset, height));
            }
        }
        offset += height;
    }
    None
}

/// 列表合并行流总高:视口越界自愈的最大偏移口径,与渲染累计高度一致。
pub(crate) fn list_transfer_rows_content_height(rows: &[ListTransferRow], row_height: f32) -> f32 {
    rows.iter().map(|row| row.height(row_height)).sum()
}

fn column_directory_entries<'a>(
    pane: BrowserPaneView<'a>,
    directory: &Path,
) -> Option<&'a [DirectoryEntry]> {
    if directory == pane.current_dir.as_path() {
        Some(pane.entries)
    } else {
        pane.expanded_directories
            .get(directory)
            .map(|expanded| expanded.entries.as_slice())
    }
}

/// 条目在多栏合并流中的纵向偏移(行距倍数):reveal 与多栏渲染共用
/// 同一合并序列,占位插入造成的位移在两侧一致。
pub(crate) fn column_transfer_item_offset(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    directory: &Path,
    path: &Path,
    row_pitch: f32,
) -> Option<f32> {
    let entries = column_directory_entries(pane, directory)?;
    let placeholders = transfer_placeholders_for_directory(&browser.operation_queue, directory);
    let merged = merge_entries_with_placeholders(
        entries,
        &placeholders,
        transfer_sort_for_pane_directory(browser, pane, directory),
    );
    let index = merged
        .iter()
        .position(|item| matches!(item, MergedTransferItem::Entry(entry) if entry.path == path))?;
    Some(index as f32 * row_pitch)
}

/// 多栏合并流的格子数:视口越界自愈的内容高口径,占位计入。
pub(crate) fn column_transfer_item_count(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    directory: &Path,
) -> usize {
    let Some(entries) = column_directory_entries(pane, directory) else {
        return 0;
    };
    entries.len()
        + transfer_placeholders_for_directory(&browser.operation_queue, directory).len()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

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
        ListTransferRow::Placeholder(TransferPlaceholder {
            name: name.to_owned(),
            is_directory: false,
            progress: None,
            total_bytes: None,
            enqueued_at: std::time::SystemTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn vertical_bounds_count_placeholder_rows_above_the_target() {
        // reveal 数学与渲染同流:占位行在选中项上方时,落点必须计入。
        let entries = [entry("/d/a.txt"), entry("/d/z.txt")];
        let rows = [
            ListTransferRow::Entry(VisibleEntry {
                entry: &entries[0],
                depth: 0,
                animation_progress: 1.0,
            }),
            placeholder_row("incoming.txt"),
            ListTransferRow::Entry(VisibleEntry {
                entry: &entries[1],
                depth: 0,
                animation_progress: 1.0,
            }),
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
            ListTransferRow::Entry(VisibleEntry {
                entry: &entry,
                depth: 0,
                animation_progress: 0.5,
            }),
            placeholder_row("incoming.txt"),
            ListTransferRow::DirectoryStatusRow {
                message: "No items",
                depth: 1,
                height: 40.0,
            },
        ];
        assert_eq!(list_transfer_rows_content_height(&rows, 40.0), 100.0);
    }
}
