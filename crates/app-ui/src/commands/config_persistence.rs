use file_operation_store::TaskQueueStore;
use iced::Task;

use crate::config;
use crate::model::Message;

pub(crate) fn save_app_config_command(app_config: config::AppConfig) -> Task<Message> {
    Task::perform(persist_app_config(app_config), Message::AppConfigSaved)
}

pub(crate) fn save_user_preferences_command(
    preferences: config::UserPreferences,
    task_queue_store: Option<TaskQueueStore>,
) -> Task<Message> {
    Task::perform(
        persist_user_preferences(preferences, task_queue_store),
        Message::UserPreferencesSaved,
    )
}

async fn persist_app_config(app_config: config::AppConfig) -> Result<(), String> {
    tokio::task::spawn_blocking(move || config::save_app_config(&app_config))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

async fn persist_user_preferences(
    preferences: config::UserPreferences,
    task_queue_store: Option<TaskQueueStore>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let store = match task_queue_store {
            Some(store) => store,
            None => TaskQueueStore::new(config::default_state_database_path())?,
        };
        config::save_user_preferences(&store, &preferences)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}
