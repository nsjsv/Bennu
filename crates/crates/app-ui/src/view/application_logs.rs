use iced::widget::{column, container, Column};
use iced::{Element, Length};

use crate::app::FileBrowser;
use crate::formatting::format_system_time;
use crate::model::{
    ApplicationLogLevel, Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility,
};
use crate::typography::readable_text;

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::option_controls::{segmented_choice_row, SegmentedChoice};
use super::settings_group::{info_setting_row, settings_group, SETTINGS_GROUP_SPACING};
use super::settings_window::chrome_top_spacer;

pub(super) fn application_logs_settings_detail(
    browser: &FileBrowser,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let mut content = column![
        chrome_top_spacer(browser),
        settings_group(
            "Display level",
            vec![info_setting_row(log_threshold_choices(
                browser.application_logs.threshold,
            ))],
        )
    ]
    .spacing(SETTINGS_GROUP_SPACING)
    .width(Length::Fill);

    let mut entries = Column::new().spacing(4).width(Length::Fill);
    if let Some(warning) = browser.application_logs.journald_warning.as_ref() {
        entries = entries.push(readable_text(warning).size(12).width(Length::Fill));
    }
    if let Some(error) = browser.application_logs.load_error.as_ref() {
        entries = entries.push(readable_text(error).size(12).width(Length::Fill));
    }
    if browser.application_logs.is_loading() {
        entries = entries.push(readable_text("Loading logs...").size(12));
    }

    let mut visible_count = 0;
    for entry in browser.application_logs.visible_entries() {
        visible_count += 1;
        let level = crate::localization::translate_current(entry.level.label());
        let source = crate::localization::translate_current(entry.source.label());
        let metadata = format!(
            "{}  ·  {level}  ·  {source}",
            format_system_time(entry.timestamp)
        );
        let log_entry = column![
            readable_text(metadata).size(11),
            readable_text(&entry.message).size(12).width(Length::Fill),
        ]
        .spacing(2)
        .width(Length::Fill);
        entries = entries.push(container(log_entry).padding([6, 0]).width(Length::Fill));
    }
    if visible_count == 0
        && !browser.application_logs.is_loading()
        && browser.application_logs.load_error.is_none()
    {
        entries = entries.push(readable_text("No logs for this level.").size(13));
    }
    content = content.push(entries);

    auxiliary_detail_scroller(
        content,
        ScrollbarRegion::Settings,
        scrollbar_visibility,
        scrollbar_viewport,
        Message::SettingsScrolled,
    )
}

fn log_threshold_choices(selected: ApplicationLogLevel) -> Element<'static, Message> {
    segmented_choice_row(
        ApplicationLogLevel::ALL
            .into_iter()
            .map(|level| SegmentedChoice {
                label: level.label(),
                selected: level == selected,
                message: Message::ApplicationLogThresholdSelected(level),
                tooltip: None,
            })
            .collect(),
    )
}
