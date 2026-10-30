//! 启动时读设置、配置遇到读不懂或读不了：要告诉用户的事，以及本次还能不能写回。
//! 参照 GNOME dconf（坏数据挪到一边、默认值继续）与 KDE KConfig（逐项回落）；本程序
//! 有界面，所以多提示一次。

use std::path::PathBuf;

use bennu_localization::UiLanguage;

/// 出问题的是哪一份数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSource {
    /// 设置（数据库里的一行）。
    Preferences,
    /// 配置文件 config.toml。
    AppConfig,
}

/// 被恢复成默认的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResetSettings {
    All,
    Keys(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsRecovery {
    /// 读不懂的部分已恢复默认，原数据已备份到 `backup`。
    Restored {
        source: SettingsSource,
        reset: ResetSettings,
        backup: PathBuf,
    },
    /// 读不懂，原数据没能备份：本次不写回，免得覆盖它。
    NotBackedUp {
        source: SettingsSource,
        error: String,
    },
    /// 读不了（打不开、读出错）：本次用默认值、不写回，免得用默认值覆盖真实数据。
    Unavailable {
        source: SettingsSource,
        error: String,
    },
    /// 首次从旧配置文件迁移的设置没能写入（本次照用）。
    MigrationNotSaved { error: String },
}

impl SettingsRecovery {
    /// 本次不能再写回的那份数据。
    pub(crate) fn blocked_source(&self) -> Option<SettingsSource> {
        match self {
            Self::NotBackedUp { source, .. } | Self::Unavailable { source, .. } => Some(*source),
            Self::Restored { .. } | Self::MigrationNotSaved { .. } => None,
        }
    }

    pub(crate) fn message(&self, language: UiLanguage) -> String {
        use SettingsSource::{AppConfig, Preferences};
        match self {
            Self::Restored {
                source,
                reset,
                backup,
            } => {
                let backup = backup.display();
                match (source, reset) {
                    (Preferences, ResetSettings::All) => tr_in!(
                        language,
                        "Settings could not be read and were reset to defaults. The original settings were backed up to %1.",
                        backup
                    ),
                    (Preferences, ResetSettings::Keys(keys)) => tr_in!(
                        language,
                        "Some settings could not be read and were reset to defaults: %1. The original settings were backed up to %2.",
                        keys.join(&tr_in!(language, ", ")),
                        backup
                    ),
                    (AppConfig, ResetSettings::All) => tr_in!(
                        language,
                        "The configuration file could not be read and was reset to defaults. The original file was backed up to %1.",
                        backup
                    ),
                    (AppConfig, ResetSettings::Keys(keys)) => tr_in!(
                        language,
                        "Some configuration entries could not be read and were reset to defaults: %1. The original file was backed up to %2.",
                        keys.join(&tr_in!(language, ", ")),
                        backup
                    ),
                }
            }
            Self::NotBackedUp {
                source: Preferences,
                error,
            } => tr_in!(
                language,
                "Settings could not be read, and the original settings could not be backed up (%1). Changes to settings will not be saved this time.",
                error
            ),
            Self::NotBackedUp {
                source: AppConfig,
                error,
            } => tr_in!(
                language,
                "The configuration file could not be read, and the original file could not be backed up (%1). Changes will not be written to it this time.",
                error
            ),
            Self::Unavailable {
                source: Preferences,
                error,
            } => tr_in!(
                language,
                "Settings could not be loaded (%1). Defaults are in use and changes will not be saved this time.",
                error
            ),
            Self::Unavailable {
                source: AppConfig,
                error,
            } => tr_in!(
                language,
                "The configuration file could not be loaded (%1). Defaults are in use and changes will not be written to it this time.",
                error
            ),
            Self::MigrationNotSaved { error } => tr_in!(
                language,
                "Settings from the old configuration file could not be saved (%1). They are in use for this session.",
                error
            ),
        }
    }
}

/// 本次能不能把这份数据写回。不能时保留原因，保存时再告诉用户一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsPersistence {
    Writable,
    Blocked(SettingsRecovery),
}

impl SettingsPersistence {
    pub(crate) fn for_source(recoveries: &[SettingsRecovery], source: SettingsSource) -> Self {
        recoveries
            .iter()
            .find(|recovery| recovery.blocked_source() == Some(source))
            .map_or(Self::Writable, |recovery| Self::Blocked(recovery.clone()))
    }
}
