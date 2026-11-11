//! LocalSend 窗口"记录"页：传输记录列表（新的在前）+ 清空。每行：收/发、设备、文件名
//! 或文字开头、大小、时间、结果；能打开的（有位置的文件、文字）整行可点。

use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local};
use iced::widget::{column, row, Column, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{Direction, HistoryItem, HistoryRecord, HistoryResult, TransferMessage};
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;

use super::settings_group::muted_setting_text;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::normal_button;
use bennu_theme::typography::CAPTION_SIZE;

const NAME_MAX_CHARS: usize = 40;
const TEXT_PREVIEW_CHARS: usize = 40;
const RESULT_MAX_CHARS: usize = 60;

pub(super) fn history_list(browser: &FileBrowser) -> Element<'_, Message> {
    let records = &browser.transfer_history.records;
    let clear = normal_button(readable_text(tr!("Clear history"))).on_press_maybe(
        (!records.is_empty()).then_some(Message::Transfer(TransferMessage::HistoryClearPressed)),
    );
    let header = row![
        readable_text(tr!("Transfer history")).width(Length::Fill),
        clear
    ]
    .align_y(Alignment::Center);
    let mut list: Column<'_, Message> = column![header].spacing(6).width(Length::Fill);
    if records.is_empty() {
        return list
            .push(muted_setting_text(tr!("Nothing sent or received yet."), 11))
            .into();
    }
    let now = SystemTime::now();
    for record in records {
        list = list.push(history_row(record, now));
    }
    list.into()
}

fn history_row(record: &HistoryRecord, now: SystemTime) -> Element<'_, Message> {
    let icon = match record.direction {
        Direction::Sent => IconSymbol::ArrowUp,
        Direction::Received => IconSymbol::ArrowDown,
    };
    // 设备名、文件名、文字是用户内容，不做本地化。
    let mut summary = vec![record.device.clone()];
    let (name, openable) = match &record.item {
        HistoryItem::File {
            name,
            path,
            size,
            more,
        } => {
            if let Some(size) = size {
                summary.push(format_file_size(*size));
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
    summary.push(record_time(record.at, now));
    let details = column![
        readable_text(name),
        muted_setting_text(summary.join(" · "), 11),
    ]
    .spacing(2)
    .width(Length::Fill);
    let content = row![
        themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE),
        details,
        Space::new().width(Length::Fixed(6.0)),
        result_text(&record.result),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    normal_button(content)
        .on_press_maybe(
            openable.then(|| {
                Message::Transfer(TransferMessage::HistoryRecordOpened(record.id.clone()))
            }),
        )
        .width(Length::Fill)
        .into()
}

fn result_text(result: &HistoryResult) -> Element<'static, Message> {
    let (label, failed) = match result {
        HistoryResult::Done => (tr!("Completed"), false),
        HistoryResult::Canceled => (tr!("Canceled"), false),
        HistoryResult::Declined => (tr!("The device declined."), true),
        HistoryResult::TooManyPinAttempts => (tr!("Too many wrong PINs"), true),
        HistoryResult::Failed(reason) => (
            format_middle_ellipsized_text(&tr!("Failed: %1", reason), RESULT_MAX_CHARS),
            true,
        ),
    };
    readable_text(label)
        .size(CAPTION_SIZE)
        .style(move |theme: &Theme| iced::widget::text::Style {
            color: Some(if failed {
                ui_colors(theme).error
            } else {
                muted_text_color(theme)
            }),
        })
        .into()
}

/// 今天"14:05"、昨天"昨天 14:05"、今年"11-08 14:05"、更早"2025-11-08"（本地时间）。
pub(crate) fn record_time(at: SystemTime, now: SystemTime) -> String {
    let at: DateTime<Local> = at.into();
    let now: DateTime<Local> = now.into();
    let (day, today) = (at.date_naive(), now.date_naive());
    if day == today {
        at.format("%H:%M").to_string()
    } else if today.pred_opt() == Some(day) {
        tr!("Yesterday %1", at.format("%H:%M"))
    } else if at.year() == now.year() {
        at.format("%m-%d %H:%M").to_string()
    } else {
        at.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> SystemTime {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .unwrap()
            .into()
    }

    #[test]
    fn times_get_shorter_the_closer_they_are() {
        let now = local(2026, 11, 10, 15, 0);

        assert_eq!(record_time(local(2026, 11, 10, 14, 5), now), "14:05");
        assert_eq!(
            record_time(local(2026, 11, 9, 9, 30), now),
            tr!("Yesterday %1", "09:30")
        );
        assert_eq!(record_time(local(2026, 11, 8, 23, 1), now), "11-08 23:01");
        assert_eq!(record_time(local(2025, 11, 8, 23, 1), now), "2025-11-08");
    }
}
