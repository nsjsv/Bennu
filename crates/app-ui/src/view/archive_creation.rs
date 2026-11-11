use iced::widget::{column, container, row, Column, Space};
use iced::{Alignment, Element, Length};

use crate::app::archive_creation::{
    archive_compression_level_label, archive_format_label, ArchiveCreationMessage,
    ArchiveCreationState, ARCHIVE_COMPRESSION_LEVELS, ARCHIVE_FORMATS,
};
use crate::app::archive_password::ArchivePasswordDraft;
use crate::appearance::context_menu_style;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::text_field;
use bennu_theme::controls::{normal_button, suggested_button};
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};
use bennu_theme::typography::{BOLD, TITLE_SIZE};

const ARCHIVE_CREATION_PANEL_WIDTH: f32 = 500.0;
const ARCHIVE_PATH_MAX_CHARS: usize = 72;

pub(super) fn archive_creation_panel(state: &ArchiveCreationState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileArchive, IconTone::Normal, MENU_ICON_SIZE),
        readable_text(tr!("Create Archive"))
            .size(TITLE_SIZE)
            .font(BOLD)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let name_input = text_field(&tr!("Archive name"), state.file_name())
        .on_input(|name| Message::ArchiveCreation(ArchiveCreationMessage::NameChanged(name)))
        .on_submit(Message::ArchiveCreation(ArchiveCreationMessage::Submitted))
        .width(Length::Fill);
    let validation_error = state
        .validation_error()
        .map(str::to_owned)
        .unwrap_or_default();

    let content = column![
        title,
        readable_text(source_summary(state)),
        readable_text(target_directory_summary(state)),
        column![readable_text(tr!("Name")), name_input].spacing(4),
        column![readable_text(tr!("Format")), archive_format_buttons(state)].spacing(4),
        column![
            readable_text(tr!("Compression")),
            archive_compression_buttons(state)
        ]
        .spacing(4),
        archive_password_input(state),
        readable_text(target_summary(state)),
        readable_text(validation_error),
        archive_creation_actions(state),
    ]
    .spacing(10)
    .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(ARCHIVE_CREATION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn archive_format_buttons(state: &ArchiveCreationState) -> Element<'_, Message> {
    segmented_choice_row(
        ARCHIVE_FORMATS
            .iter()
            .copied()
            .map(|format| SegmentedChoice {
                label: archive_format_label(format),
                selected: format == state.format(),
                message: Some(Message::ArchiveCreation(
                    ArchiveCreationMessage::FormatSelected(format),
                )),
                tooltip: None,
            })
            .collect(),
    )
}

fn archive_compression_buttons(state: &ArchiveCreationState) -> Element<'_, Message> {
    segmented_choice_row(
        ARCHIVE_COMPRESSION_LEVELS
            .iter()
            .copied()
            .map(|level| SegmentedChoice {
                label: archive_compression_level_label(level),
                selected: level == state.compression_level(),
                message: Some(Message::ArchiveCreation(
                    ArchiveCreationMessage::CompressionLevelSelected(level),
                )),
                tooltip: None,
            })
            .collect(),
    )
}

fn archive_password_input(state: &ArchiveCreationState) -> Element<'_, Message> {
    let mut password_input = text_field(&tr!("Optional password"), state.password().as_str())
        .secure(true)
        .width(Length::Fill);
    if state.password_supported() {
        password_input = password_input
            .on_input(|password| {
                Message::ArchiveCreation(ArchiveCreationMessage::PasswordChanged(
                    ArchivePasswordDraft::new(password),
                ))
            })
            .on_submit(Message::ArchiveCreation(ArchiveCreationMessage::Submitted));
    }

    let mut field = Column::new()
        .spacing(4)
        .push(readable_text(tr!("Password")))
        .push(password_input);
    if !state.password_supported() {
        field = field.push(readable_text(tr!(
            "tar.gz archives do not support passwords."
        )));
    }
    field.into()
}

fn archive_creation_actions(state: &ArchiveCreationState) -> Element<'_, Message> {
    let create_label = if state.is_checking_target() {
        tr!("Checking...")
    } else {
        tr!("Create")
    };
    let create_button = if state.can_submit() {
        suggested_button(readable_text(create_label))
            .on_press(Message::ArchiveCreation(ArchiveCreationMessage::Submitted))
    } else {
        suggested_button(readable_text(create_label))
    };

    row![
        Space::new().width(Length::Fill),
        normal_button(readable_text(tr!("Cancel"))).on_press(Message::DismissFloating),
        create_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn source_summary(state: &ArchiveCreationState) -> String {
    match state.sources() {
        [] => tr!("No selected items"),
        [source] => tr!(
            "Source: %1",
            format_middle_ellipsized_text(
                source.to_string_lossy().as_ref(),
                ARCHIVE_PATH_MAX_CHARS
            )
        ),
        sources => tr!("Sources: %1 items", sources.len()),
    }
}

fn target_directory_summary(state: &ArchiveCreationState) -> String {
    tr!(
        "Destination: %1",
        format_middle_ellipsized_text(
            state.target_directory().to_string_lossy().as_ref(),
            ARCHIVE_PATH_MAX_CHARS,
        )
    )
}

fn target_summary(state: &ArchiveCreationState) -> String {
    match state.target_path() {
        Ok(target) => tr!(
            "Target: %1",
            format_middle_ellipsized_text(
                target.to_string_lossy().as_ref(),
                ARCHIVE_PATH_MAX_CHARS
            )
        ),
        Err(error) => error,
    }
}
