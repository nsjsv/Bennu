//! 传输占位行的渲染层:占位不进入目录数据模型,只在视图把真实条目流
//! 与队列派生的占位合并时插入。所有行都是纯静态容器——不包 button/
//! mouse_area、不发 Message、不注册拖拽命中,因此天然不可选中、不可
//! 交互,键盘导航按路径匹配时自动跳过。

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length, Point};

use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::appearance::list_row_style;
use crate::file_drag_spring_ring::file_drag_spring_ring;
use crate::file_entry_view::{file_entry_symbol_icon, FileEntryIconDensity, FileEntryIconTone};
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::list_view::{ListGeometry, LIST_GROUP_HEADER_HEIGHT};
use crate::measured_middle_ellipsized_text::{
    measured_middle_ellipsized_centered_text, measured_middle_ellipsized_text,
};
use crate::model::{
    DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryStatus, FileEntryContentModifier,
    ListColumnConfig, ListColumnKind, Message,
};
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, transfer_placeholders_for_directory, MergedTransferItem,
    TransferPlaceholder, TransferSortOptions,
};
use crate::typography::readable_text;
use crate::virtual_range::VirtualRange;
use crate::visible_entries::{VisibleEntry, VisibleEntryStatusRow};

/// 根目录分组划分:目录置顶 + 文件段按组分段,列表/大图的当前目录
/// 共用(网格侧经 icon_grid_layout 的分组入参消费)。
#[path = "transfer_placeholder_view/root_grouping.rs"]
pub(crate) mod root_grouping;

/// 占位图标槽:类型图标 + 外圈传输进度环。空进度画纯 track 环,与
/// spring ring 的 progress=0 形态一致。
/// 真实条目图标的传输装饰:与占位行同款进度环叠加在图标槽上。
/// `transfer` 为 None 时原样返回,不引入额外层级。
pub(crate) fn entry_icon_with_transfer<'a>(
    base: Element<'a, Message>,
    transfer: Option<&TransferPlaceholder>,
    slot_size: f32,
) -> Element<'a, Message> {
    let Some(transfer) = transfer else {
        return base;
    };
    iced::widget::Stack::with_children([
        iced::widget::container(base)
            .width(iced::Length::Fixed(slot_size))
            .height(iced::Length::Fixed(slot_size))
            .center_x(iced::Length::Fixed(slot_size))
            .center_y(iced::Length::Fixed(slot_size))
            .into(),
        file_drag_spring_ring(transfer.progress.unwrap_or(0.0)),
    ])
    .width(iced::Length::Fixed(slot_size))
    .height(iced::Length::Fixed(slot_size))
    .into()
}

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
/// 行高与普通行一致(计入虚拟滚动)。条纹消耗所在目录段计数器的
/// 当前值:占位行参与交替,不再与下一真实行同相。
pub(crate) fn transfer_placeholder_list_row(
    placeholder: &TransferPlaceholder,
    geometry: &ListGeometry,
    visible_columns: &[ListColumnConfig],
    stripe_index: usize,
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
        .style(list_row_style(0, stripe_index))
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
///
/// 条纹相位不变量:stripe_index 在行流构建期按"每个目录段一个计数器、
/// 仅 Entry 行自增"赋值,组头/状态行/占位行只透传当前值(不自增)。
/// 旧实现用合并流的绝对下标,任何 chrome 行插入都翻转奇偶相位,条纹
/// 呈不连续随机分布;这里把相位所有权收回条目序,chrome 行插拔不再
/// 影响任何条目的条纹。
pub(crate) enum ListTransferRow<'a> {
    Entry {
        visible: VisibleEntry<'a>,
        /// 同名传输占位改为装饰挂在真实行上(图标叠加进度环),不再单独成行。
        transfer: Option<TransferPlaceholder>,
        stripe_index: usize,
    },
    DirectoryStatusRow {
        message: &'static str,
        depth: usize,
        height: f32,
        stripe_index: usize,
    },
    Placeholder {
        placeholder: TransferPlaceholder,
        stripe_index: usize,
    },
    /// 根目录分组标题行:仅分组开启时插入文件段之前,title 与
    /// index_label 都是构造时本地化的成品文案(后者供索引栏派生)。
    /// entry_path()=None 使其与占位行同一机制地断开选中连排、不进任何
    /// 条目集合。
    GroupHeader {
        title: String,
        index_label: String,
        count: usize,
        height: f32,
    },
}

