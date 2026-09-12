use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::directory_summary::{
    read_directory_contents_summary, read_directory_recursive_total_size,
};
use crate::model::{ListDirectorySummary, ListDirectorySummaryLoadRequest, Message};

pub(crate) fn load_list_directory_summary_command(
    request: ListDirectorySummaryLoadRequest,
) -> Task<Message> {
    let issued_request = request.clone();
    Task::perform(
        load_list_directory_summary(request.path, request.include_recursive_total_size),
        move |outcome| Message::ListDirectorySummaryLoaded(issued_request.clone(), outcome),
    )
}

async fn load_list_directory_summary(
    path: PathBuf,
    include_recursive_total_size: bool,
) -> Result<ListDirectorySummary, String> {
    tokio::task::spawn_blocking(move || {
        let cancellation = CancellationToken::new();
        let contents = read_directory_contents_summary(&path, cancellation.clone(), |_| {})
            .map_err(directory_summary_error_message)?;
        let recursive_total_size_bytes = if include_recursive_total_size {
            Some(
                read_directory_recursive_total_size(&path, cancellation)
                    .map_err(directory_summary_error_message)?,
            )
        } else {
            None
        };
        Ok(ListDirectorySummary {
            direct_child_count: contents.total_item_count(),
            recursive_total_size_bytes,
            files_total_size_bytes: Some(contents.files_total_size_bytes),
            hidden_files_total_size_bytes: Some(contents.hidden_files_total_size_bytes),
            hidden_entry_count: Some(contents.hidden_entry_count),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn directory_summary_error_message(
    error: crate::directory_summary::DirectorySummaryError,
) -> String {
    match error {
        crate::directory_summary::DirectorySummaryError::Cancelled => {
            "operation cancelled".to_owned()
        }
        crate::directory_summary::DirectorySummaryError::Io(error) => error.to_string(),
        crate::directory_summary::DirectorySummaryError::Overflow(field) => {
            format!("directory summary {field} overflowed")
        }
    }
}
