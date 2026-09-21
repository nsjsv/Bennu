use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::commands::startup_directory_validation_command;
use crate::config::{LaunchWindowPolicy, StartupLocationPolicy, UiLanguageSetting};
use crate::model::{Message, StartupDirectoryAvailability, StartupDirectoryValidationRequest};

impl FileBrowser {
    pub(super) fn select_language_setting(
        &mut self,
        language_setting: UiLanguageSetting,
    ) -> Task<Message> {
        if self.user_config.language_setting == language_setting {
            return Task::none();
        }
        self.user_config.language_setting = language_setting;
        self.refresh_current_language();
        self.persist_user_preferences_command()
    }

    pub(super) fn select_startup_location_policy(
        &mut self,
        policy: StartupLocationPolicy,
    ) -> Task<Message> {
        let save_view_state = policy.saves_view_state();
        if self.user_config.startup_location_policy == policy
            && self.user_config.save_view_state == save_view_state
        {
            return Task::none();
        }
        self.invalidate_startup_directory_validation();
        self.user_config.startup_location_policy = policy;
        self.user_config.save_view_state = save_view_state;
        if !save_view_state {
            self.pending_browser_session_save = false;
        }
        Task::batch([
            self.persist_user_preferences_command(),
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn select_launch_window_policy(
        &mut self,
        policy: LaunchWindowPolicy,
    ) -> Task<Message> {
        if self.user_config.launch_window_policy == policy {
            return Task::none();
        }
        self.user_config.launch_window_policy = policy;
        self.persist_user_preferences_command()
    }

    pub(super) fn update_startup_custom_directory_input(&mut self, value: String) -> Task<Message> {
        self.invalidate_startup_directory_validation();
        self.startup_custom_directory_input = value;
        self.startup_custom_directory_error = None;
        Task::none()
    }

    pub(super) fn commit_startup_custom_directory_input(&mut self) -> Task<Message> {
        self.invalidate_startup_directory_validation();
        let trimmed = self.startup_custom_directory_input.trim();
        if trimmed.is_empty() {
            self.startup_custom_directory_error = Some("Enter a directory path.".to_owned());
            return Task::none();
        }
        let request = StartupDirectoryValidationRequest {
            generation: self.startup_directory_validation_generation,
            input: self.startup_custom_directory_input.clone(),
            directory: PathBuf::from(trimmed),
        };
        self.pending_startup_directory_validation = Some(request.clone());
        self.startup_custom_directory_error = None;
        startup_directory_validation_command(request)
    }

    pub(super) fn accept_startup_directory_validation(
        &mut self,
        request: StartupDirectoryValidationRequest,
        availability: StartupDirectoryAvailability,
    ) -> Task<Message> {
        let request_is_current = self
            .pending_startup_directory_validation
            .as_ref()
            .is_some_and(|pending| pending == &request)
            && self.startup_custom_directory_input == request.input;
        if !request_is_current {
            return Task::none();
        }
        self.pending_startup_directory_validation = None;
        match availability {
            StartupDirectoryAvailability::Usable => {
                self.user_config.startup_custom_directory = request.directory;
                self.startup_custom_directory_input = self
                    .user_config
                    .startup_custom_directory
                    .to_string_lossy()
                    .into_owned();
                self.startup_custom_directory_error = None;
                self.persist_user_preferences_command()
            }
            StartupDirectoryAvailability::Unavailable => {
                self.startup_custom_directory_error =
                    Some("Choose an existing directory.".to_owned());
                Task::none()
            }
        }
    }

    pub(super) fn invalidate_startup_directory_validation(&mut self) {
        self.startup_directory_validation_generation =
            self.startup_directory_validation_generation.wrapping_add(1);
        self.pending_startup_directory_validation = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn previous_state_startup_policy_enables_view_state_saving() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.select_startup_location_policy(StartupLocationPolicy::PreviousSession));

        assert_eq!(
            browser.user_config().startup_location_policy,
            StartupLocationPolicy::PreviousSession
        );
        assert!(browser.user_config().save_view_state);
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn non_previous_startup_policy_disables_view_state_saving() {
        let mut user_config = config::ui_thread_startup_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(user_config);

        drop(browser.select_startup_location_policy(StartupLocationPolicy::Home));

        assert_eq!(
            browser.user_config().startup_location_policy,
            StartupLocationPolicy::Home
        );
        assert!(!browser.user_config().save_view_state);
        assert!(!browser.pending_browser_session_save);
    }

    #[test]
    fn system_language_selection_tracks_detected_language() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        browser.system_language = config::UiLanguage::Chinese;

        drop(browser.select_language_setting(UiLanguageSetting::System));

        assert_eq!(browser.active_language(), config::UiLanguage::Chinese);
    }

    #[test]
    fn explicit_language_selection_overrides_system_language() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        browser.system_language = config::UiLanguage::Chinese;

        drop(browser.select_language_setting(UiLanguageSetting::English));

        assert_eq!(browser.active_language(), config::UiLanguage::English);
    }

    #[test]
    fn current_startup_directory_validation_saves_only_usable_directory() {
        let mut user_config = config::ui_thread_startup_config();
        let original_directory = PathBuf::from("/original");
        user_config.startup_custom_directory = original_directory.clone();
        let (mut browser, _) = FileBrowser::new(user_config);
        let requested_directory = PathBuf::from("/requested");

        drop(browser.update_startup_custom_directory_input(
            requested_directory.to_string_lossy().into_owned(),
        ));
        drop(browser.commit_startup_custom_directory_input());
        let request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("pending validation request");
        assert_eq!(
            browser.user_config.startup_custom_directory,
            original_directory
        );
        drop(
            browser
                .accept_startup_directory_validation(request, StartupDirectoryAvailability::Usable),
        );

        assert_eq!(
            browser.user_config.startup_custom_directory,
            requested_directory
        );
        assert_eq!(browser.startup_custom_directory_error, None);
        assert!(browser.pending_startup_directory_validation.is_none());

        drop(browser.update_startup_custom_directory_input("/unavailable".to_owned()));
        drop(browser.commit_startup_custom_directory_input());
        let request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("pending validation request");
        drop(browser.accept_startup_directory_validation(
            request,
            StartupDirectoryAvailability::Unavailable,
        ));

        assert_eq!(
            browser.user_config.startup_custom_directory,
            requested_directory
        );
        assert_eq!(
            browser.startup_custom_directory_error.as_deref(),
            Some("Choose an existing directory.")
        );
    }

    #[test]
    fn input_change_rejects_late_startup_directory_success_and_failure() {
        let mut user_config = config::ui_thread_startup_config();
        let original_directory = PathBuf::from("/original");
        user_config.startup_custom_directory = original_directory.clone();
        let (mut browser, _) = FileBrowser::new(user_config);

        drop(browser.update_startup_custom_directory_input("/first".to_owned()));
        drop(browser.commit_startup_custom_directory_input());
        let first_request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("first validation request");
        drop(browser.update_startup_custom_directory_input("/second".to_owned()));
        drop(browser.accept_startup_directory_validation(
            first_request,
            StartupDirectoryAvailability::Usable,
        ));

        assert_eq!(
            browser.user_config.startup_custom_directory,
            original_directory
        );
        assert_eq!(browser.startup_custom_directory_error, None);

        drop(browser.commit_startup_custom_directory_input());
        let second_request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("second validation request");
        drop(browser.update_startup_custom_directory_input("/third".to_owned()));
        drop(browser.accept_startup_directory_validation(
            second_request,
            StartupDirectoryAvailability::Unavailable,
        ));

        assert_eq!(browser.startup_custom_directory_error, None);
        assert_eq!(browser.startup_custom_directory_input, "/third");
    }

    #[test]
    fn newer_startup_directory_request_replaces_same_input_generation() {
        let mut user_config = config::ui_thread_startup_config();
        let original_directory = PathBuf::from("/original");
        user_config.startup_custom_directory = original_directory.clone();
        let (mut browser, _) = FileBrowser::new(user_config);

        drop(browser.update_startup_custom_directory_input("/requested".to_owned()));
        drop(browser.commit_startup_custom_directory_input());
        let first_request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("first validation request");
        drop(browser.commit_startup_custom_directory_input());
        let second_request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("replacement validation request");
        assert_ne!(first_request.generation, second_request.generation);

        drop(browser.accept_startup_directory_validation(
            first_request,
            StartupDirectoryAvailability::Usable,
        ));
        assert_eq!(
            browser.user_config.startup_custom_directory,
            original_directory
        );

        drop(browser.accept_startup_directory_validation(
            second_request,
            StartupDirectoryAvailability::Usable,
        ));
        assert_eq!(
            browser.user_config.startup_custom_directory,
            PathBuf::from("/requested")
        );
    }

    #[test]
    fn closing_settings_rejects_late_startup_directory_validation() {
        let mut user_config = config::ui_thread_startup_config();
        let original_directory = PathBuf::from("/original");
        user_config.startup_custom_directory = original_directory.clone();
        let (mut browser, _) = FileBrowser::new(user_config);
        browser.settings_window = Some(iced::window::Id::unique());

        drop(browser.update_startup_custom_directory_input("/requested".to_owned()));
        drop(browser.commit_startup_custom_directory_input());
        let request = browser
            .pending_startup_directory_validation
            .clone()
            .expect("pending validation request");
        drop(browser.close_settings_window());
        drop(
            browser
                .accept_startup_directory_validation(request, StartupDirectoryAvailability::Usable),
        );

        assert_eq!(
            browser.user_config.startup_custom_directory,
            original_directory
        );
        assert!(browser.pending_startup_directory_validation.is_none());
    }
}
