use std::path::Path;

use file_core::{convert_file_with_controls, ConversionRequest, FileError, FileOperationControls};

use file_operations::{
    FileOperationEvent, FileOperationEventSender, FileOperationOutcome, FileOperationProgressUpdate,
};

pub(super) async fn run_queued_convert(
    requests: Vec<ConversionRequest>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &FileOperationEventSender,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;

    let total_items = requests.len();
    let mut failures = Vec::new();
    for (index, request) in requests.into_iter().enumerate() {
        if let Err(error) = convert_file_with_controls(request, &controls).await {
            match error {
                // 取消/停机让整个任务终止,由队列按取消语义收敛。
                FileError::Cancelled | FileError::ApplicationStopping => {
                    return Err(error.to_string());
                }
                other => failures.push(format_conversion_failure(&other)),
            }
        }
        send_convert_progress(output, task_id, index + 1, total_items).await;
    }

    // 一个都没成功按失败处理;部分失败保持成功 + warning 清单。
    if total_items > 0 && failures.len() == total_items {
        return Err(failures.join("; "));
    }
    Ok(FileOperationOutcome::Convert { failures })
}

fn format_conversion_failure(error: &FileError) -> String {
    match error {
        FileError::Convert { path, message } | FileError::InvalidInput { path, message } => {
            format!("{}: {}", source_display(path), message)
        }
        other => other.to_string(),
    }
}

fn source_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

async fn send_convert_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    completed: usize,
    total: usize,
) {
    let _ = output
        .send(FileOperationEvent::Progressed {
            task_id,
            update: FileOperationProgressUpdate::IndeterminateItems { completed, total },
            entries: Vec::new(),
        })
        .await;
}
