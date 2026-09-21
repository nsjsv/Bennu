use file_operation_store::{StoredApplicationShutdown, TaskQueueStore};
use iced::Task;

use crate::model::Message;

pub(crate) fn commit_application_shutdown_command(
    store: Option<TaskQueueStore>,
    shutdown: StoredApplicationShutdown,
) -> Task<Message> {
    Task::perform(
        async move {
            let store = match store {
                Some(store) => Some(store),
                None if shutdown.user_preferences.is_some() => Some(
                    TaskQueueStore::new(crate::config::default_state_database_path())
                        .map_err(|error| error.to_string())?,
                ),
                None => None,
            };
            let Some(store) = store else {
                if shutdown.interrupted_recoverable_tasks.is_empty()
                    && shutdown.transient_task_ids.is_empty()
                    && matches!(
                        shutdown.browser_session,
                        file_operation_store::StoredBrowserSessionShutdown::Skip
                    )
                {
                    return Ok(());
                }
                return Err("operation store is unavailable during application shutdown".to_owned());
            };
            tokio::task::spawn_blocking(move || store.commit_application_shutdown(shutdown))
                .await
                .map_err(|error| format!("application shutdown worker failed: {error}"))?
                .map_err(|error| error.to_string())
        },
        Message::ApplicationShutdownPersisted,
    )
}
