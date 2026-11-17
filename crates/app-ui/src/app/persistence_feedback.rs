use iced::Task;

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    pub(super) fn accept_user_preferences_saved(
        &mut self,
        result: Result<crate::config::UserPreferences, String>,
    ) -> Task<Message> {
        // 先记下写成的那份，排着的下一次保存才按它算改了哪些键。
        let result = result.map(|saved| self.record_saved_user_preferences(saved));
        let feedback = self.accept_persistence_result(result, "Failed to save user preferences");
        Task::batch([feedback, self.continue_user_preferences_save()])
    }

    pub(super) fn accept_app_config_saved(
        &mut self,
        result: Result<crate::config::AppConfig, String>,
    ) -> Task<Message> {
        let result = result.map(|saved| self.record_saved_app_config(saved));
        self.accept_persistence_result(result, "Failed to save application configuration")
    }

    pub(super) fn accept_column_width_saved(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        self.accept_persistence_result(result, "Failed to save column width")
    }

    pub(super) fn accept_browser_session_saved(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        let result = self.record_browser_session_save_outcome(result);
        self.accept_persistence_result(result, "Failed to save browser session")
    }

    fn accept_persistence_result(
        &mut self,
        result: Result<(), String>,
        action: &'static str,
    ) -> Task<Message> {
        if let Err(error) = result {
            self.show_global_error(format!("{action}: {error}"));
        }
        Task::none()
    }
}
