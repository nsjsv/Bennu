use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::directory_summary::{count_directory_children, measure_directories, MeasureFailure};
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
    let direct_child_count =
        tokio::task::spawn_blocking(move || count_directory_children(&direct_path))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
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
        direct_child_count,
        recursive_total_size,
    })
}
