use file_operation_store::TaskQueueStore;

use crate::model::{snapshot_to_stored, BrowserSessionSnapshot};

pub(super) async fn persist_browser_session(
    task_queue_store: TaskQueueStore,
    snapshot: BrowserSessionSnapshot,
) -> Result<(), String> {
    let Some(stored) = snapshot_to_stored(snapshot) else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || task_queue_store.replace_browser_session(&stored))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}
