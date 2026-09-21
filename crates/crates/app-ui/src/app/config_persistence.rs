use iced::Task;

use super::FileBrowser;
use crate::commands::{
    custom_color_scheme_import_command, save_app_config_command, save_user_preferences_command,
};
use crate::config;
use crate::matugen_theme::{ColorSchemeFamily, ColorSchemePreset, CustomColorScheme};
use crate::model::Message;

impl FileBrowser {
    pub(super) fn select_color_scheme_family(
        &mut self,
        family: ColorSchemeFamily,
    ) -> Task<Message> {
        let mode = self
            .application_theme
            .effective_mode(self.user_config.theme_mode);
        let has_styles = family.styles(mode).len() > 1;
        if self.user_config.color_scheme.family() == family {
            self.expanded_color_scheme_family =
                if has_styles && self.expanded_color_scheme_family != Some(family) {
                    Some(family)
                } else {
                    None
                };
            return Task::none();
        }

        self.expanded_color_scheme_family = has_styles.then_some(family);
        self.user_config.color_scheme = family.default_preset();
        self.persist_user_preferences_command()
    }

    pub(super) fn select_color_scheme_preset(
        &mut self,
        preset: ColorSchemePreset,
    ) -> Task<Message> {
        if self.user_config.color_scheme == preset {
            return Task::none();
        }
        self.user_config.color_scheme = preset;
        let mode = self
            .application_theme
            .effective_mode(self.user_config.theme_mode);
        self.expanded_color_scheme_family =
            (preset.family().styles(mode).len() > 1).then_some(preset.family());
        self.persist_user_preferences_command()
    }

    pub(super) fn select_theme_mode(
        &mut self,
        theme_mode: crate::matugen_theme::ThemeMode,
    ) -> Task<Message> {
        if self.user_config.color_scheme == ColorSchemePreset::Matugen
            || self.user_config.theme_mode == theme_mode
        {
            return Task::none();
        }
        self.user_config.theme_mode = theme_mode;
        self.expanded_color_scheme_family = None;
        self.persist_user_preferences_command()
    }
    pub(super) fn import_custom_color_scheme(&self) -> Task<Message> {
        custom_color_scheme_import_command()
    }

    pub(super) fn accept_custom_color_scheme_import(
        &mut self,
        outcome: Result<Option<String>, String>,
    ) -> Task<Message> {
        let document = match outcome {
            Ok(Some(document)) => document,
            Ok(None) => return Task::none(),
            Err(error) => {
                self.custom_color_scheme_import_error = Some(error);
                return Task::none();
            }
        };
        let custom_color_scheme = match CustomColorScheme::from_json(&document) {
            Ok(custom_color_scheme) => custom_color_scheme,
            Err(error) => {
                self.custom_color_scheme_import_error = Some(error);
                return Task::none();
            }
        };

        self.application_theme
            .replace_custom_color_scheme(custom_color_scheme.clone());
        self.user_config.custom_color_scheme = custom_color_scheme;
        self.user_config.color_scheme = ColorSchemePreset::Custom;
        self.expanded_color_scheme_family = None;
        self.custom_color_scheme_import_error = None;
        self.persist_user_preferences_command()
    }

    pub(super) fn persist_user_preferences_command(&mut self) -> Task<Message> {
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
            self.operation_queue.task_queue_store().cloned(),
        )
    }

    pub(super) fn persist_app_config_command(&self) -> Task<Message> {
        save_app_config_command(config::AppConfig::from_user_config(&self.user_config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_import_is_atomic_and_selects_custom_only_after_validation() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let previous = browser.user_config.custom_color_scheme.clone();

        drop(browser.accept_custom_color_scheme_import(Err("read failed".to_owned())));
        assert_eq!(browser.user_config.custom_color_scheme, previous);
        assert_eq!(browser.user_config.color_scheme, ColorSchemePreset::Default);
        assert!(!browser.user_preferences_save_in_flight);

        let document = r##"{
            "version": 1,
            "light": {
                "background": "#ffffff", "surface": "#f6f8fa", "text": "#1f2328",
                "muted_text": "#59636e", "primary": "#0969da", "success": "#1a7f37",
                "warning": "#9a6700", "danger": "#d1242f"
            },
            "dark": {
                "background": "#0d1117", "surface": "#151b23", "text": "#f0f6fc",
                "muted_text": "#9198a1", "primary": "#4493f8", "success": "#3fb950",
                "warning": "#d29922", "danger": "#f85149"
            }
        }"##;
        drop(browser.accept_custom_color_scheme_import(Ok(Some(document.to_owned()))));
        assert_eq!(browser.user_config.color_scheme, ColorSchemePreset::Custom);
        assert_eq!(
            browser.user_config.custom_color_scheme.light.background,
            iced::Color::from_rgb8(255, 255, 255)
        );
        assert!(browser.user_preferences_save_in_flight);
    }

    #[test]
    fn canceled_custom_import_does_not_change_state() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.custom_color_scheme_import_error = Some("old error".to_owned());
        let before = browser.user_config.custom_color_scheme.clone();
        drop(browser.accept_custom_color_scheme_import(Ok(None)));
        assert_eq!(browser.user_config.custom_color_scheme, before);
        assert_eq!(
            browser.custom_color_scheme_import_error.as_deref(),
            Some("old error")
        );
        assert!(!browser.user_preferences_save_in_flight);
    }

    #[test]
    fn color_scheme_family_selection_uses_one_persisted_preset_and_toggles_styles() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.user_config.theme_mode = crate::matugen_theme::ThemeMode::Dark;

        drop(browser.select_color_scheme_family(ColorSchemeFamily::Catppuccin));
        assert_eq!(
            browser.user_config.color_scheme,
            ColorSchemePreset::Catppuccin
        );
        assert_eq!(
            browser.expanded_color_scheme_family,
            Some(ColorSchemeFamily::Catppuccin)
        );

        drop(browser.select_color_scheme_preset(ColorSchemePreset::CatppuccinFrappe));
        assert_eq!(
            browser.user_config.color_scheme,
            ColorSchemePreset::CatppuccinFrappe
        );
        assert_eq!(
            browser.expanded_color_scheme_family,
            Some(ColorSchemeFamily::Catppuccin)
        );

        drop(browser.select_color_scheme_family(ColorSchemeFamily::Catppuccin));
        assert_eq!(browser.expanded_color_scheme_family, None);
        drop(browser.select_color_scheme_family(ColorSchemeFamily::Catppuccin));
        assert_eq!(
            browser.expanded_color_scheme_family,
            Some(ColorSchemeFamily::Catppuccin)
        );
    }

    #[test]
    fn theme_mode_selection_clears_styles_and_matugen_keeps_its_mode() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.user_config.theme_mode = crate::matugen_theme::ThemeMode::Dark;
        browser.expanded_color_scheme_family = Some(ColorSchemeFamily::Everforest);

        drop(browser.select_theme_mode(crate::matugen_theme::ThemeMode::Light));
        assert_eq!(
            browser.user_config.theme_mode,
            crate::matugen_theme::ThemeMode::Light
        );
        assert_eq!(browser.expanded_color_scheme_family, None);

        browser.user_config.color_scheme = ColorSchemePreset::Matugen;
        drop(browser.select_theme_mode(crate::matugen_theme::ThemeMode::Dark));
        assert_eq!(
            browser.user_config.theme_mode,
            crate::matugen_theme::ThemeMode::Light
        );
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
}
