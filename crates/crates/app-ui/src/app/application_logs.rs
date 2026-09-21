use iced::Task;

use super::FileBrowser;
use crate::commands::application_logs_command;
use crate::model::{ApplicationLogEntry, ApplicationLogLevel, ApplicationLogRequest, Message};

impl FileBrowser {
    pub(super) fn refresh_application_logs(&mut self) -> Task<Message> {
        let Some(request) = self.application_logs.request_refresh() else {
            return Task::none();
        };
        application_logs_command(request)
    }

    pub(super) fn select_application_log_threshold(
        &mut self,
        threshold: ApplicationLogLevel,
    ) -> Task<Message> {
        let Some(request) = self.application_logs.select_threshold(threshold) else {
            return Task::none();
        };
        application_logs_command(request)
    }

    pub(super) fn accept_application_logs(
        &mut self,
        request: ApplicationLogRequest,
        outcome: Result<Vec<ApplicationLogEntry>, String>,
    ) -> Task<Message> {
        self.application_logs
            .accept_loaded(request, outcome)
            .map(application_logs_command)
            .unwrap_or_else(Task::none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn log_loading_failure_stays_local_to_the_logs_page() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let request = browser.application_logs.request_refresh().unwrap();

        drop(browser.accept_application_logs(request, Err("journalctl unavailable".to_owned())));

        assert_eq!(browser.current_error(), None);
        assert_eq!(
            browser.application_logs.load_error.as_deref(),
            Some("journalctl unavailable")
        );
    }

    #[test]
    fn selecting_log_threshold_starts_a_session_only_request() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let initial_language = browser.user_config().language_setting;

        drop(browser.select_application_log_threshold(ApplicationLogLevel::Debug));

        assert_eq!(
            browser.application_logs.threshold,
            ApplicationLogLevel::Debug
        );
        assert!(browser.application_logs.is_loading());
        assert_eq!(browser.user_config().language_setting, initial_language);
    }
}
