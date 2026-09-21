use std::time::{Duration, Instant};

use super::FileBrowser;
use crate::model::sanitized_application_log_detail;

const ERROR_NOTIFICATION_DISPLAY_DURATION: Duration = Duration::from_secs(5);

pub(super) struct GlobalErrorNotification {
    message: String,
    generation: u64,
    remaining: Duration,
    countdown_started_at: Option<Instant>,
}

impl GlobalErrorNotification {
    fn new(message: String, generation: u64) -> Self {
        Self {
            message,
            generation,
            remaining: ERROR_NOTIFICATION_DISPLAY_DURATION,
            countdown_started_at: Some(Instant::now()),
        }
    }

    fn new_paused(message: String, generation: u64) -> Self {
        Self {
            message,
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

    pub(super) fn show_global_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        let log_error = sanitized_application_log_detail(&error);
        record_global_error(&log_error);
        let notification_was_paused = self
            .global_error_notification
            .as_ref()
            .is_some_and(|notification| notification.countdown_started_at.is_none());
        let generation = self.next_global_error_notification_generation();
        self.global_error_notification = Some(if notification_was_paused {
            GlobalErrorNotification::new_paused(error, generation)
        } else {
            GlobalErrorNotification::new(error, generation)
        });
    }

    pub(super) fn clear_global_error(&mut self) {
        self.global_error_notification = None;
    }

    pub(super) fn replace_global_error(&mut self, error: Option<String>) {
        self.clear_global_error();
        if let Some(error) = error {
            self.show_global_error(error);
        }
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

        browser.clear_global_error();
        assert_eq!(browser.current_error(), None);
        browser.replace_global_error(Some("replacement failure".to_owned()));
        assert_eq!(browser.current_error(), Some("replacement failure"));
    }

    #[test]
    fn error_notification_pause_preserves_remaining_duration() {
        let started_at = Instant::now();
        let mut notification = GlobalErrorNotification::new("failure".to_owned(), 1);
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
