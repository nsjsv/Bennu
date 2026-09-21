use file_core::{resolve_directory_metadata, DirectoryMetadataRequest, DirectoryMetadataResolver};
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::model::{DirectoryMetadataLoadFailure, DirectoryMetadataLoadRequest, Message};

pub(crate) fn load_directory_metadata_command(
    request: DirectoryMetadataLoadRequest,
    resolver: DirectoryMetadataResolver,
    cancellation: CancellationToken,
) -> Task<Message> {
    let core_request = DirectoryMetadataRequest {
        request_generation: request.request_generation,
        requirement: request.requirement,
        targets: request.targets.clone(),
    };
    Task::perform(
        async move {
            match resolve_directory_metadata(resolver, core_request, cancellation).await {
                Ok(resolution) => Ok(resolution),
                Err(file_core::FileError::Cancelled) => {
                    Err(DirectoryMetadataLoadFailure::Cancelled)
                }
                Err(error) => Err(DirectoryMetadataLoadFailure::ReadFailed(error.to_string())),
            }
        },
        move |outcome| Message::DirectoryMetadataResolved(request, outcome),
    )
}
