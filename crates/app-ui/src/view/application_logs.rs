use iced::widget::column;
use iced::{Element, Length};

use crate::app::FileBrowser;
use crate::formatting::format_system_time;
use crate::model::{ApplicationLogLevel, Message};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::SegmentedChoice;
use bennu_theme::typography::CAPTION_SIZE;

use super::settings_group::{
    choice_row, custom_row, notice_row, settings_card, RowNote, SettingsRow,
};

/// 日志页：级别选择一张卡，日志条目一张卡（每条一行）。
pub(super) fn application_logs_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let logs = &browser.application_logs;
    let mut rows: Vec<SettingsRow<'_>> = Vec::new();
    if let Some(warning) = logs.journald_warning.as_ref() {
        rows.push(notice_row(RowNote::Hint(warning.clone())));
    }
    if let Some(error) = logs.load_error.as_ref() {
        rows.push(notice_row(RowNote::Error(error.clone())));
    }
    if logs.is_loading() {
        rows.push(notice_row(RowNote::Hint(tr!("Loading logs..."))));
    }
    let entry_count = rows.len();
    rows.extend(logs.visible_entries().map(|entry| {
        let metadata = format!(
            "{}  ·  {}  ·  {}",
            format_system_time(entry.timestamp),
            entry.level.label(),
            entry.source.label()
        );
        custom_row(
            column![
                readable_text(metadata).size(CAPTION_SIZE),
                readable_text(entry.message.clone()).width(Length::Fill),
            ]
            .spacing(2)
            .width(Length::Fill),
        )
    }));
    let has_entries = rows.len() > entry_count;
    if !has_entries && !logs.is_loading() && logs.load_error.is_none() {
        rows.push(notice_row(RowNote::Hint(tr!("No logs for this level."))));
    }

    vec![
        settings_card(vec![log_threshold_row(logs.threshold)]),
        settings_card(rows),
    ]
}

fn log_threshold_row(selected: ApplicationLogLevel) -> SettingsRow<'static> {
    choice_row(
        tr!("Display level"),
        None,
        ApplicationLogLevel::ALL
            .into_iter()
            .map(|level| SegmentedChoice {
                label: level.label(),
                selected: level == selected,
                message: Some(Message::ApplicationLogThresholdSelected(level)),
                tooltip: None,
            })
            .collect(),
    )
}
