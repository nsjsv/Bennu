use bennu_state::UserSettingsStore;
use iced::Task;

use crate::config;
use crate::model::Message;

pub(crate) fn save_app_config_command(app_config: config::AppConfig) -> Task<Message> {
    Task::perform(persist_app_config(app_config), Message::AppConfigSaved)
}

pub(crate) fn save_user_preferences_command(
    preferences: config::UserPreferences,
    settings_store: Option<UserSettingsStore>,
) -> Task<Message> {
    Task::perform(
        persist_user_preferences(preferences, settings_store),
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
    settings_store: Option<UserSettingsStore>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let store = match settings_store {
            Some(store) => store,
            None => UserSettingsStore::new(config::default_state_database_path())?,
        };
        config::save_user_preferences(&store, &preferences)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}
