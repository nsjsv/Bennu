use file_core::{inspect_archive_extraction, ArchiveExtractionRequest, FileError};
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::app::archive_extraction::{ArchiveExtractionInspection, ArchiveExtractionMessage};
use crate::model::Message;

pub(crate) fn inspect_archive_extraction_command(
    request: ArchiveExtractionRequest,
) -> Task<Message> {
    let issued_request = request.clone();
    Task::perform(
        async move {
            inspect_archive_extraction(request, CancellationToken::new())
                .await
                .map(|_| ArchiveExtractionInspection::Ready)
                .unwrap_or_else(archive_extraction_inspection_from_error)
        },
        move |outcome| {
            Message::ArchiveExtraction(ArchiveExtractionMessage::Inspected {
                request: issued_request.clone(),
                outcome,
            })
        },
    )
}

fn archive_extraction_inspection_from_error(error: FileError) -> ArchiveExtractionInspection {
    match error {
        FileError::ArchivePasswordRequired { .. } => ArchiveExtractionInspection::PasswordRequired,
        FileError::ArchiveInvalidPassword { .. } => ArchiveExtractionInspection::InvalidPassword,
        error => ArchiveExtractionInspection::Failed(error.to_string()),
    }
}
