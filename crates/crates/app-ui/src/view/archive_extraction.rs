use iced::widget::{column, container, row, text_input, Column, Space};
use iced::{Alignment, Element, Length};

use crate::app::archive_extraction::{ArchiveExtractionMessage, ArchiveExtractionState};
use crate::app::archive_password::ArchivePasswordDraft;
use crate::appearance::context_menu_style;
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::{localized_text, readable_text};

use super::option_controls::{
    inactive_primary_action_button, primary_action_button, secondary_action_button,
};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const ARCHIVE_EXTRACTION_PANEL_WIDTH: f32 = 500.0;
const ARCHIVE_PATH_MAX_CHARS: usize = 72;

pub(super) fn archive_extraction_panel(state: &ArchiveExtractionState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileArchive, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("Extract Archive")
            .size(16)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let validation_error = state
        .validation_error()
        .map(str::to_owned)
        .unwrap_or_default();

    let content = if state.is_waiting_for_password() || state.is_checking_password() {
        column![
            title,
            readable_text(archive_summary(state)).size(12),
            readable_text(destination_summary(state)).size(12),
            readable_text(status_text(state)).size(13),
            archive_password_input(state),
            localized_text(validation_error).size(12),
            archive_extraction_actions(state),
        ]
    } else {
        column![
            title,
            readable_text(archive_summary(state)).size(12),
            readable_text(destination_summary(state)).size(12),
            readable_text(status_text(state)).size(13),
            localized_text(validation_error).size(12),
            archive_extraction_actions(state),
        ]
    }
    .spacing(10)
    .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(ARCHIVE_EXTRACTION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn archive_password_input(state: &ArchiveExtractionState) -> Element<'_, Message> {
    let mut password_input = text_input(
        &crate::localization::translate_current("Password"),
        state.password().as_str(),
    )
    .secure(true)
    .padding([6, 8])
    .size(14)
    .width(Length::Fill);
    if state.is_waiting_for_password() {
        password_input = password_input
            .on_input(|password| {
                Message::ArchiveExtraction(ArchiveExtractionMessage::PasswordChanged(
                    ArchivePasswordDraft::new(password),
                ))
            })
            .on_submit(Message::ArchiveExtraction(
                ArchiveExtractionMessage::Submitted,
            ));
    }

    Column::new()
        .spacing(4)
        .push(readable_text("Password").size(12))
        .push(password_input)
        .into()
}

fn archive_extraction_actions(state: &ArchiveExtractionState) -> Element<'_, Message> {
    let extract_label = if state.is_checking_password() {
        "Checking..."
    } else if state.is_inspecting() {
        "Checking"
    } else {
        "Extract"
    };
    let extract_button = if state.is_waiting_for_password() && state.can_submit_password() {
        primary_action_button(
            extract_label,
            Message::ArchiveExtraction(ArchiveExtractionMessage::Submitted),
        )
    } else {
        inactive_primary_action_button(extract_label)
    };

    row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::DismissFloating),
        extract_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn archive_summary(state: &ArchiveExtractionState) -> String {
    let archive = format_middle_ellipsized_text(
        state.request().archive.to_string_lossy().as_ref(),
        ARCHIVE_PATH_MAX_CHARS,
    );
    if crate::localization::current_language_is_chinese() {
        format!("归档：{archive}")
    } else {
        format!("Archive: {archive}")
    }
}

fn destination_summary(state: &ArchiveExtractionState) -> String {
    let destination = format_middle_ellipsized_text(
        state.request().destination.to_string_lossy().as_ref(),
        ARCHIVE_PATH_MAX_CHARS,
    );
    if crate::localization::current_language_is_chinese() {
        format!("目标目录：{destination}")
    } else {
        format!("Destination: {destination}")
    }
}

fn status_text(state: &ArchiveExtractionState) -> &'static str {
    if state.is_checking_password() {
        "Checking password..."
    } else if state.is_waiting_for_password() {
        "Enter the archive password to continue."
    } else {
        "Checking archive..."
    }
}
