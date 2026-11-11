//! LocalSend 窗口"记录"页：传输记录按天分组（新的在前），每天一张卡片。每条：发出 / 收到
//! 图标、文件名或文字开头、“发给 / 来自 设备 · 大小”、时间；没成的才在时间下面写结果。
//! 能打开的（有位置的文件、文字）整行可点。标题和“清空”在窗口顶栏上。

use chrono::{DateTime, Datelike, Local, NaiveDate};
use iced::widget::{button, column, container, row, Column};
use iced::{alignment, Alignment, Background, Border, Element, Length, Theme};

use crate::app::transfer::{Direction, HistoryItem, HistoryRecord, HistoryResult, TransferMessage};
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::CARD_RADIUS;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};
use bennu_theme::ui_colors;

use super::local_send_tiles::centered_muted_text;
use super::settings_group::{card_row_button_style, muted_setting_text};
use super::{themed_icon, IconTone};

const NAME_MAX_CHARS: usize = 34;
const TEXT_PREVIEW_CHARS: usize = 34;
const RESULT_MAX_CHARS: usize = 24;
const DAY_SPACING: f32 = 18.0;
const ROW_ICON_CIRCLE: f32 = 36.0;
const ROW_ICON_SIZE: f32 = 16.0;
const EMPTY_ICON_SIZE: f32 = 28.0;

pub(super) fn history_list(browser: &FileBrowser) -> Element<'_, Message> {
    let records = &browser.transfer_history.records;
    if records.is_empty() {
        return column![
            themed_icon(IconSymbol::History, IconTone::Normal, EMPTY_ICON_SIZE),
            centered_muted_text(tr!("Nothing sent or received yet.")),
        ]
        .spacing(10)
        .padding([48, 0])
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into();
    }
    let today = Local::now().date_naive();
    let mut days: Column<'_, Message> = column![].spacing(DAY_SPACING).width(Length::Fill);
    let mut current: Option<(NaiveDate, Column<'_, Message>)> = None;
    for record in records {
        let at: DateTime<Local> = record.at.into();
        let day = at.date_naive();
        if current.as_ref().map(|(current_day, _)| *current_day) != Some(day) {
            if let Some((day, rows)) = current.take() {
                days = days.push(day_group(day, today, rows));
            }
            current = Some((day, column![].spacing(2).width(Length::Fill)));
        }
        if let Some((_, rows)) = current.take() {
            current = Some((day, rows.push(history_row(record, at))));
        }
    }
    if let Some((day, rows)) = current {
        days = days.push(day_group(day, today, rows));
    }
    days.into()
}

/// 一天：小标题 + 一张卡片。
fn day_group<'a>(
    day: NaiveDate,
    today: NaiveDate,
    rows: Column<'a, Message>,
) -> Element<'a, Message> {
    column![
        container(
            readable_text(day_label(day, today))
                .size(CAPTION_SIZE)
                .font(BOLD)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text_color(theme)),
                }),
        )
        .padding([0, 4]),
        container(rows)
            .padding(4)
            .width(Length::Fill)
            .style(day_card_style),
    ]
    .spacing(6)
    .into()
}

fn history_row(record: &HistoryRecord, at: DateTime<Local>) -> Element<'_, Message> {
    let icon = match record.direction {
        Direction::Sent => IconSymbol::ArrowUp,
        Direction::Received => IconSymbol::ArrowDown,
    };
    // 设备名、文件名、文字是用户内容，不做本地化。
    let mut subtitle = vec![match record.direction {
        Direction::Sent => tr!("To %1", record.device),
        Direction::Received => tr!("From %1", record.device),
    }];
    let (name, openable) = match &record.item {
        HistoryItem::File {
            name,
            path,
            size,
            more,
        } => {
            if let Some(size) = size {
                subtitle.push(format_file_size(*size));
            }
            let name = format_middle_ellipsized_text(name, NAME_MAX_CHARS);
            let name = match more {
                0 => name,
                more => tr!("%1 and %2 more", name, more),
            };
            (name, path.is_some() && *more == 0)
        }
        HistoryItem::Text(text) => {
            let preview: String = text.chars().take(TEXT_PREVIEW_CHARS).collect();
            let ellipsis = if text.chars().count() > TEXT_PREVIEW_CHARS {
                "…"
            } else {
                ""
            };
            (format!("\u{201c}{preview}{ellipsis}\u{201d}"), true)
        }
    };
    let circle = container(themed_icon(icon, IconTone::Normal, ROW_ICON_SIZE))
        .center(Length::Fixed(ROW_ICON_CIRCLE))
        .style(icon_circle_style);
    let mut trailing = column![muted_caption(at.format("%H:%M").to_string())]
        .spacing(2)
        .align_x(alignment::Horizontal::Right);
    if let Some(result) = result_text(&record.result) {
        trailing = trailing.push(result);
    }
    let content = row![
        circle,
        column![
            readable_text(name),
            muted_setting_text(subtitle.join(" · ")),
        ]
        .spacing(2)
        .width(Length::Fill),
        trailing,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    button(container(content).padding([8, 10]))
        .on_press_maybe(
            openable.then(|| {
                Message::Transfer(TransferMessage::HistoryRecordOpened(record.id.clone()))
            }),
        )
        .padding(0)
        .width(Length::Fill)
        .style(card_row_button_style())
        .into()
}

/// 只有没成的写结果；正常完成是默认，不在每条上重复“已完成”。
fn result_text(result: &HistoryResult) -> Option<Element<'static, Message>> {
    let (label, failed) = match result {
        HistoryResult::Done => return None,
        HistoryResult::Canceled => (tr!("Canceled"), false),
        HistoryResult::Declined => (tr!("Declined"), true),
        HistoryResult::TooManyPinAttempts => (tr!("Too many wrong PINs"), true),
        HistoryResult::Failed(reason) => (
            format_middle_ellipsized_text(&tr!("Failed: %1", reason), RESULT_MAX_CHARS),
            true,
        ),
    };
    Some(
        readable_text(label)
            .size(CAPTION_SIZE)
            .style(move |theme: &Theme| iced::widget::text::Style {
                color: Some(if failed {
                    ui_colors(theme).error
                } else {
                    muted_text_color(theme)
                }),
            })
            .into(),
    )
}

fn muted_caption(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

/// 今天、昨天；今年“11-08”；更早“2025-11-08”（本地时间）。
fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        tr!("Today")
    } else if today.pred_opt() == Some(day) {
        tr!("Yesterday")
    } else if day.year() == today.year() {
        day.format("%m-%d").to_string()
    } else {
        day.format("%Y-%m-%d").to_string()
    }
}

fn day_card_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container)),
        border: Border {
            radius: CARD_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

fn icon_circle_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container_high)),
        border: Border {
            radius: (ROW_ICON_CIRCLE / 2.0).into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn day_headers_get_shorter_the_closer_they_are() {
        let today = date(2026, 11, 10);

        assert_eq!(day_label(date(2026, 11, 10), today), tr!("Today"));
        assert_eq!(day_label(date(2026, 11, 9), today), tr!("Yesterday"));
        assert_eq!(day_label(date(2026, 11, 8), today), "11-08");
        assert_eq!(day_label(date(2025, 11, 8), today), "2025-11-08");
    }
}
