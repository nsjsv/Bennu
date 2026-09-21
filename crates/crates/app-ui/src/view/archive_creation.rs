use iced::widget::{column, container, row, text_input, Column, Space};
use iced::{Alignment, Element, Length};

use crate::app::archive_creation::{
    archive_compression_level_label, archive_format_label, ArchiveCreationMessage,
    ArchiveCreationState, ARCHIVE_COMPRESSION_LEVELS, ARCHIVE_FORMATS,
};
use crate::app::archive_password::ArchivePasswordDraft;
use crate::appearance::context_menu_style;
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::{localized_text, readable_text};

use super::option_controls::{
    inactive_primary_action_button, primary_action_button, secondary_action_button,
    segmented_choice_row, SegmentedChoice,
};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const ARCHIVE_CREATION_PANEL_WIDTH: f32 = 500.0;
const ARCHIVE_PATH_MAX_CHARS: usize = 72;

pub(super) fn archive_creation_panel(state: &ArchiveCreationState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileArchive, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("Create Archive").size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let name_input = text_input(
        &crate::localization::translate_current("Archive name"),
        state.file_name(),
    )
    .on_input(|name| Message::ArchiveCreation(ArchiveCreationMessage::NameChanged(name)))
    .on_submit(Message::ArchiveCreation(ArchiveCreationMessage::Submitted))
    .padding([6, 8])
    .size(14)
    .width(Length::Fill);
    let validation_error = state
        .validation_error()
        .map(str::to_owned)
        .unwrap_or_default();

    let content = column![
        title,
        readable_text(source_summary(state)).size(12),
        readable_text(target_directory_summary(state)).size(12),
        column![readable_text("Name").size(12), name_input].spacing(4),
        column![
            readable_text("Format").size(12),
            archive_format_buttons(state)
        ]
        .spacing(4),
        column![
            readable_text("Compression").size(12),
            archive_compression_buttons(state)
        ]
        .spacing(4),
        archive_password_input(state),
        readable_text(target_summary(state)).size(12),
        localized_text(validation_error).size(12),
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
                message: Message::ArchiveCreation(ArchiveCreationMessage::FormatSelected(format)),
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
                message: Message::ArchiveCreation(
                    ArchiveCreationMessage::CompressionLevelSelected(level),
                ),
                tooltip: None,
            })
            .collect(),
    )
}

fn archive_password_input(state: &ArchiveCreationState) -> Element<'_, Message> {
    let mut password_input = text_input(
        &crate::localization::translate_current("Optional password"),
        state.password().as_str(),
    )
    .secure(true)
    .padding([6, 8])
    .size(14)
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
        .push(readable_text("Password").size(12))
        .push(password_input);
    if !state.password_supported() {
        field = field.push(readable_text("tar.gz archives do not support passwords.").size(12));
    }
    field.into()
}

fn archive_creation_actions(state: &ArchiveCreationState) -> Element<'_, Message> {
    let create_label = if state.is_checking_target() {
        "Checking..."
    } else {
        "Create"
    };
    let create_button = if state.can_submit() {
        primary_action_button(
            create_label,
            Message::ArchiveCreation(ArchiveCreationMessage::Submitted),
        )
    } else {
        inactive_primary_action_button(create_label)
    };

    row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::DismissFloating),
        create_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn source_summary(state: &ArchiveCreationState) -> String {
    match state.sources() {
        [] => crate::localization::translate_current("No selected items"),
        [source] => {
            let source = format_middle_ellipsized_text(
                source.to_string_lossy().as_ref(),
                ARCHIVE_PATH_MAX_CHARS,
            );
            if crate::localization::current_language_is_chinese() {
                format!("来源：{source}")
            } else {
                format!("Source: {source}")
            }
        }
        sources => {
            if crate::localization::current_language_is_chinese() {
                format!("来源：{} 个项目", sources.len())
            } else {
                format!("Sources: {} items", sources.len())
            }
        }
    }
}

fn target_directory_summary(state: &ArchiveCreationState) -> String {
    let destination = format_middle_ellipsized_text(
        state.target_directory().to_string_lossy().as_ref(),
        ARCHIVE_PATH_MAX_CHARS,
    );
    if crate::localization::current_language_is_chinese() {
        format!("目标目录：{destination}")
    } else {
        format!("Destination: {destination}")
    }
}

fn target_summary(state: &ArchiveCreationState) -> String {
    match state.target_path() {
        Ok(target) => {
            let target = format_middle_ellipsized_text(
                target.to_string_lossy().as_ref(),
                ARCHIVE_PATH_MAX_CHARS,
            );
            if crate::localization::current_language_is_chinese() {
                format!("目标文件：{target}")
            } else {
                format!("Target: {target}")
            }
        }
        Err(error) => error,
    }
}
