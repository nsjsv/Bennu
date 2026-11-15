//! 目录/归档预览树面板：缩进条目行、目录展开切换、加载/错误状态行。
//! 自 app-ui view/preview_panel.rs 下沉；滚动接线（smooth-scroll 包装/
//! scrollable id/视口回传）经 ScrollRegionWiring 由宿主注入。

use iced::widget::{column, container, mouse_area, row, text, Column, Space};
use iced::{Alignment, Element, Length};

use crate::entry_icons::preview_entry_icon_symbol;
use bennu_theme::icons::{icon_tone_style, rotated_chevron_right_view, themed_icon, IconTone};
use bennu_theme::measured_text::format_middle_ellipsized_text;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};

use crate::formatting::format_file_size;
use crate::panel_text::readable_text;
use crate::preview::{
    ArchiveSummary, PreviewSize, PreviewTreeDirectoryChildren, PreviewTreeEntry, TreeViewport,
};
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
/// 压缩包每行固定高：按行高算哪些行看得见。
const ARCHIVE_ROW_HEIGHT: f32 = 28.0;
/// 看得见的上下再多排几行，滚得快时不露白。
const ARCHIVE_OVERSCAN_ROWS: usize = 20;
const ARCHIVE_SIZE_WIDTH: f32 = 72.0;
const ARCHIVE_DATE_WIDTH: f32 = 112.0;
/// 比这窄（右侧面板）不显示修改时间，留给名字。
const ARCHIVE_DATE_MIN_PANEL_WIDTH: f32 = 520.0;

pub(crate) fn directory_preview_panel<Message>(
    entries: &[PreviewTreeEntry],
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let listing = preview_tree_listing(entries, tr!("Empty directory"));
    tree_scroller(listing, scroll)
}

/// 压缩包：顶上一行总计；每个条目带大小（文件夹是里面加起来）和修改时间（够宽才显示）；
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
    let total = visible.len();
    let first = ((viewport.offset / ARCHIVE_ROW_HEIGHT).floor().max(0.0) as usize)
        .saturating_sub(ARCHIVE_OVERSCAN_ROWS)
        .min(total);
    let last = (((viewport.offset + viewport.height) / ARCHIVE_ROW_HEIGHT)
        .ceil()
        .max(0.0) as usize)
        .saturating_add(ARCHIVE_OVERSCAN_ROWS)
        .min(total);
    let show_dates = size.width >= ARCHIVE_DATE_MIN_PANEL_WIDTH;
    let mut listing = Column::new();
    if first > 0 {
        listing =
            listing.push(Space::new().height(Length::Fixed(first as f32 * ARCHIVE_ROW_HEIGHT)));
    }
    for entry in &visible[first..last] {
        let mut details = vec![detail_text(
            entry.len.map(format_file_size).unwrap_or_default(),
            ARCHIVE_SIZE_WIDTH,
        )];
        if show_dates {
            details.push(detail_text(
                entry.modified.map(format_utc_minute).unwrap_or_default(),
                ARCHIVE_DATE_WIDTH,
            ));
        }
        listing = listing.push(
            container(tree_entry_row(entry, details))
                .height(Length::Fixed(ARCHIVE_ROW_HEIGHT))
                .center_y(Length::Fixed(ARCHIVE_ROW_HEIGHT)),
        );
    }
    if last < total {
        listing = listing
            .push(Space::new().height(Length::Fixed((total - last) as f32 * ARCHIVE_ROW_HEIGHT)));
    }
    Column::new()
        .push(archive_summary_row(summary))
        .push(tree_scroller(listing, scroll))
        .spacing(6)
}

/// 「1234 个文件 · 56 个文件夹 · 解压后 2.3 GB · 压缩到 31%」。
fn archive_summary_row<Message: 'static>(summary: &ArchiveSummary) -> Element<'static, Message> {
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
    container(
        text(parts.join(" · "))
            .size(CAPTION_SIZE)
            .style(|theme| text::Style {
                color: Some(muted_text_color(theme)),
            }),
    )
    .padding([2, 6])
    .into()
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

/// 「2024-05-01 12:34」（UTC，和文件列表一样不换时区）。
fn format_utc_minute(time: std::time::SystemTime) -> String {
    let seconds = match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let (days, seconds_of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // 公历日期由距 1970-01-01 的天数推算（按 400 年一个周期）。
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60
    )
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
    let scroller = scroll
        .scrollable(listing.into())
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .height(Length::Fill);

    column![enhanced_scrollbar(scroller, ScrollbarAxis::Vertical)]
}

fn preview_tree_listing<Message>(
    entries: &[PreviewTreeEntry],
    empty_message: String,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let mut listing = Column::new().spacing(3);
    if entries.is_empty() {
        return listing.push(readable_text(empty_message));
    }

    for entry in visible_preview_tree_entries(entries) {
        listing = listing.push(tree_entry_row(entry, Vec::new()));
        if let Some(message) = preview_tree_directory_status_message(entry) {
            listing = listing.push(preview_tree_status_row(entry, message));
        }
    }

    listing
}

fn preview_tree_directory_status_message(entry: &PreviewTreeEntry) -> Option<String> {
    if !entry.is_expanded {
        return None;
    }

    match entry.directory_children.as_ref()? {
        PreviewTreeDirectoryChildren::Error(error) => Some(tr!("Could not load: %1", error)),
        PreviewTreeDirectoryChildren::Loading
        | PreviewTreeDirectoryChildren::Pending
        | PreviewTreeDirectoryChildren::Loaded => None,
    }
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

/// 一行：缩进、展开箭头、图标、名字，后面跟着 `details`（压缩包的大小、时间）。
fn tree_entry_row<Message>(
    entry: &PreviewTreeEntry,
    details: Vec<Element<'static, Message>>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let name = format_middle_ellipsized_text(&entry.name, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        entry.depth as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let toggle: Element<'static, Message> = if entry.is_directory() {
        container(
            rotated_chevron_right_view(
                entry.toggle_rotation_progress * PREVIEW_TREE_TOGGLE_ROTATION_DEGREES,
                PREVIEW_TREE_TOGGLE_WIDTH,
            )
            .style(icon_tone_style(IconTone::Normal)),
        )
        .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .height(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_x(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_y(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .into()
    } else {
        Space::new()
            .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
            .into()
    };
    let mut row_content = row![
        indent,
        toggle,
        themed_icon(
            preview_entry_icon_symbol(entry.kind, &entry.name),
            IconTone::Normal,
            PREVIEW_ICON_SIZE,
        ),
        readable_text(name).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    for detail in details {
        row_content = row_content.push(detail);
    }
    let row_container = container(row_content).padding([3, 6]).width(Length::Fill);

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

fn preview_tree_status_row<Message>(
    entry: &PreviewTreeEntry,
    message: String,
) -> Element<'static, Message>
where
    Message: 'static,
{
    let message = format_middle_ellipsized_text(&message, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        (entry.depth + 1) as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let row_content = row![
        indent,
        Space::new().width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH)),
        Space::new().width(Length::Fixed(PREVIEW_ICON_SIZE)),
        readable_text(message).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(row_content)
        .padding([3, 6])
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::format_utc_minute;

    #[test]
    fn archive_times_read_as_utc_minutes() {
        assert_eq!(format_utc_minute(UNIX_EPOCH), "1970-01-01 00:00");
        assert_eq!(
            format_utc_minute(UNIX_EPOCH + Duration::from_secs(1_714_566_840)),
            "2024-05-01 12:34"
        );
        assert_eq!(
            format_utc_minute(UNIX_EPOCH - Duration::from_secs(60)),
            "1969-12-31 23:59"
        );
    }
}