impl ListTransferRow<'_> {
    /// 与渲染同口径:条目行按展开动画进度收缩(list_entry_row 的容器
    /// 高度),状态行按各自动画高度,占位行恒定整行高,组头行按自身
    /// chrome 高度——虚拟范围/内容高/reveal 数学全部基于本函数累计。
    fn height(&self, row_height: f32) -> f32 {
        match self {
            Self::DirectoryStatusRow { height, .. } | Self::GroupHeader { height, .. } => *height,
            Self::Entry { visible, .. } => row_height * visible.animation_progress.clamp(0.0, 1.0),
            Self::Placeholder { .. } => row_height,
        }
    }

    /// 选中连排(selection run)计算只看真实条目;占位与状态行天然断开。
    fn entry_path(&self) -> Option<&Path> {
        match self {
            Self::Entry { visible, .. } => Some(visible.entry.path.as_path()),
            Self::DirectoryStatusRow { .. }
            | Self::GroupHeader { .. }
            | Self::Placeholder { .. } => None,
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
        .or_insert_with(|| transfer_placeholders_for_directory(&browser.operation_queue, directory))
        .clone();
    let merged = merge_entries_with_placeholders(entries, &placeholders, sort);
    // 条纹计数器按目录段各自新建:递归展开的子目录从 0 起算,组间不
    // 重置(本段计数器跨组延续),chrome 行插拔不消耗序号。
    let mut stripe_index = 0usize;
    // 分组只作用于当前目录(根);展开子目录递归进入时 directory 已经
    // 是子目录路径,天然不等于 current_dir,走平铺分支保持展开内容不分组。
    let Some(grouping) = root_grouping::RootGrouping::for_pane_root(browser, pane, directory)
    else {
        for item in &merged {
            push_list_transfer_item(
                browser,
                pane,
                item,
                depth,
                animation_progress,
                row_height,
                placeholder_cache,
                &mut stripe_index,
                rows,
            );
        }
        return;
    };
    // 分组开启:目录(含目录占位)稳定置顶(不受 directories_first 影响),
    // 文件段按组分段,每组先输出组头行再摊平组内条目/占位行。
    let (directories, files) = root_grouping::split_directories_first(merged);
    for item in &directories {
        push_list_transfer_item(
            browser,
            pane,
            item,
            depth,
            animation_progress,
            row_height,
            placeholder_cache,
            &mut stripe_index,
            rows,
        );
    }
    for section in grouping.partition(&files, |entry| pane.metadata_for_entry(entry)) {
        rows.push(ListTransferRow::GroupHeader {
            title: section.descriptor.title,
            index_label: section.descriptor.index_label,
            count: section.items.len(),
            height: LIST_GROUP_HEADER_HEIGHT,
        });
        for item in section.items {
            push_list_transfer_item(
                browser,
                pane,
                item,
                depth,
                animation_progress,
                row_height,
                placeholder_cache,
                &mut stripe_index,
                rows,
            );
        }
    }
}

/// 单个合并项的行摊平:条目行后紧跟其展开子行,占位行直接入流。
/// 组头行不经过此处——它不属于任何合并项,由分组划分单独插入。
/// stripe_index 是所在目录段的条纹计数器:Entry 与占位行都消耗序号
/// (占位若透传,会与下一真实行同相,出现连续同色两行);展开状态行
/// 在子段创建前同样透传。
#[allow(clippy::too_many_arguments)]
fn push_list_transfer_item<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    item: &MergedTransferItem<'a>,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    stripe_index: &mut usize,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    match item {
        MergedTransferItem::Entry { entry, transfer } => {
            rows.push(ListTransferRow::Entry {
                visible: VisibleEntry {
                    entry,
                    depth,
                    animation_progress,
                },
                transfer: transfer.clone(),
                stripe_index: *stripe_index,
            });
            *stripe_index += 1;
            push_expansion_transfer_rows(
                browser,
                pane,
                entry,
                depth,
                animation_progress,
                row_height,
                placeholder_cache,
                stripe_index,
                rows,
            );
        }
        // 占位是合并时逐个克隆出的临时集合,按引用再克隆一份成本可忽略。
        MergedTransferItem::Placeholder(placeholder) => {
            rows.push(ListTransferRow::Placeholder {
                placeholder: placeholder.clone(),
                stripe_index: *stripe_index,
            });
            // 占位行参与条纹交替。被同名真实条目替换时序号 1:1 传承,
            // 行相位不跳变;仅取消且未落地时后续行移一次相位,与任何
            // 普通行增删一致。
            *stripe_index += 1;
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
    stripe_index: &mut usize,
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
            stripe_index: *stripe_index,
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
    Some(
        crate::file_entry_presentation::SelectionRunPosition::from_neighbors(
            previous_selected,
            next_selected,
        ),
    )
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
        if let ListTransferRow::Entry { visible, .. } = row {
            if visible.entry.path == path {
                return Some((offset, height));
            }
        }
        offset += height;
    }
    None
}

