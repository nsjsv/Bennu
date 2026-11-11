use bennu_state::UserSettingsStore;
use iced::Task;

use super::FileBrowser;
use crate::commands::{save_app_config_command, save_user_preferences_command};
use crate::config;
use crate::model::Message;
use bennu_theme::ColorSchemePreset;

impl FileBrowser {
    pub(super) fn select_color_scheme_preset(
        &mut self,
        preset: ColorSchemePreset,
    ) -> Task<Message> {
        if self.user_config.color_scheme == preset {
            return Task::none();
        }
        self.user_config.color_scheme = preset;
        self.persist_user_preferences_command()
    }

    pub(super) fn select_theme_mode(
        &mut self,
        theme_mode: bennu_theme::ThemeMode,
    ) -> Task<Message> {
        if self.user_config.theme_mode == theme_mode {
            return Task::none();
        }
        self.user_config.theme_mode = theme_mode;
        self.persist_user_preferences_command()
    }

    pub(super) fn persist_user_preferences_command(&mut self) -> Task<Message> {
        // 预览域设置（展开层数/后缀规则/大小上限）的变都以持久化收尾，
        // 这里是引擎配置快照的唯一刷新点，保证引擎读到落盘值。
        self.preview_engine.config = bennu_preview::engine::PreviewEngineConfig {
            directory_expand_levels: self.user_config.preview_directory_expand_levels,
            extension_rules: self.user_config.preview_extension_rules.clone(),
            size_limits: self.user_config.preview_size_limits,
        };
        if let config::SettingsPersistence::Blocked(recovery) = &self.user_preferences_persistence {
            // 原设置读不了或没能备份：写回会覆盖它，修改本次只在界面上生效。
            let message = recovery.message(self.active_language());
            self.show_global_error(message);
            return Task::none();
        }
        let preferences = self.user_config.user_preferences();
        if self.user_preferences_save_in_flight {
            self.pending_user_preferences_save = Some(preferences);
            return Task::none();
        }

        self.user_preferences_save_in_flight = true;
        self.save_user_preferences_snapshot_command(preferences)
    }

    pub(super) fn continue_user_preferences_save(&mut self) -> Task<Message> {
        let Some(preferences) = self.pending_user_preferences_save.take() else {
            self.user_preferences_save_in_flight = false;
            return Task::none();
        };
        self.save_user_preferences_snapshot_command(preferences)
    }

    fn save_user_preferences_snapshot_command(
        &self,
        preferences: config::UserPreferences,
    ) -> Task<Message> {
        save_user_preferences_command(
            preferences,
            self.operation_queue
                .state_database()
                .map(UserSettingsStore::from),
        )
    }

    /// 启动时读设置、配置的结果：决定本次能否写回，并把要告诉用户的事合成一条提示。
    pub(super) fn accept_settings_recoveries(&mut self, recoveries: Vec<config::SettingsRecovery>) {
        self.user_preferences_persistence = config::SettingsPersistence::for_source(
            &recoveries,
            config::SettingsSource::Preferences,
        );
        self.app_config_persistence =
            config::SettingsPersistence::for_source(&recoveries, config::SettingsSource::AppConfig);
        if recoveries.is_empty() {
            return;
        }
        let language = self.active_language();
        let message = recoveries
            .iter()
            .map(|recovery| recovery.message(language))
            .collect::<Vec<_>>()
            .join("\n");
        self.show_global_error(message);
    }

    pub(super) fn persist_app_config_command(&mut self) -> Task<Message> {
        if let config::SettingsPersistence::Blocked(recovery) = &self.app_config_persistence {
            // 原配置文件读不了或没能备份：写回会覆盖它，修改本次只在界面上生效。
            let message = recovery.message(self.active_language());
            self.show_global_error(message);
            return Task::none();
        }
        save_app_config_command(config::AppConfig::from_user_config(&self.user_config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_and_mode_selection_persist_only_real_changes() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.select_color_scheme_preset(ColorSchemePreset::Default));
        drop(browser.select_theme_mode(browser.user_config.theme_mode));
        assert!(!browser.user_preferences_save_in_flight);

        drop(browser.select_color_scheme_preset(ColorSchemePreset::Catppuccin));
        assert_eq!(
            browser.user_config.color_scheme,
            ColorSchemePreset::Catppuccin
        );
        assert!(browser.user_preferences_save_in_flight);
    }

    #[test]
    fn user_preferences_save_coalesces_to_the_latest_pending_snapshot() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.persist_user_preferences_command());
        assert!(browser.user_preferences_save_in_flight);
        assert!(browser.pending_user_preferences_save.is_none());

