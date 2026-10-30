use std::time::{Duration, Instant};

use super::FileBrowser;
use crate::model::sanitized_application_log_detail;

const ERROR_NOTIFICATION_DISPLAY_DURATION: Duration = Duration::from_secs(5);

/// 能收回自己报的错的来源：同一件事后来成功了（或重新开始），它的旧错误就过时了。
/// 别的来源、别的事件都动不了这条错误（同 libadwaita：toast 只由创建它的代码撤回）。
/// 不归任何来源的错误只会到期、被用户关闭或被新错误替换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobalErrorSource {
    /// 目录、展开目录与目录元数据的加载。
    DirectoryLoad,
    /// 打开文件、打开方式、打开终端。
    Launch,
    Preview,
    ArchiveMemberOpen,
    /// 复制路径文本、写文件剪贴板。
    ClipboardWrite,
    FileOperationEnqueue,
}

pub(super) struct GlobalErrorNotification {
    message: String,
    source: Option<GlobalErrorSource>,
    generation: u64,
    remaining: Duration,
    countdown_started_at: Option<Instant>,
}

impl GlobalErrorNotification {
    fn new(message: String, source: Option<GlobalErrorSource>, generation: u64) -> Self {
        Self {
            message,
            source,
            generation,
            remaining: ERROR_NOTIFICATION_DISPLAY_DURATION,
            countdown_started_at: Some(Instant::now()),
        }
    }

    fn new_paused(message: String, source: Option<GlobalErrorSource>, generation: u64) -> Self {
        Self {
            message,
            source,
            generation,
            remaining: ERROR_NOTIFICATION_DISPLAY_DURATION,
            countdown_started_at: None,
        }
    }

    fn pause(&mut self, now: Instant) {
        let Some(started_at) = self.countdown_started_at.take() else {
            return;
        };

        self.remaining = self
            .remaining
            .saturating_sub(now.saturating_duration_since(started_at));
    }

    fn resume(&mut self, now: Instant) {
        self.countdown_started_at = Some(now);
    }
}

