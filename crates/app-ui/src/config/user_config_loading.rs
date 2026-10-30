use std::path::Path;

use file_operation_store::{StoreResult, TaskQueueStore, UnreadablePreferences};

use super::app_config::AppConfig;
use super::legacy_toml;
use super::settings_recovery::{ResetSettings, SettingsRecovery, SettingsSource};
use super::unrecognized_settings::UnrecognizedSettings;
use super::{default_state_database_path, default_user_config, UserConfig, UserPreferences};

/// 启动时读到的设置，以及读的过程中要告诉用户的事。
#[derive(Debug)]
pub(crate) struct LoadedUserConfig {
    pub(crate) config: UserConfig,
    pub(crate) recovery: Option<SettingsRecovery>,
}

pub(crate) fn load_user_config_for_app_config(app_config: AppConfig) -> LoadedUserConfig {
    let state_database_path = default_state_database_path();
    let config_dir = file_core::app_dirs::app_config_dir();
    load_user_config_from_sources(app_config, &state_database_path, Some(&config_dir))
}

pub(super) fn load_user_config_from_sources(
    app_config: AppConfig,
    state_database_path: &Path,
    config_dir: Option<&Path>,
) -> LoadedUserConfig {
    let mut default = default_user_config();
    app_config.apply_to_user_config(&mut default);
    let unavailable = |default: UserConfig, error: String| LoadedUserConfig {
        config: default,
        recovery: Some(SettingsRecovery::Unavailable {
            source: SettingsSource::Preferences,
            error,
        }),
    };

    let store = match TaskQueueStore::new(state_database_path) {
        Ok(store) => store,
        Err(error) => return unavailable(default, error.to_string()),
    };
    match store.read_user_preferences(&default.user_preferences().to_stored()) {
        Ok(Some(read)) => {
            let interpreted = UserPreferences::from_stored(read.preferences, &default);
            let config = UserConfig::from_parts(app_config, interpreted.preferences);
            let recovery = reset_settings(read.unreadable, interpreted.unrecognized)
                .map(|reset| repair_user_preferences(&store, &read.raw_payload, &config, reset));
            LoadedUserConfig { config, recovery }
        }
        Ok(None) => {
            let legacy = config_dir.map_or_else(
                || Ok(default.clone()),
                |config_dir| {
                    legacy_toml::load_legacy_user_config_from_dir(config_dir, default.clone())
                },
            );
            let migrated = match legacy {
                Ok(migrated) => migrated,
                // 旧配置读不了：不写设置，下次启动再迁移。
                Err(error) => return unavailable(default, error.to_string()),
            };
            let recovery = store
                .replace_user_preferences(&migrated.user_preferences().to_stored())
                .err()
                .map(|error| SettingsRecovery::MigrationNotSaved {
                    error: error.to_string(),
                });
            LoadedUserConfig {
                config: migrated,
                recovery,
            }
        }
        Err(error) => unavailable(default, error.to_string()),
    }
}

/// 完整加载之前的只读读取方用（二次启动决定转发还是独立运行、窗口图标）：与启动
/// 加载同一套解释规则，库里还没有设置时为默认值。不备份、不写、不提示——那些归
/// 启动加载。读出错交给调用方记录：二次启动读策略时日志还没初始化。
pub(crate) fn read_user_preferences_without_repair(
    state_database_path: &Path,
) -> StoreResult<UserPreferences> {
    let default = default_user_config();
    let read = TaskQueueStore::new(state_database_path)?
        .read_user_preferences(&default.user_preferences().to_stored())?;
    Ok(match read {
        Some(read) => UserPreferences::from_stored(read.preferences, &default).preferences,
        None => default.user_preferences(),
    })
}

/// 存储层读不懂的键与界面层不认识的值合成一份"已恢复默认"的清单。
fn reset_settings(
    unreadable: Option<UnreadablePreferences>,
    unrecognized: UnrecognizedSettings,
) -> Option<ResetSettings> {
    let mut keys = match unreadable {
        Some(UnreadablePreferences::Whole) => return Some(ResetSettings::All),
        Some(UnreadablePreferences::Keys(keys)) => keys,
        None => Vec::new(),
    };
    for key in unrecognized.into_keys() {
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_owned());
        }
    }
    keys.sort();
    (!keys.is_empty()).then_some(ResetSettings::Keys(keys))
}

/// 先备份原始数据再写回修复后的设置，之后的保存不会再碰到读不懂的数据。
/// 备份失败时原数据不动，本次也不再写回设置。备份成功而写回失败只记日志：
/// 原数据已有备份，之后照常保存会再写回。
fn repair_user_preferences(
    store: &TaskQueueStore,
    raw_payload: &str,
    config: &UserConfig,
    reset: ResetSettings,
) -> SettingsRecovery {
    match store
        .replace_unreadable_user_preferences(raw_payload, &config.user_preferences().to_stored())
    {
        Ok(backup) => {
            if let Err(error) = backup.write_back {
                tracing::warn!(%error, backup = %backup.path.display(), "repaired settings were not written back");
            }
            SettingsRecovery::Restored {
                source: SettingsSource::Preferences,
                reset,
                backup: backup.path,
            }
        }
        Err(error) => SettingsRecovery::NotBackedUp {
            source: SettingsSource::Preferences,
            error: error.to_string(),
        },
    }
}
