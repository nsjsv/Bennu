use std::time::{Duration, Instant};

use iced::Task;

use super::FileBrowser;
use crate::commands::save_browser_session_command;
use crate::model::{pane_session_from_live, BrowserSessionSnapshot, Message};

const SESSION_SAVE_INTERVAL: Duration = Duration::from_millis(500);

impl FileBrowser {
    pub(super) fn should_save_browser_session(&self) -> bool {
        self.application_launch_request
            .allows_browser_session_persistence()
            && self.user_config.startup_location_policy.saves_view_state()
    }

    pub(super) fn browser_session_snapshot(&mut self) -> BrowserSessionSnapshot {
        self.sync_active_tab_state();
        let panes = self
            .panes
            .iter()
            .map(pane_session_from_live)
            .collect::<Vec<_>>();
        BrowserSessionSnapshot {
            panes,
            layout: self.pane_layout,
        }
    }

    pub(super) fn request_browser_session_save(&mut self) -> Task<Message> {
        if !self.should_save_browser_session() {
            self.pending_browser_session_save = false;
            return Task::none();
        }
        let Some(store) = self.operation_queue.task_queue_store().cloned() else {
            self.pending_browser_session_save = true;
            return Task::none();
        };
        let now = Instant::now();
        if self
            .last_browser_session_save
            .is_some_and(|last| now.duration_since(last) < SESSION_SAVE_INTERVAL)
        {
            self.pending_browser_session_save = true;
            return browser_session_save_delay_command();
        }
        self.last_browser_session_save = Some(now);
        self.pending_browser_session_save = false;
        self.start_browser_session_save(store)
    }

    pub(super) fn flush_browser_session_save(&mut self) -> Task<Message> {
        if !self.should_save_browser_session() {
            self.pending_browser_session_save = false;
            return Task::none();
        }
        let Some(store) = self.operation_queue.task_queue_store().cloned() else {
            return Task::none();
        };
        self.last_browser_session_save = Some(Instant::now());
        self.pending_browser_session_save = false;
        self.start_browser_session_save(store)
    }

    fn start_browser_session_save(
        &mut self,
        store: file_operation_store::TaskQueueStore,
    ) -> Task<Message> {
        let snapshot = self.browser_session_snapshot();
        self.browser_session_saves_in_flight += 1;
        save_browser_session_command(store, snapshot)
    }

    pub(super) fn record_browser_session_save_outcome(
        &mut self,
        outcome: Result<(), String>,
    ) -> Result<(), String> {
        assert!(
            self.browser_session_saves_in_flight > 0,
            "browser session save outcome without an in-flight save"
        );
        self.browser_session_saves_in_flight -= 1;
        outcome
    }

    pub(super) fn maybe_flush_pending_browser_session_save(&mut self) -> Task<Message> {
        if !self.pending_browser_session_save {
            return Task::none();
        }
        let Some(last) = self.last_browser_session_save else {
            return self.request_browser_session_save();
        };
        if last.elapsed() >= SESSION_SAVE_INTERVAL {
            return self.flush_browser_session_save();
        }
        Task::none()
    }
}

fn browser_session_save_delay_command() -> Task<Message> {
    Task::perform(
        async {
            tokio::time::sleep(SESSION_SAVE_INTERVAL).await;
        },
        |_| Message::BrowserSessionSaveDelayElapsed,
    )
}
