use file_core::{batch_rename_paths, BatchRenameItem, FileOperationControls};

use crate::operation_history::FileOperationOutcome;

pub(super) async fn run_queued_batch_rename(
    items: Vec<BatchRenameItem>,
    mut controls: FileOperationControls,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    let renames = batch_rename_paths(items)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::BatchRename { renames })
}
