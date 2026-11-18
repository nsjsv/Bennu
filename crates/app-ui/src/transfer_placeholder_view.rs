//! 传输占位行的渲染层:占位不进入目录数据模型,只在视图把真实条目流
//! 与队列派生的占位合并时插入。所有行都是纯静态容器——不包 button/
//! mouse_area、不发 Message、不注册拖拽命中,因此天然不可选中、不可
//! 交互,键盘导航按路径匹配时自动跳过。

use std::ffi::OsStr;
use std::path::Path;

use bennu_preview::entry_icons::file_entry_icon_symbol;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::measured_text::{
    measured_middle_ellipsized_centered_text, measured_middle_ellipsized_text,
};
use file_core::DirectoryEntry;
use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::appearance::list_row_style;
use crate::file_entry_view::{file_entry_symbol_icon, FileEntryIconDensity, FileEntryIconTone};
use crate::list_view::ListGeometry;
use crate::model::{
    DirectoryOrderPhase, ExpandedDirectory, FileEntryContentModifier, ListColumnConfig,
    ListColumnKind, Message,
};
use crate::progress_ring::progress_ring;
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, transfer_placeholders_for_directory, MergedTransferItem,
    TransferPlaceholder, TransferSortOptions,
};

/// 根目录分组划分:目录置顶 + 文件段按组分段,列表/大图的当前目录
/// 共用(网格侧经 icon_grid_layout 的分组入参消费)。
#[path = "transfer_placeholder_view/root_grouping.rs"]
pub(crate) mod root_grouping;

#[path = "transfer_placeholder_view/list_row_layout.rs"]
mod list_row_layout;
#[cfg(test)]
pub(crate) use list_row_layout::ListRowLayout;
pub(crate) use list_row_layout::ListTransferRow;
#[path = "transfer_placeholder_view/list_rows.rs"]
mod list_rows;
pub(crate) use list_rows::{list_row_layout, ListRowCache};

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
        progress_ring(transfer.progress.unwrap_or(0.0)),
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
        progress_ring(placeholder.progress.unwrap_or(0.0)),
    ])
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size))
    .into()
}

fn transfer_placeholder_name_text(
    placeholder: &TransferPlaceholder,
    text_size: f32,
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
                transfer_placeholder_name_text(placeholder, geometry.text_size),
            ]
            .spacing(geometry.row_spacing)
            .align_y(Alignment::Center)
            .width(list_column_width(column.width))
            .into(),
            _ => container(readable_text("-".to_owned()).size(geometry.text_size))
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
            geometry.text_size
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
/// directories_first 沿用根目录的(全局扫描选项)。
pub(crate) fn transfer_sort_for_expanded(
    root: TransferSortOptions,
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
        directories_first: root.directories_first,
    }
}

/// 任一 pane 目录的排序配置:当前目录用扫描选项,其余目录用各自展开
/// 目录的 directory_order_phase。三栏视图与 reveal 数学共用此解析,
/// 保证插入排序与渲染同源。
pub(crate) fn transfer_sort_for_pane_directory(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    directory: &Path,
) -> TransferSortOptions {
    if pane.current_directory() == Some(directory) {
        return transfer_sort_for_root(browser);
    }
    pane.expanded_directories
        .get(directory)
        .map(|expanded| transfer_sort_for_expanded(transfer_sort_for_root(browser), expanded))
        .unwrap_or_else(|| transfer_sort_for_root(browser))
}

fn column_directory_entries<'a>(
    pane: BrowserPaneView<'a>,
    directory: &Path,
) -> Option<&'a [DirectoryEntry]> {
    if pane.current_directory() == Some(directory) {
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
        |item| matches!(item, MergedTransferItem::Entry { entry, .. } if entry.path() == path),
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
