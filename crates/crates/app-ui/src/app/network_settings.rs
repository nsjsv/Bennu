use iced::Task;

use super::FileBrowser;
use crate::app::remote_mounts::path_is_remote_mount;
use crate::config;
use crate::model::Message;
use crate::thumbnail_cache::{ThumbnailLoadPolicy, ThumbnailPurpose};

impl FileBrowser {
    pub(super) fn toggle_network_list_thumbnail_downloads(&mut self) -> Task<Message> {
        self.user_config.network_list_thumbnail_downloads_enabled =
            !self.user_config.network_list_thumbnail_downloads_enabled;
        if !self.user_config.network_list_thumbnail_downloads_enabled {
            self.remove_pending_network_thumbnail_generations();
        }

        Task::batch([
            self.persist_user_preferences_command(),
            self.schedule_thumbnail_refresh(),
        ])
    }

    pub(super) fn update_preview_size_limit_mib_input(
        &mut self,
        kind_index: usize,
        value: String,
    ) -> Task<Message> {
        if let Some(input) = self.preview_size_limit_mib_inputs.get_mut(kind_index) {
            *input = value;
        }
        if let Some(error) = self.preview_size_limit_mib_errors.get_mut(kind_index) {
            *error = None;
        }
        Task::none()
    }

    pub(super) fn commit_preview_size_limit_mib_input(
        &mut self,
        kind_index: usize,
    ) -> Task<Message> {
        let Some(kind) = config::PreviewFileSizeKind::ALL.get(kind_index).copied() else {
            return Task::none();
        };
        let mib = match self.preview_size_limit_mib_inputs[kind_index]
            .trim()
            .parse::<u64>()
        {
            Ok(mib) => mib,
            Err(_) => {
                self.preview_size_limit_mib_errors[kind_index] =
                    Some("Enter a whole number of MiB (0 = unlimited).".to_owned());
                return Task::none();
            }
        };
        let Some(bytes) = config::preview_size_limit_bytes_from_mib(mib) else {
            self.preview_size_limit_mib_errors[kind_index] =
                Some("Enter a smaller preview size.".to_owned());
            return Task::none();
        };

        self.preview_size_limit_mib_inputs[kind_index] =
            config::preview_size_limit_mib(bytes).to_string();
        self.preview_size_limit_mib_errors[kind_index] = None;
        if self.user_config.preview_size_limits.limit(kind) == bytes {
            return Task::none();
        }

        self.user_config.preview_size_limits.set_limit(kind, bytes);
        self.persist_user_preferences_command()
    }

    pub(super) fn update_preview_directory_expand_levels_input(
        &mut self,
        value: String,
    ) -> Task<Message> {
        self.preview_directory_expand_levels_input = value;
        self.preview_directory_expand_levels_error = None;
        Task::none()
    }

    pub(super) fn commit_preview_directory_expand_levels_input(&mut self) -> Task<Message> {
        let Ok(levels) = self
            .preview_directory_expand_levels_input
            .trim()
            .parse::<u8>()
        else {
            self.preview_directory_expand_levels_error =
                Some("Enter a whole number from 0 to 3.".to_owned());
            return Task::none();
        };
        if levels > config::MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS {
            self.preview_directory_expand_levels_error =
                Some("Enter a whole number from 0 to 3.".to_owned());
            return Task::none();
        }

        self.preview_directory_expand_levels_input = levels.to_string();
        self.preview_directory_expand_levels_error = None;
        if self.user_config.preview_directory_expand_levels == levels {
            return Task::none();
        }

        self.user_config.preview_directory_expand_levels = levels;
        self.persist_user_preferences_command()
    }

    fn remove_pending_network_thumbnail_generations(&mut self) {
        let network_connections = self.network_connections.clone();
        let sidebar_devices = self.sidebar_devices.clone();
        self.thumbnail_cache.retain_queued_work(|work| {
            work.purpose == ThumbnailPurpose::Preview
                || work.load_policy == ThumbnailLoadPolicy::CacheOnly
                || !path_is_remote_mount(
                    &network_connections,
                    &sidebar_devices,
                    &work.request.source,
                )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committing_invalid_preview_size_keeps_config_and_reports_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let original_limits = browser.user_config.preview_size_limits;

        let _ = browser.update_preview_size_limit_mib_input(0, "abc".to_owned());
        let _ = browser.commit_preview_size_limit_mib_input(0);

        assert_eq!(browser.user_config.preview_size_limits, original_limits);
        assert_eq!(
            browser.preview_size_limit_mib_errors[0].as_deref(),
            Some("Enter a whole number of MiB (0 = unlimited).")
        );
    }

    #[test]
    fn committing_zero_persists_unlimited_preview_size() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        let _ = browser.update_preview_size_limit_mib_input(0, "0".to_owned());
        let _ = browser.commit_preview_size_limit_mib_input(0);

        assert_eq!(browser.user_config.preview_size_limits.text_bytes, 0);
        assert!(browser.preview_size_limit_mib_errors[0].is_none());
    }

    #[test]
    fn committing_rejects_directory_expand_levels_above_maximum() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        let _ = browser.update_preview_directory_expand_levels_input("4".to_owned());
        let _ = browser.commit_preview_directory_expand_levels_input();

        assert_eq!(browser.user_config.preview_directory_expand_levels, 1);
        assert_eq!(
            browser.preview_directory_expand_levels_error.as_deref(),
            Some("Enter a whole number from 0 to 3.")
        );
    }
}
