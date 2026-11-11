use file_core::FileOperationVerification;

use crate::model::Message;

use super::settings_group::{switch_row, RowNote, SettingsRow};

pub(super) fn file_operation_verification_options(
    selected: FileOperationVerification,
) -> SettingsRow<'static> {
    let is_strong = selected == FileOperationVerification::Strong;
    let next_verification = if is_strong {
        FileOperationVerification::BasicMetadata
    } else {
        FileOperationVerification::Strong
    };

    switch_row(
        tr!("Strong Verification"),
        Some(RowNote::Hint(tr!(
            "Compare copied file content hashes after standard metadata checks."
        ))),
        is_strong,
        Message::FileOperationVerificationSelected(next_verification),
    )
}
