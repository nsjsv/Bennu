//! 树形预览面板：文件夹和压缩包。行高固定，只排看得见的行（上下多排几行余量），
//! 上下用空白撑住滚动区高度，几万条也不卡；每行缩进、箭头、图标、名字、大小，够宽
//! 再加修改时间；底下一行总计（像状态栏，列表滚到哪都在原处）。滚动接线（smooth-scroll 包装、scrollable id、视口回传）
//! 经 ScrollRegionWiring 由宿主注入。

use std::ops::Range;

use iced::widget::{column, container, mouse_area, row, text, Column, Space};
use iced::{Alignment, Element, Length};

use crate::engine::PreviewLoadSurface;
use crate::entry_icons::preview_entry_icon_symbol;
use crate::folder_preview::{
    FolderNode, FolderPreview, FolderPreviewMessage, FolderRow, FolderUsage,
};
use crate::media_keys::folder_keys;
use bennu_theme::icons::{icon_tone_style, rotated_chevron_right_view, themed_icon, IconTone};
use bennu_theme::measured_text::format_middle_ellipsized_text;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, selected_row_style,
};

use crate::formatting::format_file_size;
use crate::panel_text::readable_text;
use crate::preview::{ArchiveSummary, PreviewSize, PreviewTreeEntry, TreeViewport};
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use bennu_localization::tr;
use bennu_theme::styles::muted_text_color;
use bennu_theme::typography::CAPTION_SIZE;

const PREVIEW_ICON_SIZE: f32 = 16.0;
const PREVIEW_ENTRY_NAME_MAX_CHARS: usize = 48;
const PREVIEW_TREE_INDENT_WIDTH: f32 = 18.0;
const PREVIEW_TREE_TOGGLE_WIDTH: f32 = 16.0;
const PREVIEW_TREE_TOGGLE_ROTATION_DEGREES: f32 = 90.0;
/// 每行固定高：按行高算哪些行看得见，键盘选中后按它把行滚进视口。
pub(crate) const TREE_ROW_HEIGHT: f32 = 28.0;
/// 看得见的上下再多排几行，滚得快时不露白。
const TREE_OVERSCAN_ROWS: usize = 20;
const TREE_SIZE_WIDTH: f32 = 72.0;
const TREE_DATE_WIDTH: f32 = 112.0;
/// 比这窄（右侧面板）不显示修改时间，留给名字。
const TREE_DATE_MIN_PANEL_WIDTH: f32 = 520.0;

/// 文件夹：底下一行总计（全部层，边算边更新）；行上文件显示大小，子文件夹显示里面
/// 总大小（算完前空着）；点箭头展开收起，点行选中，双击打开。键盘只在独立预览窗口
/// 里接——右侧面板的方向键归主列表。
pub(crate) fn folder_preview_panel<Message>(
    folder: &FolderPreview,
    size: PreviewSize,
    surface: PreviewLoadSurface,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let scroll_region_id = scroll.scrollable_id.clone();
    let listing = if folder.entries().is_empty() {
        Column::new().push(readable_text(tr!("Empty directory")))
    } else {
        let rows = folder.visible_rows();
        let show_dates = size.width >= TREE_DATE_MIN_PANEL_WIDTH;
        virtual_listing(rows.len(), folder.viewport, |index| match rows[index] {
            FolderRow::Entry { node, depth } => folder_entry_row(
                node,
                depth,
                folder.selected() == Some(node.path.as_path()),
                show_dates,
            ),
            FolderRow::Reading { depth } => status_row(depth, tr!("Reading…")),
            FolderRow::Failed { depth, error } => {
                status_row(depth, tr!("Could not load: %1", error))
            }
        })
    };
    let panel: Element<'static, Message> = Column::new()
        .push(tree_scroller(listing, scroll))
        .push(summary_row(folder_summary_text(folder.summary())))
        .spacing(6)
        .into();
    match surface {
        PreviewLoadSurface::StandaloneWindow => folder_keys(panel, scroll_region_id),
        PreviewLoadSurface::RightDockedPanel => panel,
    }
}

