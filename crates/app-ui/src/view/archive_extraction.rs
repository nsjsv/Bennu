use iced::{Element, Length};

use super::dialog::{self, Dialog};
use crate::app::archive_extraction::{ArchiveExtractionMessage, ArchiveExtractionState};
use crate::app::archive_password::ArchivePasswordDraft;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::text_field;
use bennu_theme::controls::{normal_button, suggested_button};

pub(super) fn archive_extraction_panel(state: &ArchiveExtractionState) -> Element<'_, Message> {
    let mut body = dialog::fields(vec![dialog::hint(status_text(state))]);
    if state.is_waiting_for_password() || state.is_checking_password() {
        body = body.push(archive_password_input(state));
    }
    if let Some(problem) = state.validation_error() {
        body = body.push(dialog::error(problem.to_owned()));
    }
    let request = state.request();
    Dialog::new(tr!("Extract Archive"), body)
        .subtitle(dialog::into_subtitle(
            &dialog::path_name(&request.archive),
            &request.destination,
        ))
        .actions(archive_extraction_actions(state))
        .into()
}

fn archive_password_input(state: &ArchiveExtractionState) -> Element<'_, Message> {
    archive_password_input_field(
        state.password(),
        state.is_waiting_for_password(),
        |password| Message::ArchiveExtraction(ArchiveExtractionMessage::PasswordChanged(password)),
        Message::ArchiveExtraction(ArchiveExtractionMessage::Submitted),
    )
}

/// 整包解压与成员密码弹窗共享的密码输入框:同一套样式与提交行为,
/// 消息由调用方闭包各自包装,避免两处弹窗各抄一份输入框。
pub(super) fn archive_password_input_field(
    draft: &ArchivePasswordDraft,
    editable: bool,
    on_changed: impl Fn(ArchivePasswordDraft) -> Message + 'static,
    on_submitted: Message,
) -> Element<'_, Message> {
    let mut password_input = text_field(&tr!("Password"), draft.as_str())
        .secure(true)
        .width(Length::Fill);
    if editable {
        password_input = password_input
            .on_input(move |password| on_changed(ArchivePasswordDraft::new(password)))
            .on_submit(on_submitted);
    }

    dialog::field(tr!("Password"), password_input)
}

fn archive_extraction_actions(state: &ArchiveExtractionState) -> Vec<Element<'_, Message>> {
    let extract_label = if state.is_checking_password() {
        tr!("Checking...")
    } else if state.is_inspecting() {
        tr!("Checking")
    } else {
        tr!("Extract")
    };
    let can_extract = state.is_waiting_for_password() && state.can_submit_password();
    vec![
        normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::DismissFloating)
            .into(),
        suggested_button(readable_text(extract_label))
            .on_press_maybe(can_extract.then_some(Message::ArchiveExtraction(
                ArchiveExtractionMessage::Submitted,
            )))
            .into(),
    ]
}

fn status_text(state: &ArchiveExtractionState) -> String {
    if state.is_checking_password() {
        tr!("Checking password...")
    } else if state.is_waiting_for_password() {
        tr!("Enter the archive password to continue.")
    } else {
        tr!("Checking archive...")
    }
}
