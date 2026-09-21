use file_core::FileOperationVerification;
use iced::Element;

use crate::model::Message;

use super::settings_group::toggle_setting_row;

pub(super) fn file_operation_verification_options(
    selected: FileOperationVerification,
) -> Element<'static, Message> {
    let is_strong = selected == FileOperationVerification::Strong;
    let next_verification = if is_strong {
        FileOperationVerification::BasicMetadata
    } else {
        FileOperationVerification::Strong
    };

    toggle_setting_row(
        "Strong Verification",
        Some("Compare copied file content hashes after standard metadata checks."),
        is_strong,
        Message::FileOperationVerificationSelected(next_verification),
    )
}
