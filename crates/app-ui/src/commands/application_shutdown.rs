use bennu_state::{StateDatabase, StoredApplicationShutdown};
use iced::Task;

use crate::model::Message;

pub(crate) fn commit_application_shutdown_command(
    database: Option<StateDatabase>,
    shutdown: StoredApplicationShutdown<crate::config::StoredUserPreferences>,
) -> Task<Message> {
    Task::perform(
        async move {
            let database = match database {
                Some(database) => Some(database),
                None if shutdown.user_preferences.is_some() => Some(
                    StateDatabase::open(crate::config::default_state_database_path())
                        .map_err(|error| error.to_string())?,
                ),
                None => None,
            };
            let Some(database) = database else {
                if shutdown.interrupted_recoverable_tasks.is_empty()
                    && shutdown.transient_task_ids.is_empty()
                    && matches!(
                        shutdown.browser_session,
                        bennu_state::StoredBrowserSessionShutdown::Skip
                    )
                {
                    return Ok(());
                }
                return Err("operation store is unavailable during application shutdown".to_owned());
            };
            tokio::task::spawn_blocking(move || database.commit_application_shutdown(shutdown))
                .await
                .map_err(|error| format!("application shutdown worker failed: {error}"))?
                .map_err(|error| error.to_string())
        },
        Message::ApplicationShutdownPersisted,
    )
}