/// 「120 个文件 · 8 个文件夹 · 共 2.3 GiB」；还在算时前面写「正在计算」，有读不了的写「≥」。
fn folder_summary_text(summary: FolderUsage) -> String {
    let usage = summary.usage;
    let mut parts = vec![tr!("%1 files", usage.files)];
    if usage.folders > 0 {
        parts.push(tr!("%1 folders", usage.folders));
    }
    parts.push(tr!("%1 in total", folder_size_text(summary)));
    let line = parts.join(" · ");
    if summary.settled {
        line
    } else {
        tr!("Calculating… %1", line)
    }
}

/// 文件夹的大小：读不了的部分没算进来时写「≥」。
fn folder_size_text(usage: FolderUsage) -> String {
    let size = format_file_size(usage.usage.bytes);
    if usage.usage.unreadable > 0 {
        format!("≥ {size}")
    } else {
        size
    }
}

fn folder_entry_row<Message>(
    node: &FolderNode,
    depth: usize,
    selected: bool,
    show_dates: bool,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let folder_message = |message| Message::from(PreviewMessage::FolderPreview(message));
    let size = match node.folder.as_deref() {
        None => format_file_size(node.len),
        Some(state) if state.usage.settled => folder_size_text(state.usage),
        Some(_) => String::new(),
    };
    let mut details = vec![detail_text(size, TREE_SIZE_WIDTH)];
    if show_dates {
        details.push(detail_text(
            node.modified.map(format_local_minute).unwrap_or_default(),
            TREE_DATE_WIDTH,
        ));
    }
    let toggle = node.folder.as_deref().map(|state| {
        mouse_area(rotation_chevron(state.rotation))
            .on_press(folder_message(FolderPreviewMessage::Toggled(
                node.path.clone(),
            )))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    });
    let content = tree_row_content(depth, toggle, node.kind, &node.name, details);
    let mut row_container = container(content)
        .padding([3, 6])
        .width(Length::Fill)
        .height(Length::Fixed(TREE_ROW_HEIGHT))
        .center_y(Length::Fixed(TREE_ROW_HEIGHT));
    if selected {
        row_container = row_container.style(selected_row_style);
    }
    mouse_area(row_container)
        .on_press(folder_message(FolderPreviewMessage::Selected(
            node.path.clone(),
        )))
        .on_double_click(Message::from(PreviewMessage::FolderEntryOpened {
            path: node.path.clone(),
            kind: node.kind,
        }))
        .into()
}

/// 压缩包：底下一行总计；每个条目带大小（文件夹是里面加起来）和修改时间（够宽才显示）；
/// 几万条也只排看得见的那些行，上下用空白顶住滚动区的高度。
pub(crate) fn archive_preview_panel<Message>(
    entries: &[PreviewTreeEntry],
    summary: &ArchiveSummary,
    viewport: TreeViewport,
    size: PreviewSize,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    if entries.is_empty() {
        return tree_scroller(
            Column::new().push(readable_text(tr!("Empty archive"))),
            scroll,
        );
    }
    let visible = visible_preview_tree_entries(entries);
    let show_dates = size.width >= TREE_DATE_MIN_PANEL_WIDTH;
    let listing = virtual_listing(visible.len(), viewport, |index| {
        let entry = visible[index];
        let mut details = vec![detail_text(
            entry.len.map(format_file_size).unwrap_or_default(),
            TREE_SIZE_WIDTH,
        )];
        if show_dates {
            details.push(detail_text(
                entry.modified.map(format_local_minute).unwrap_or_default(),
                TREE_DATE_WIDTH,
            ));
        }
        archive_entry_row(entry, details)
    });
    Column::new()
        .push(tree_scroller(listing, scroll))
        .push(summary_row(archive_summary_text(summary)))
        .spacing(6)
}

