//! 设置在 Bennu 进程之间同步。二次启动默认另起一个进程开新窗口，每个进程手上各有一份
//! 设置：存的时候只写自己改过的键（`bennu_state::PreferencesChange`），状态库有写入时重读，
//! 把别的进程改的键拿过来。
//!
//! 拿过来的全局设置马上作用到这个进程：语言、预览、终端、侧栏书签、网络连接、LocalSend；
//! 主题、密度、快捷键这类画界面时直接读设置的不用管。窗口自己的状态（侧栏宽度、右侧预览、
//! 视图方式、排序、显示隐藏文件）只记进设置，给以后新开的窗口用，不去改这个窗口正在用的：
//! 拖宽一个窗口的侧栏，不该让另一个窗口跟着变。

use bennu_state::{merge_stored_preferences, PreferencesChange};
use iced::Task;

use super::FileBrowser;
use crate::commands::sidebar_locations_command;
use crate::config::{self, UserPreferences};
use crate::model::Message;

impl FileBrowser {
    /// 状态库有写入：重读设置，看是不是别的进程改的。
    pub(super) fn reread_user_preferences_command(&self) -> Task<Message> {
        let Some(database) = self.operation_queue.state_database() else {
            return Task::none();
        };
        let path = database.path().to_path_buf();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    config::read_user_preferences_without_repair(&path)
                })
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())
            },
            Message::StoredUserPreferencesRead,
        )
    }

    pub(super) fn accept_stored_user_preferences(
        &mut self,
        read: Result<UserPreferences, String>,
    ) -> Task<Message> {
        let stored = match read {
            Ok(stored) => normalized(stored),
            Err(error) => {
                tracing::warn!(
                    target: "app_ui::settings",
                    event = "user_preferences_reread_failed",
                    %error,
                    "could not read settings saved by another Bennu window"
                );
                return Task::none();
            }
        };
        // 自己刚写下的、或别的表有写入：库里的设置和上次看到的一样，没什么可拿。
        if stored == self.user_preferences_seen {
            return Task::none();
        }
        let change = config::stored_change(&PreferencesChange {
            seen: self.user_preferences_seen.clone(),
            current: self.user_config.user_preferences(),
        });
        let merged = match merge_stored_preferences(&change, &stored.to_stored()) {
            Ok(merged) => {
                UserPreferences::from_stored(merged, &config::default_user_config()).preferences
            }
            Err(error) => {
                tracing::warn!(
                    target: "app_ui::settings",
                    event = "user_preferences_merge_failed",
                    %error,
                    "could not take over settings saved by another Bennu window"
                );
                return Task::none();
            }
        };
        self.user_preferences_seen = stored;
        self.adopt_user_preferences(normalized(merged))
    }

    /// 换上合并后的设置，再让有运行时副本的全局设置跟上。设置页里正在输入的框只在它
    /// 对应的设置真变了时才换掉。
    fn adopt_user_preferences(&mut self, preferences: UserPreferences) -> Task<Message> {
        let before = self.user_config.user_preferences();
        if preferences == before {
            return Task::none();
        }
        preferences.apply_to_user_config(&mut self.user_config);
        let after = preferences;
        if before.language_setting != after.language_setting {
            self.refresh_current_language();
        }
        if before.media_preview_volume != after.media_preview_volume
            || before.media_end_action != after.media_end_action
        {
            self.apply_media_preferences_to_preview();
        }
        self.refresh_preview_engine_config();
        if before.preview_size_limits != after.preview_size_limits {
            self.preview_size_limit_inputs =
                config::preview_size_limit_inputs(&after.preview_size_limits);
            self.preview_size_limit_errors =
                [const { None }; config::PreviewSizeLimitKind::ALL.len()];
        }
        if before.preview_directory_expand_levels != after.preview_directory_expand_levels {
            self.preview_directory_expand_levels_input =
                after.preview_directory_expand_levels.to_string();
            self.preview_directory_expand_levels_error = None;
        }
        if before.startup_custom_directory != after.startup_custom_directory {
            self.invalidate_startup_directory_validation();
            self.startup_custom_directory_input = after
                .startup_custom_directory
                .to_string_lossy()
                .into_owned();
            self.startup_custom_directory_error = None;
        }
        if before.terminal_emulator != after.terminal_emulator {
            self.terminal_emulator = after.terminal_emulator;
        }
        if before.visible_column_count != after.visible_column_count {
            self.forget_column_widths();
        }
        let mut tasks = Vec::new();
        if before.interface_text_size != after.interface_text_size {
            tasks.push(self.reread_preview_window_size());
        }
        if before.network_connections != after.network_connections {
            self.network_connections
                .adopt_saved_connections(after.network_connections.clone());
            tasks.push(self.refresh_network_mount_states());
        }
        if before.sidebar_favorites != after.sidebar_favorites {
            tasks.push(sidebar_locations_command(
                self.home_dir.clone(),
                after.sidebar_favorites.clone(),
            ));
        }
        self.adopt_transfer_preferences(&before);
        Task::batch(tasks)
    }
}

/// 和启动加载同一条路：读出的值先过一遍界面设置的规整（侧栏宽度、栏数等），再和这个
/// 进程手上的比，规整造成的差别不算改动。
fn normalized(preferences: UserPreferences) -> UserPreferences {
    let mut config = config::default_user_config();
    preferences.apply_to_user_config(&mut config);
    config.user_preferences()
}

#[cfg(test)]
mod tests {
    use bennu_theme::ColorSchemePreset;

    use super::*;

    #[test]
    fn settings_saved_elsewhere_are_taken_without_losing_an_unsaved_change_here() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        // 这个窗口刚改、还没存完的。
        browser.user_config.terminal_shell = "fish".to_owned();
        let live_sidebar_width = browser.sidebar_width_when_shown;
        // 另一个窗口存下的。
        let mut stored = config::default_user_config().user_preferences();
        stored.color_scheme = ColorSchemePreset::Catppuccin;
        stored.transfer_port = 50000;
        stored.sidebar_width = live_sidebar_width + 40.0;

        drop(browser.accept_stored_user_preferences(Ok(stored.clone())));

        assert_eq!(
            browser.user_config.color_scheme,
            ColorSchemePreset::Catppuccin
        );
        assert_eq!(browser.user_config.terminal_shell, "fish");
        assert_eq!(browser.transfer_port_input, "50000");
        // 窗口自己的状态：记进设置给新窗口用，这个窗口正在用的不动。
        assert_eq!(browser.user_config.sidebar_width, live_sidebar_width + 40.0);
        assert_eq!(browser.sidebar_width_when_shown, live_sidebar_width);
        assert_eq!(browser.user_preferences_seen, stored);
    }

    #[test]
    fn reading_back_what_this_window_wrote_changes_nothing() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.user_config.color_scheme = ColorSchemePreset::Catppuccin;
        let saved = browser.user_config.user_preferences();
        browser.record_saved_user_preferences(saved.clone());
        browser.transfer_port_input = "being typed".to_owned();

        drop(browser.accept_stored_user_preferences(Ok(saved)));

        assert_eq!(browser.transfer_port_input, "being typed");
    }
}