        browser.user_config.show_hidden_files = true;
        drop(browser.persist_user_preferences_command());
        browser.user_config.sidebar_width = 240.0;
        drop(browser.persist_user_preferences_command());

        let pending = browser
            .pending_user_preferences_save
            .as_ref()
            .expect("latest preferences snapshot");
        assert!(pending.show_hidden_files);
        assert_eq!(pending.sidebar_width, 240.0);

        drop(browser.accept_user_preferences_saved(Ok(())));
        assert!(browser.user_preferences_save_in_flight);
        assert!(browser.pending_user_preferences_save.is_none());

        drop(browser.accept_user_preferences_saved(Ok(())));
        assert!(!browser.user_preferences_save_in_flight);
    }

    #[test]
    fn failed_user_preferences_save_still_advances_to_the_latest_snapshot() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        drop(browser.persist_user_preferences_command());
        browser.user_config.show_hidden_files = true;
        drop(browser.persist_user_preferences_command());

        drop(browser.accept_user_preferences_saved(Err("read-only".to_owned())));

        assert!(browser.user_preferences_save_in_flight);
        assert!(browser.pending_user_preferences_save.is_none());
        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("read-only")));
    }

    #[test]
    fn restored_settings_are_announced_once_with_the_backup_path() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        browser.accept_settings_recoveries(vec![config::SettingsRecovery::Restored {
            source: config::SettingsSource::Preferences,
            reset: config::ResetSettings::All,
            backup: std::path::PathBuf::from("/data/bennu/user-preferences-1.json"),
        }]);

        assert!(browser
            .current_error()
            .is_some_and(|notice| notice.contains("/data/bennu/user-preferences-1.json")));
        drop(browser.persist_user_preferences_command());
        assert!(browser.user_preferences_save_in_flight);
    }

    #[test]
    fn settings_that_could_not_be_backed_up_are_never_written_back() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.accept_settings_recoveries(vec![config::SettingsRecovery::NotBackedUp {
            source: config::SettingsSource::Preferences,
            error: "permission denied".to_owned(),
        }]);
        browser.dismiss_global_error();

        browser.user_config.show_hidden_files = true;
        drop(browser.persist_user_preferences_command());

        assert!(!browser.user_preferences_save_in_flight);
        assert!(browser
            .current_error()
            .is_some_and(|notice| notice.contains("permission denied")));
    }

    #[test]
    fn a_config_file_that_could_not_be_backed_up_is_never_written_back() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.accept_settings_recoveries(vec![config::SettingsRecovery::NotBackedUp {
            source: config::SettingsSource::AppConfig,
            error: "read-only file system".to_owned(),
        }]);
        browser.dismiss_global_error();

        drop(browser.persist_app_config_command());

        assert!(browser
            .current_error()
            .is_some_and(|notice| notice.contains("read-only file system")));
        assert_eq!(
            browser.user_preferences_persistence,
            config::SettingsPersistence::Writable
        );
    }

    #[test]
    fn both_restored_sources_are_announced_in_one_notice() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        browser.accept_settings_recoveries(vec![
            config::SettingsRecovery::Restored {
                source: config::SettingsSource::AppConfig,
                reset: config::ResetSettings::All,
                backup: std::path::PathBuf::from("/config/bennu/config.toml.1.bak"),
            },
            config::SettingsRecovery::Restored {
                source: config::SettingsSource::Preferences,
                reset: config::ResetSettings::All,
                backup: std::path::PathBuf::from("/data/bennu/user-preferences-1.json"),
            },
        ]);

        let notice = browser.current_error().expect("notice");
        assert!(notice.contains("/config/bennu/config.toml.1.bak"));
        assert!(notice.contains("/data/bennu/user-preferences-1.json"));
    }
}
