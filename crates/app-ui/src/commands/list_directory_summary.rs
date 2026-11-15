use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::directory_summary::{
    measure_directories, read_directory_contents_summary, DirectorySummaryError, MeasureFailure,
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
    let direct_path = path.clone();
    let contents = tokio::task::spawn_blocking(move || {
        read_directory_contents_summary(&direct_path, CancellationToken::new(), |_| {})
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(directory_summary_error_message)?;
    let recursive_total_size = if include_recursive_total_size {
        let usage = measure_directories(&[path], &CancellationToken::new(), |_| {})
            .await
            .map_err(|failure| match failure {
                MeasureFailure::Cancelled => "operation cancelled".to_owned(),
                MeasureFailure::Unreadable(_, error) => error,
            })?;
        Some(usage.into())
    } else {
        None
    };
    Ok(ListDirectorySummary {
        direct_child_count: contents.total_item_count(),
        recursive_total_size,
        files_total_size_bytes: Some(contents.files_total_size_bytes),
        hidden_files_total_size_bytes: Some(contents.hidden_files_total_size_bytes),
        hidden_entry_count: Some(contents.hidden_entry_count),
    })
}

fn directory_summary_error_message(error: DirectorySummaryError) -> String {
    match error {
        DirectorySummaryError::Cancelled => "operation cancelled".to_owned(),
        DirectorySummaryError::Io(error) => error.to_string(),
        DirectorySummaryError::Overflow(field) => format!("directory summary {field} overflowed"),
    }
}
