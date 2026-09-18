//! 包内成员批量提取的队列执行：复制/拖出到真实目录的落地动作。

use std::path::PathBuf;

use file_core::{
    extract_archive_members_with_controls_and_progress, ArchiveMemberExtractionRequest,
    FileOperationControls,
};
use iced::futures::SinkExt;

use crate::model::Message;
use crate::operation_history::FileOperationOutcome;
use crate::operation_progress::FileOperationProgressUpdate;

type IcedSender = iced::futures::channel::mpsc::Sender<Message>;

pub(crate) async fn run_queued_extract_archive_members(
    sources: Vec<PathBuf>,
    destination: PathBuf,
    controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender,
) -> Result<FileOperationOutcome, String> {
    let _ = output
        .send(Message::FileOperationProgressed(
            task_id,
            FileOperationProgressUpdate::Indeterminate,
            Vec::new(),
        ))
        .await;
    let request = ArchiveMemberExtractionRequest::new(sources, destination);
    extract_archive_members_with_controls_and_progress(request, controls, |_| {})
        .await
        .map(|_| FileOperationOutcome::NoHistory)
        .map_err(|error| error.to_string())
}
