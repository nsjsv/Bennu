use std::path::PathBuf;

use file_core::is_transfer_target_available;
use iced::Task;

use crate::app::archive_creation::{ArchiveCreationMessage, ArchiveCreationState};
use crate::model::Message;

pub(crate) fn check_archive_target_command(
    state: ArchiveCreationState,
    target: PathBuf,
) -> Task<Message> {
    let issued_state = state.clone();
    let issued_target = target.clone();
    Task::perform(
        async move {
            is_transfer_target_available(target)
                .await
                .map_err(|error| error.to_string())
        },
        move |available| {
            Message::ArchiveCreation(ArchiveCreationMessage::TargetChecked {
                state: issued_state.clone(),
                target: issued_target.clone(),
                available,
            })
        },
    )
}