/// 「1234 个文件 · 56 个文件夹 · 解压后 2.3 GiB · 压缩到 31%」。
fn archive_summary_text(summary: &ArchiveSummary) -> String {
    let mut parts = vec![tr!("%1 files", summary.files)];
    if summary.folders > 0 {
        parts.push(tr!("%1 folders", summary.folders));
    }
    parts.push(tr!("%1 unpacked", format_file_size(summary.unpacked_bytes)));
    if summary.unpacked_bytes > 0 && summary.archive_bytes > 0 {
        let percent =
            (summary.archive_bytes as f64 / summary.unpacked_bytes as f64 * 100.0).round();
        parts.push(tr!("packed to %1%", percent));
    }
    parts.join(" · ")
}

fn summary_row<Message: 'static>(line: String) -> Element<'static, Message> {
    container(text(line).size(CAPTION_SIZE).style(|theme| text::Style {
        color: Some(muted_text_color(theme)),
    }))
    .padding([2, 6])
    .into()
}

/// `total` 行里只排 `viewport` 看得见的（加余量），上下用空白顶住滚动高度。
fn virtual_listing<Message: 'static>(
    total: usize,
    viewport: TreeViewport,
    mut build_row: impl FnMut(usize) -> Element<'static, Message>,
) -> Column<'static, Message> {
    let range = visible_row_range(total, viewport);
    let mut listing = Column::new();
    if range.start > 0 {
        listing =
            listing.push(Space::new().height(Length::Fixed(range.start as f32 * TREE_ROW_HEIGHT)));
    }
    for index in range.clone() {
        listing = listing.push(build_row(index));
    }
    if range.end < total {
        listing = listing
            .push(Space::new().height(Length::Fixed((total - range.end) as f32 * TREE_ROW_HEIGHT)));
    }
    listing
}

fn visible_row_range(total: usize, viewport: TreeViewport) -> Range<usize> {
    let first = ((viewport.offset / TREE_ROW_HEIGHT).floor().max(0.0) as usize)
        .saturating_sub(TREE_OVERSCAN_ROWS)
        .min(total);
    let last = (((viewport.offset + viewport.height) / TREE_ROW_HEIGHT)
        .ceil()
        .max(0.0) as usize)
        .saturating_add(TREE_OVERSCAN_ROWS)
        .min(total);
    first..last
}

