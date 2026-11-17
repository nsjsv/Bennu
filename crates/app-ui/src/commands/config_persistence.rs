use bennu_state::{PreferencesChange, UserSettingsStore};
use iced::Task;

use crate::config;
use crate::model::Message;

pub(crate) fn save_app_config_command(
    change: PreferencesChange<config::AppConfig>,
) -> Task<Message> {
    Task::perform(persist_app_config(change), Message::AppConfigSaved)
}

/// 存成功时回写下的那份：它就是这个进程此后"上次写下的设置"。
pub(crate) fn save_user_preferences_command(
    change: PreferencesChange<config::UserPreferences>,
    settings_store: Option<UserSettingsStore>,
) -> Task<Message> {
    Task::perform(
        persist_user_preferences(change, settings_store),
        Message::UserPreferencesSaved,
    )
}

async fn persist_app_config(
    change: PreferencesChange<config::AppConfig>,
) -> Result<config::AppConfig, String> {
    tokio::task::spawn_blocking(move || config::save_app_config(&change).map(|()| change.current))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

async fn persist_user_preferences(
    change: PreferencesChange<config::UserPreferences>,
    settings_store: Option<UserSettingsStore>,
) -> Result<config::UserPreferences, String> {
    tokio::task::spawn_blocking(move || {
        let store = match settings_store {
            Some(store) => store,
            None => UserSettingsStore::new(config::default_state_database_path())?,
        };
        config::save_user_preferences(&store, &change).map(|()| change.current)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}
