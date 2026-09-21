use iced::Task;

use super::FileBrowser;
use crate::config;
use crate::model::Message;

impl FileBrowser {
    pub(super) fn update_preview_extension_input(
        &mut self,
        kind_index: usize,
        value: String,
    ) -> Task<Message> {
        if let Some(input) = self.preview_extension_inputs.get_mut(kind_index) {
            *input = value;
        }
        if let Some(error) = self.preview_extension_input_errors.get_mut(kind_index) {
            *error = None;
        }
        Task::none()
    }

    pub(super) fn add_preview_extension(&mut self, kind_index: usize) -> Task<Message> {
        let Some(kind) = config::PreviewFileSizeKind::ALL.get(kind_index).copied() else {
            return Task::none();
        };
        let Some(extension) =
            config::normalize_preview_extension(&self.preview_extension_inputs[kind_index])
        else {
            self.preview_extension_input_errors[kind_index] =
                Some("Enter an extension like txt (a leading dot is optional).".to_owned());
            return Task::none();
        };
        // 重复后缀不算错误：清空输入框表示本次添加已完成。
        let already_listed = self
            .user_config
            .preview_extension_rules
            .list(kind)
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&extension));
        self.preview_extension_inputs[kind_index].clear();
        self.preview_extension_input_errors[kind_index] = None;
        if already_listed {
            return Task::none();
        }

        self.user_config
            .preview_extension_rules
            .list_mut(kind)
            .push(extension);
        self.persist_user_preferences_command()
    }

    /// 重置需要行内确认：先记录待确认的类型，确认后才真正恢复默认。
    pub(super) fn request_preview_extension_reset(&mut self, kind_index: usize) -> Task<Message> {
        self.preview_extension_reset_confirmation = Some(kind_index);
        Task::none()
    }

    pub(super) fn confirm_preview_extension_reset(&mut self, kind_index: usize) -> Task<Message> {
        self.preview_extension_reset_confirmation = None;
        self.reset_preview_extension_list(kind_index)
    }

    pub(super) fn remove_preview_extension(
        &mut self,
        kind_index: usize,
        extension: &str,
    ) -> Task<Message> {
        let Some(kind) = config::PreviewFileSizeKind::ALL.get(kind_index).copied() else {
            return Task::none();
        };
        self.user_config
            .preview_extension_rules
            .list_mut(kind)
            .retain(|candidate| !candidate.eq_ignore_ascii_case(extension));
        self.persist_user_preferences_command()
    }

    pub(super) fn reset_preview_extension_list(&mut self, kind_index: usize) -> Task<Message> {
        let Some(kind) = config::PreviewFileSizeKind::ALL.get(kind_index).copied() else {
            return Task::none();
        };
        let default_list = config::PreviewExtensionRules::default_list(kind);
        if *self.user_config.preview_extension_rules.list(kind) == default_list {
            return Task::none();
        }

        self.user_config
            .preview_extension_rules
            .set_list(kind, default_list);
        self.persist_user_preferences_command()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn kind_index(kind: config::PreviewFileSizeKind) -> usize {
        config::PreviewFileSizeKind::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .expect("kind is part of ALL")
    }

    #[test]
    fn adding_extension_normalizes_into_requested_type() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let image_index = kind_index(config::PreviewFileSizeKind::Image);

        let _ = browser.update_preview_extension_input(image_index, ".PNG2".to_owned());
        let _ = browser.add_preview_extension(image_index);

        assert!(browser
            .user_config
            .preview_extension_rules
            .list(config::PreviewFileSizeKind::Image)
            .iter()
            .any(|candidate| candidate == "png2"));
        assert!(browser.preview_extension_inputs[image_index].is_empty());
    }

    #[test]
    fn adding_invalid_extension_reports_error_without_config_change() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let video_index = kind_index(config::PreviewFileSizeKind::Video);
        let original = browser.user_config.preview_extension_rules.clone();

        let _ = browser.update_preview_extension_input(video_index, "my ext".to_owned());
        let _ = browser.add_preview_extension(video_index);

        assert_eq!(browser.user_config.preview_extension_rules, original);
        assert!(browser.preview_extension_input_errors[video_index].is_some());
    }

    #[test]
    fn reset_restores_default_list_for_requested_type() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let text_index = kind_index(config::PreviewFileSizeKind::Text);
        let kind = config::PreviewFileSizeKind::Text;
        browser
            .user_config
            .preview_extension_rules
            .list_mut(kind)
            .push("custom".to_owned());

        let _ = browser.reset_preview_extension_list(text_index);

        assert_eq!(
            browser.user_config.preview_extension_rules.list(kind),
            &config::PreviewExtensionRules::default_list(kind)
        );
    }

    #[test]
    fn removing_extension_updates_requested_type_only() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let text_index = kind_index(config::PreviewFileSizeKind::Text);

        let _ = browser.remove_preview_extension(text_index, "TXT");

        assert!(!browser
            .user_config
            .preview_extension_rules
            .list(config::PreviewFileSizeKind::Text)
            .iter()
            .any(|candidate| candidate == "txt"));
        // 其他类型的列表不受影响。
        assert!(browser
            .user_config
            .preview_extension_rules
            .list(config::PreviewFileSizeKind::Image)
            .iter()
            .any(|candidate| candidate == "png"));
    }

    #[test]
    fn reset_runs_only_after_second_press() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let text_index = kind_index(config::PreviewFileSizeKind::Text);
        let kind = config::PreviewFileSizeKind::Text;
        browser
            .user_config
            .preview_extension_rules
            .list_mut(kind)
            .push("custom".to_owned());

        let _ = browser.request_preview_extension_reset(text_index);
        assert_eq!(
            browser.preview_extension_reset_confirmation,
            Some(text_index)
        );
        // 未确认前配置不变。
        assert!(browser
            .user_config
            .preview_extension_rules
            .list(kind)
            .iter()
            .any(|candidate| candidate == "custom"));

        assert!(browser
            .user_config
            .preview_extension_rules
            .list(kind)
            .iter()
            .any(|candidate| candidate == "custom"));

        let _ = browser.request_preview_extension_reset(text_index);
        let _ = browser.confirm_preview_extension_reset(text_index);
        assert_eq!(browser.preview_extension_reset_confirmation, None);
        assert_eq!(
            browser.user_config.preview_extension_rules.list(kind),
            &config::PreviewExtensionRules::default_list(kind)
        );
    }
}
