use iced::widget::column;
use iced::{Element, Length};

use crate::app::archive_creation::{
    archive_compression_level_label, archive_format_label, ArchiveCreationMessage,
    ArchiveCreationState, ARCHIVE_COMPRESSION_LEVELS, ARCHIVE_FORMATS,
};
use crate::app::archive_password::ArchivePasswordDraft;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;

use super::dialog::{self, Dialog};
use bennu_theme::controls::text_field;
use bennu_theme::controls::{normal_button, suggested_button};
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};

pub(super) fn archive_creation_panel(state: &ArchiveCreationState) -> Element<'_, Message> {
    let name_input = text_field(&tr!("Archive name"), state.file_name())
        .on_input(|name| Message::ArchiveCreation(ArchiveCreationMessage::NameChanged(name)))
        .on_submit(Message::ArchiveCreation(ArchiveCreationMessage::Submitted))
        .width(Length::Fill);
    // 名字下面：要建出来的文件名；名字不行时换成原因。
    let name_note = match (state.validation_error(), state.target_path()) {
        (Some(problem), _) => dialog::error(problem.to_owned()),
        (None, Err(problem)) => dialog::error(problem),
        (None, Ok(target)) => dialog::hint(tr!("Creates %1", dialog::path_name(&target))),
    };
    Dialog::new(
        tr!("Create Archive"),
        dialog::fields(vec![
            dialog::field(tr!("Name"), column![name_input, name_note].spacing(6)),
            dialog::field(tr!("Format"), archive_format_buttons(state)),
            dialog::field(tr!("Compression"), archive_compression_buttons(state)),
            archive_password_input(state),
        ]),
    )
    .subtitle(sources_into_target(state))
    .actions(archive_creation_actions(state))
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
    let mut control = column![password_input].spacing(6);
    if !state.password_supported() {
        control = control.push(dialog::hint(tr!(
            "tar.gz archives do not support passwords."
        )));
    }
    dialog::field(tr!("Password"), control)
}

fn archive_creation_actions(state: &ArchiveCreationState) -> Vec<Element<'_, Message>> {
    let create_label = if state.is_checking_target() {
        tr!("Checking...")
    } else {
        tr!("Create")
    };
    vec![
        normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::DismissFloating)
            .into(),
        suggested_button(readable_text(create_label))
            .on_press_maybe(
                state
                    .can_submit()
                    .then_some(Message::ArchiveCreation(ArchiveCreationMessage::Submitted)),
            )
            .into(),
    ]
}

fn sources_into_target(state: &ArchiveCreationState) -> String {
    let sources = match state.sources() {
        [] => tr!("No selected items"),
        [source] => dialog::path_name(source),
        sources => tr!("%1 items", sources.len()),
    };
    dialog::into_subtitle(&sources, state.target_directory())
}