#[cfg(test)]
std::thread_local! {
    static RECORDED_GLOBAL_ERRORS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn record_global_error(log_error: &str) {
    tracing::error!(
        target: "app_ui::global_error",
        event = "global_error_displayed",
        error = %log_error,
        "global application error displayed"
    );
    #[cfg(test)]
    RECORDED_GLOBAL_ERRORS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn reset_recorded_global_errors() {
    RECORDED_GLOBAL_ERRORS.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn recorded_global_error_count() -> usize {
    RECORDED_GLOBAL_ERRORS.with(std::cell::Cell::get)
}

impl FileBrowser {
    pub(crate) fn current_error(&self) -> Option<&str> {
        self.global_error_notification
            .as_ref()
            .map(|notification| notification.message.as_str())
    }

    pub(crate) fn current_error_notification(&self) -> Option<(&str, u64)> {
        self.global_error_notification
            .as_ref()
            .map(|notification| (notification.message.as_str(), notification.generation))
    }

    pub(super) fn global_error_notification_countdown(&self) -> Option<(u64, Duration)> {
        self.global_error_notification
            .as_ref()
            .and_then(|notification| {
                notification
                    .countdown_started_at
                    .is_some()
                    .then_some((notification.generation, notification.remaining))
                    .filter(|(_, remaining)| !remaining.is_zero())
            })
    }

    /// 不归任何来源的错误：只会到期、被用户关闭或被新错误替换。
    pub(super) fn show_global_error(&mut self, error: impl Into<String>) {
        self.show_global_error_notification(error.into(), None);
    }

    /// 归 `source` 所有的错误：该来源后来成功或重新开始时由 `retract_global_error` 收回。
    pub(super) fn show_global_error_from(
        &mut self,
        source: GlobalErrorSource,
        error: impl Into<String>,
    ) {
        self.show_global_error_notification(error.into(), Some(source));
    }

    /// 只收回 `source` 自己报的、仍在显示的错误。
    pub(super) fn retract_global_error(&mut self, source: GlobalErrorSource) {
        if self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| notification.source == Some(source))
        {
            self.clear_global_error();
        }
    }

    /// 用户动作接管了界面（打开模态框）时关掉任何错误。
    pub(super) fn dismiss_global_error(&mut self) {
        self.clear_global_error();
    }

    fn show_global_error_notification(&mut self, error: String, source: Option<GlobalErrorSource>) {
        let log_error = sanitized_application_log_detail(&error);
        record_global_error(&log_error);
        let notification_was_paused = self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| notification.countdown_started_at.is_none());
        let generation = self.next_global_error_notification_generation();
        self.global_error_notification = Some(if notification_was_paused {
            GlobalErrorNotification::new_paused(error, source, generation)
        } else {
            GlobalErrorNotification::new(error, source, generation)
        });
    }

    fn clear_global_error(&mut self) {
        self.global_error_notification = None;
    }

    pub(super) fn pause_global_error_notification(&mut self, generation: u64) {
        let should_pause = self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| {
                notification.generation == generation && notification.countdown_started_at.is_some()
            });
        if !should_pause {
            return;
        }

        let next_generation = self.next_global_error_notification_generation();
        let remaining = match self.global_error_notification.as_mut() {
            Some(notification) => {
                notification.pause(Instant::now());
                notification.remaining
            }
            None => return,
        };
        if remaining.is_zero() {
            self.clear_global_error();
        } else if let Some(notification) = self.global_error_notification.as_mut() {
            notification.generation = next_generation;
        }
    }

    pub(super) fn resume_global_error_notification(&mut self, generation: u64) {
        let should_resume = self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| {
                notification.generation == generation && notification.countdown_started_at.is_none()
            });
        if !should_resume {
            return;
        }

        let next_generation = self.next_global_error_notification_generation();
        if let Some(notification) = self.global_error_notification.as_mut() {
            notification.generation = next_generation;
            notification.resume(Instant::now());
        }
    }

    pub(super) fn expire_global_error_notification(&mut self, generation: u64) {
        if self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| {
                notification.generation == generation && notification.countdown_started_at.is_some()
            })
        {
            self.clear_global_error();
        }
    }

    pub(super) fn dismiss_global_error_notification(&mut self, generation: u64) {
        if self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| notification.generation == generation)
        {
            self.clear_global_error();
        }
    }

    fn next_global_error_notification_generation(&mut self) -> u64 {
        self.next_global_error_notification_generation = self
            .next_global_error_notification_generation
            .wrapping_add(1);
        self.next_global_error_notification_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::model::Message;

    #[test]
    fn global_error_keeps_only_the_current_toast_and_records_once() {
        reset_recorded_global_errors();
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        browser.show_global_error("smb://alice:secret@example.test/share");
        assert_eq!(
            browser.current_error(),
            Some("smb://alice:secret@example.test/share")
        );
        assert_eq!(
            browser
                .global_error_notification_countdown()
                .map(|(_, remaining)| remaining),
            Some(ERROR_NOTIFICATION_DISPLAY_DURATION)
        );
        assert_eq!(recorded_global_error_count(), 1);

        browser.dismiss_global_error();
        assert_eq!(browser.current_error(), None);
        browser.show_global_error("replacement failure");
        assert_eq!(browser.current_error(), Some("replacement failure"));
    }

    #[test]
    fn a_source_retracts_only_its_own_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error_from(GlobalErrorSource::Launch, "could not open");

        browser.retract_global_error(GlobalErrorSource::DirectoryLoad);
        assert_eq!(browser.current_error(), Some("could not open"));

        browser.retract_global_error(GlobalErrorSource::Launch);
        assert_eq!(browser.current_error(), None);
    }

    #[test]
    fn no_source_retracts_an_error_that_belongs_to_none() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("settings were reset");

        for source in [
            GlobalErrorSource::DirectoryLoad,
            GlobalErrorSource::Launch,
            GlobalErrorSource::Preview,
            GlobalErrorSource::ArchiveMemberOpen,
            GlobalErrorSource::ClipboardWrite,
            GlobalErrorSource::FileOperationEnqueue,
        ] {
            browser.retract_global_error(source);
        }

        assert_eq!(browser.current_error(), Some("settings were reset"));
    }

    #[test]
    fn reloading_the_directory_keeps_an_error_reported_elsewhere() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("settings were reset");

        drop(browser.reload_visible_panes());

        assert_eq!(browser.current_error(), Some("settings were reset"));
    }

    #[test]
    fn reloading_the_directory_retracts_its_own_earlier_load_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error_from(GlobalErrorSource::DirectoryLoad, "permission denied");

        drop(browser.reload_visible_panes());

        assert_eq!(browser.current_error(), None);
    }

    #[test]
    fn error_notification_pause_preserves_remaining_duration() {
        let started_at = Instant::now();
        let mut notification = GlobalErrorNotification::new("failure".to_owned(), None, 1);
        notification.countdown_started_at = Some(started_at);

        notification.pause(started_at + Duration::from_secs(2));
        assert_eq!(notification.remaining, Duration::from_secs(3));
        assert!(notification.countdown_started_at.is_none());

        notification.resume(started_at + Duration::from_secs(2));
        assert_eq!(notification.remaining, Duration::from_secs(3));
        assert_eq!(
            notification.countdown_started_at,
            Some(started_at + Duration::from_secs(2))
        );
    }

    #[test]
    fn stale_error_notification_events_do_not_clear_replacement() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("first failure");
        let first_generation = browser
            .current_error_notification()
            .expect("first notification")
            .1;

        browser.show_global_error("second failure");
        let second_generation = browser
            .current_error_notification()
            .expect("second notification")
            .1;
        assert_ne!(first_generation, second_generation);

        drop(browser.update(Message::GlobalErrorNotificationElapsed(first_generation)));
        drop(browser.update(Message::GlobalErrorNotificationDismissed(first_generation)));
        assert_eq!(browser.current_error(), Some("second failure"));

        drop(browser.update(Message::GlobalErrorNotificationElapsed(second_generation)));
        assert_eq!(browser.current_error(), None);
    }

    #[test]
    fn replacement_while_hovered_remains_paused_until_pointer_leaves() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("first failure");
        let first_generation = browser
            .current_error_notification()
            .expect("first notification")
            .1;

        drop(
            browser.update(Message::GlobalErrorNotificationPointerEntered(
                first_generation,
            )),
        );
        browser.show_global_error("second failure");
        let (_, second_generation) = browser
            .current_error_notification()
            .expect("second notification");
        assert_eq!(browser.global_error_notification_countdown(), None);

        drop(
            browser.update(Message::GlobalErrorNotificationPointerExited(
                second_generation,
            )),
        );
        assert_eq!(
            browser
                .global_error_notification_countdown()
                .map(|(_, remaining)| remaining),
            Some(ERROR_NOTIFICATION_DISPLAY_DURATION)
        );
    }
    #[test]
    fn stale_elapsed_events_cannot_clear_a_notification_after_pause_and_resume() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("failure");
        let active_generation = browser
            .current_error_notification()
            .expect("active notification")
            .1;

        drop(
            browser.update(Message::GlobalErrorNotificationPointerEntered(
                active_generation,
            )),
        );
        let paused_generation = browser
            .current_error_notification()
            .expect("paused notification")
            .1;
        assert_ne!(paused_generation, active_generation);
        assert_eq!(browser.global_error_notification_countdown(), None);
        drop(browser.update(Message::GlobalErrorNotificationElapsed(active_generation)));
        assert_eq!(browser.current_error(), Some("failure"));

        drop(
            browser.update(Message::GlobalErrorNotificationPointerExited(
                paused_generation,
            )),
        );
        let resumed_generation = browser
            .current_error_notification()
            .expect("resumed notification")
            .1;
        assert_ne!(resumed_generation, paused_generation);
        drop(browser.update(Message::GlobalErrorNotificationElapsed(paused_generation)));
        assert_eq!(browser.current_error(), Some("failure"));

        drop(browser.update(Message::GlobalErrorNotificationElapsed(resumed_generation)));
        assert_eq!(browser.current_error(), None);
    }
}