/// 光标落点 → 条目路径:沿行流按 `row.height` 累计命中 Entry 行,与
/// 虚拟范围/reveal 用同一份高度口径,滚动帧的 hover 补偿重算才有与
/// 渲染一致的落点。组头/状态行/占位行不是条目,命中即返回 None;
/// 落点越过最后一行(表尾空白)同样为 None。
pub(crate) fn list_entry_path_at_point<'a>(
    rows: &'a [ListTransferRow<'a>],
    row_height: f32,
    header_height: f32,
    point: Point,
) -> Option<&'a Path> {
    if row_height <= f32::EPSILON {
        return None;
    }
    let mut offset = header_height;
    for row in rows {
        let height = row.height(row_height);
        if let ListTransferRow::Entry { visible, .. } = row {
            if point.y >= offset && point.y < offset + height {
                return Some(visible.entry.path.as_path());
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

/// 分组索引栏条目:沿合并行流按 `row.height` 累计遇到组头行记下
/// "组短标签 → 组头顶点偏移"。与虚拟范围/内容高/reveal 用同一份行高
/// 口径,派生结果与渲染天然对齐,不另建第二份分组状态。
pub(crate) fn list_file_group_rail_entries(
    rows: &[ListTransferRow],
    row_height: f32,
) -> Vec<crate::model::FileGroupRailEntry> {
    let mut offset = 0.0;
    let mut entries = Vec::new();
    for row in rows {
        if let ListTransferRow::GroupHeader { index_label, .. } = row {
            entries.push(crate::model::FileGroupRailEntry {
                index_label: index_label.clone(),
                top_offset: offset,
            });
        }
        offset += row.height(row_height);
    }
    entries
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
    let index = merged.iter().position(
        |item| matches!(item, MergedTransferItem::Entry { entry, .. } if entry.path == path),
    )?;
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
    entries.len() + transfer_placeholders_for_directory(&browser.operation_queue, directory).len()
}

#[cfg(test)]
#[path = "transfer_placeholder_view/root_grouping_tests.rs"]
mod root_grouping_tests;

#[cfg(test)]
#[path = "transfer_placeholder_view/stripe_index_tests.rs"]
mod stripe_index_tests;

#[cfg(test)]
#[path = "transfer_placeholder_view/row_stream_tests.rs"]
mod tests;