fn detail_text<Message: 'static>(content: String, width: f32) -> Element<'static, Message> {
    text(content)
        .size(CAPTION_SIZE)
        .width(Length::Fixed(width))
        .align_x(iced::alignment::Horizontal::Right)
        .style(|theme| text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

/// 「2024-05-01 12:34」，本机时区（跟文件列表一样跟着系统的时区）。
fn format_local_minute(time: std::time::SystemTime) -> String {
    format_minute_in(time, &chrono::Local)
}

fn format_minute_in<Zone: chrono::TimeZone>(time: std::time::SystemTime, zone: &Zone) -> String
where
    Zone::Offset: std::fmt::Display,
{
    chrono::DateTime::<chrono::Utc>::from(time)
        .with_timezone(zone)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// 目录/归档树共用滚动容器：填满剩余高度的 scrollable + 常驻细线滑块；
/// 滚轮惯性包装来自宿主持有的滚动管线。
fn tree_scroller<Message>(
    listing: Column<'static, Message>,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone,
{
    let scroller = scroll.scrollable(listing.into(), |scroller| {
        enhanced_scrollbar(
            scroller
                .direction(enhanced_vertical_scrollbar_direction())
                .style(enhanced_scrollbar_style)
                .height(Length::Fill),
            ScrollbarAxis::Vertical,
        )
    });

    column![scroller]
}

fn visible_preview_tree_entries(entries: &[PreviewTreeEntry]) -> Vec<&PreviewTreeEntry> {
    entries
        .iter()
        .filter(|entry| preview_tree_entry_visible(entry, entries))
        .collect()
}

fn preview_tree_entry_visible(entry: &PreviewTreeEntry, entries: &[PreviewTreeEntry]) -> bool {
    let mut parent = entry.parent;
    while let Some(parent_id) = parent {
        let Some(parent_entry) = entries.get(parent_id) else {
            return false;
        };
        if !(parent_entry.is_expanded || parent_entry.toggle_rotation_progress > 0.0) {
            return false;
        }
        parent = parent_entry.parent;
    }

    true
}

fn archive_entry_row<Message>(
    entry: &PreviewTreeEntry,
    details: Vec<Element<'static, Message>>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let toggle = entry
        .is_directory()
        .then(|| rotation_chevron(entry.toggle_rotation_progress));
    let row_container = container(tree_row_content(
        entry.depth,
        toggle,
        entry.kind,
        &entry.name,
        details,
    ))
    .padding([3, 6])
    .width(Length::Fill)
    .height(Length::Fixed(TREE_ROW_HEIGHT))
    .center_y(Length::Fixed(TREE_ROW_HEIGHT));
    if entry.is_directory() {
        mouse_area(row_container)
            .on_press(Message::from(PreviewMessage::PreviewTreeDirectoryToggled(
                entry.id,
            )))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    } else {
        row_container.into()
    }
}

/// 缩进、箭头（文件夹才有，没有就留同宽的空）、图标、名字，后面跟着 `details`。
fn tree_row_content<Message: 'static>(
    depth: usize,
    toggle: Option<Element<'static, Message>>,
    kind: file_core::FileKind,
    name: &str,
    details: Vec<Element<'static, Message>>,
) -> Element<'static, Message> {
    let indent = Space::new().width(Length::Fixed(depth as f32 * PREVIEW_TREE_INDENT_WIDTH));
    let toggle = toggle.unwrap_or_else(|| {
        Space::new()
            .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
            .into()
    });
    let mut content = row![
        indent,
        toggle,
        themed_icon(
            preview_entry_icon_symbol(kind, name),
            IconTone::Normal,
            PREVIEW_ICON_SIZE,
        ),
        readable_text(format_middle_ellipsized_text(
            name,
            PREVIEW_ENTRY_NAME_MAX_CHARS
        ))
        .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    for detail in details {
        content = content.push(detail);
    }
    content.into()
}

fn rotation_chevron<Message: 'static>(rotation: f32) -> Element<'static, Message> {
    container(
        rotated_chevron_right_view(
            rotation * PREVIEW_TREE_TOGGLE_ROTATION_DEGREES,
            PREVIEW_TREE_TOGGLE_WIDTH,
        )
        .style(icon_tone_style(IconTone::Normal)),
    )
    .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
    .height(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
    .center_x(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
    .center_y(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
    .into()
}

/// 展开的文件夹下面那一行状态（在读、读不了），缩进到孩子那一层，淡色。
fn status_row<Message: 'static>(depth: usize, message: String) -> Element<'static, Message> {
    let indent = Space::new().width(Length::Fixed(
        depth as f32 * PREVIEW_TREE_INDENT_WIDTH + PREVIEW_TREE_TOGGLE_WIDTH + 6.0,
    ));
    container(
        row![
            indent,
            text(format_middle_ellipsized_text(
                &message,
                PREVIEW_ENTRY_NAME_MAX_CHARS
            ))
            .size(CAPTION_SIZE)
            .style(|theme| text::Style {
                color: Some(muted_text_color(theme)),
            }),
        ]
        .align_y(Alignment::Center),
    )
    .padding([3, 6])
    .width(Length::Fill)
    .height(Length::Fixed(TREE_ROW_HEIGHT))
    .center_y(Length::Fixed(TREE_ROW_HEIGHT))
    .into()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::format_minute_in;

    #[test]
    fn archive_times_read_as_minutes_in_the_zone() {
        let utc = |time| format_minute_in(time, &chrono::Utc);
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01 00:00");
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_secs(1_714_566_840)),
            "2024-05-01 12:34"
        );
        assert_eq!(
            utc(UNIX_EPOCH - Duration::from_secs(60)),
            "1969-12-31 23:59"
        );
        assert_eq!(
            format_minute_in(
                UNIX_EPOCH,
                &chrono::FixedOffset::east_opt(8 * 3600).unwrap()
            ),
            "1970-01-01 08:00"
        );
    }
}
