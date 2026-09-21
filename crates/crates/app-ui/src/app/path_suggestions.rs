use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Task;

use super::paths::completed_path_text;
use super::FileBrowser;
use crate::commands::path_suggestions_command;
use crate::model::{
    AddressBarTransition, AddressEditingSession, AddressEditingSessionId, AddressSuggestionRequest,
    BrowserPaneId, Message, NavigationMode, PathSuggestionDirection,
};
use crate::view::address_input_id;
use crate::{app::smooth_scroll::smooth_scroll_id, model::ScrollbarRegion};

const PATH_SUGGESTION_INPUT_STABILIZATION_DELAY: Duration = Duration::from_millis(120);

impl FileBrowser {
    pub(super) fn begin_address_editing(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        if self.destructive_action_confirmation.is_some()
            || self.transfer_conflict.is_some()
            || self.archive_creation.is_some()
            || self.archive_extraction.is_some()
            || self.batch_rename.is_some()
            || self.network_connection_editor.is_some()
        {
            return Task::none();
        }

        let Some(pane) = self.pane_view(pane_id) else {
            return Task::none();
        };
        if pane.is_trash_view {
            return Task::none();
        }
        let address_bar_directory = pane.address_bar_directory().to_path_buf();

        if self
            .address_editing
            .as_ref()
            .is_some_and(|session| session.pane_id == pane_id)
        {
            return focus_address_input(pane_id);
        }

        self.activate_pane(pane_id);
        if self.is_trash_view {
            return Task::none();
        }

        self.context_menu = None;
        self.open_with = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();

        let session_id = AddressEditingSessionId(self.next_address_editing_session_id);
        self.next_address_editing_session_id = self.next_address_editing_session_id.wrapping_add(1);
        self.address_editing = Some(AddressEditingSession::new(
            pane_id,
            session_id,
            &address_bar_directory,
        ));
        self.address_bar_transition = Some(AddressBarTransition::retarget(
            self.address_bar_transition.as_ref(),
            pane_id,
            1.0,
            None,
            std::time::Instant::now(),
        ));

        Task::batch([self.commit_rename_if_active(), focus_address_input(pane_id)])
    }

    pub(super) fn update_address_draft(
        &mut self,
        pane_id: BrowserPaneId,
        value: String,
    ) -> Task<Message> {
        if pane_id != self.active_pane_id() {
            return Task::none();
        }
        let address_bar_directory = self.active_address_bar_directory();
        let Some(session) = self
            .address_editing
            .as_mut()
            .filter(|session| session.pane_id == pane_id)
        else {
            return Task::none();
        };

        session.draft = value;
        session.suggestions.clear();
        session.suggestion_selection = None;
        let request = session.next_suggestion_request(&address_bar_directory);
        if request.draft.trim().is_empty() {
            return Task::none();
        }
        path_suggestion_input_stabilization_command(request)
    }

    pub(super) fn load_stable_address_suggestions(
        &self,
        request: AddressSuggestionRequest,
    ) -> Task<Message> {
        if !self.address_suggestion_request_matches(&request) {
            return Task::none();
        }
        path_suggestions_command(request)
    }

    pub(super) fn accept_address_suggestions(
        &mut self,
        request: AddressSuggestionRequest,
        suggestions: Vec<PathBuf>,
    ) -> Task<Message> {
        if !self.address_suggestion_request_matches(&request) {
            return Task::none();
        }

        let Some(session) = self.address_editing.as_mut() else {
            return Task::none();
        };
        session.suggestions = suggestions;
        normalize_address_suggestion_selection(session);
        Task::none()
    }

    pub(super) fn submit_address_editing(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        let address_bar_directory = self.active_address_bar_directory();
        let Some(session) = self
            .address_editing
            .as_ref()
            .filter(|session| session.pane_id == pane_id && pane_id == self.active_pane_id())
        else {
            return Task::none();
        };

        let selected_suggestion = session
            .suggestion_selection
            .and_then(|index| session.suggestions.get(index))
            .cloned();
        let parsed_draft = path_from_address_draft(&session.draft, &address_bar_directory);
        let Some(target) = selected_suggestion.or(parsed_draft) else {
            return self.cancel_address_editing();
        };

        self.finish_address_submission(pane_id, target)
    }

    pub(super) fn submit_address_suggestion(
        &mut self,
        pane_id: BrowserPaneId,
        target: PathBuf,
    ) -> Task<Message> {
        let suggestion_is_current = self.address_editing.as_ref().is_some_and(|session| {
            session.pane_id == pane_id
                && pane_id == self.active_pane_id()
                && session.suggestions.contains(&target)
        });
        if !suggestion_is_current {
            return Task::none();
        }

        self.finish_address_submission(pane_id, target)
    }

    fn finish_address_submission(
        &mut self,
        pane_id: BrowserPaneId,
        target: PathBuf,
    ) -> Task<Message> {
        let Some(session) = self.take_address_editing_session(pane_id) else {
            return Task::none();
        };
        self.start_address_bar_exit(pane_id, session.draft);
        self.navigate_to(target, NavigationMode::RecordHistory)
    }

    pub(super) fn cancel_address_editing(&mut self) -> Task<Message> {
        let Some(session) = self.address_editing.take() else {
            return Task::none();
        };
        self.start_address_bar_exit(session.pane_id, session.draft);
        Task::none()
    }

    fn take_address_editing_session(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Option<AddressEditingSession> {
        if self
            .address_editing
            .as_ref()
            .is_some_and(|session| session.pane_id == pane_id)
        {
            self.address_editing.take()
        } else {
            None
        }
    }

    fn start_address_bar_exit(&mut self, pane_id: BrowserPaneId, snapshot: String) {
        self.address_bar_transition = Some(AddressBarTransition::retarget(
            self.address_bar_transition.as_ref(),
            pane_id,
            0.0,
            Some(snapshot),
            std::time::Instant::now(),
        ));
    }

    pub(super) fn activate_breadcrumb_target(
        &mut self,
        pane_id: BrowserPaneId,
        target: PathBuf,
    ) -> Task<Message> {
        let Some(pane) = self.pane_view(pane_id) else {
            return Task::none();
        };
        if pane.is_trash_view {
            return Task::none();
        }
        if pane.address_bar_directory() == target {
            return self.begin_address_editing(pane_id);
        }

        let cancel_task = self.cancel_address_editing();
        self.activate_pane(pane_id);
        Task::batch([
            cancel_task,
            self.navigate_to(target, NavigationMode::RecordHistory),
        ])
    }

    pub(super) fn move_path_suggestion_selection_from_keyboard(
        &mut self,
        direction: PathSuggestionDirection,
    ) -> Task<Message> {
        let Some(session) = self.active_address_editing_mut() else {
            return Task::none();
        };
        move_address_suggestion_selection(session, direction);
        Task::none()
    }

    pub(super) fn complete_path_suggestion_from_keyboard(
        &mut self,
        direction: PathSuggestionDirection,
    ) -> Task<Message> {
        let active_pane_id = self.active_pane_id();
        let address_bar_directory = self.active_address_bar_directory();
        let Some(session) = self.active_address_editing_mut() else {
            return Task::none();
        };
        if session.suggestions.is_empty() {
            return Task::none();
        }

        if session.suggestion_selection.is_none() {
            session.suggestion_selection = Some(0);
        } else if direction == PathSuggestionDirection::Previous {
            move_address_suggestion_selection(session, direction);
        }

        let Some(path) = session
            .suggestion_selection
            .and_then(|index| session.suggestions.get(index))
            .cloned()
        else {
            return Task::none();
        };

        session.draft = completed_path_text(&path);
        let request = session.next_suggestion_request(&address_bar_directory);
        Task::batch([
            path_suggestions_command(request),
            iced::widget::operation::move_cursor_to_end(address_input_id(active_pane_id)),
        ])
    }

    pub(super) fn address_suggestion_keyboard_is_active(&self) -> bool {
        self.address_editing.as_ref().is_some_and(|session| {
            session.pane_id == self.active_pane_id() && !session.suggestions.is_empty()
        })
    }

    fn active_address_editing_mut(&mut self) -> Option<&mut AddressEditingSession> {
        let active_pane_id = self.active_pane_id();
        self.address_editing
            .as_mut()
            .filter(|session| session.pane_id == active_pane_id)
    }

    fn address_suggestion_request_matches(&self, request: &AddressSuggestionRequest) -> bool {
        let address_bar_directory = self.active_address_bar_directory();
        request.pane_id == self.active_pane_id()
            && self.address_editing.as_ref().is_some_and(|session| {
                session.matches_suggestion_request(request, &address_bar_directory)
            })
    }

    fn active_address_bar_directory(&self) -> PathBuf {
        self.pane_view(self.active_pane_id())
            .expect("active pane must exist")
            .address_bar_directory()
            .to_path_buf()
    }

    pub(super) fn address_bar_transition_is_active(&self) -> bool {
        self.address_bar_transition
            .as_ref()
            .is_some_and(|transition| !transition.is_complete())
    }

    pub(super) fn advance_address_bar_transition(&mut self) -> Task<Message> {
        let should_remove = self
            .address_bar_transition
            .as_ref()
            .is_some_and(|transition| {
                transition.is_complete() && transition.target_fraction() <= f32::EPSILON
            });
        if should_remove {
            self.address_bar_transition = None;
        }
        Task::none()
    }

    pub(super) fn reveal_address_bar_current_segment(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        iced::widget::operation::scroll_to(
            smooth_scroll_id(&ScrollbarRegion::AddressBar(pane_id)),
            iced::widget::scrollable::AbsoluteOffset {
                x: f32::MAX,
                y: 0.0,
            },
        )
        .chain(self.request_breadcrumb_drop_target_bounds_measurement())
    }
}

fn focus_address_input(pane_id: BrowserPaneId) -> Task<Message> {
    let input_id = address_input_id(pane_id);
    Task::batch([
        iced::widget::operation::focus(input_id.clone()),
        iced::widget::operation::select_all(input_id),
    ])
}

fn path_from_address_draft(draft: &str, current_dir: &Path) -> Option<PathBuf> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(current_dir.join(path))
    }
}

fn normalize_address_suggestion_selection(session: &mut AddressEditingSession) {
    if session.suggestions.is_empty() {
        session.suggestion_selection = None;
        return;
    }

    session.suggestion_selection = session
        .suggestion_selection
        .filter(|index| *index < session.suggestions.len());
}

fn move_address_suggestion_selection(
    session: &mut AddressEditingSession,
    direction: PathSuggestionDirection,
) {
    if session.suggestions.is_empty() {
        session.suggestion_selection = None;
        return;
    }

    let last_index = session.suggestions.len() - 1;
    let Some(current_index) = session.suggestion_selection else {
        session.suggestion_selection = Some(match direction {
            PathSuggestionDirection::Next => 0,
            PathSuggestionDirection::Previous => last_index,
        });
        return;
    };

    session.suggestion_selection = Some(match direction {
        PathSuggestionDirection::Next if current_index >= last_index => 0,
        PathSuggestionDirection::Next => current_index + 1,
        PathSuggestionDirection::Previous if current_index == 0 => last_index,
        PathSuggestionDirection::Previous => current_index - 1,
    });
}

fn path_suggestion_input_stabilization_command(request: AddressSuggestionRequest) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(PATH_SUGGESTION_INPUT_STABILIZATION_DELAY).await;
            request
        },
        Message::AddressSuggestionInputStabilized,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_rejects_debounce_and_loaded_results() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.current_dir = PathBuf::from("/tmp");
        let _ = browser.begin_address_editing(BrowserPaneId::PRIMARY);
        let session = browser.address_editing.as_mut().expect("editing session");
        session.draft = "docs".to_owned();
        let request = session.next_suggestion_request(Path::new("/tmp"));

        let _ = browser.cancel_address_editing();
        assert!(!browser.address_suggestion_request_matches(&request));
        let _ = browser.accept_address_suggestions(request, vec![PathBuf::from("/tmp/stale")]);
        assert!(browser.address_editing.is_none());
    }

    #[test]
    fn suggestion_keyboard_requires_active_editing_session() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

        assert!(!browser.address_suggestion_keyboard_is_active());
        let _ = browser.move_path_suggestion_selection_from_keyboard(PathSuggestionDirection::Next);
        assert!(browser.address_editing.is_none());
    }

    #[test]
    fn losing_address_input_focus_cancels_current_editing_session() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let pane_id = BrowserPaneId::PRIMARY;
        let _ = browser.begin_address_editing(pane_id);

        let _ = browser.update(Message::AddressInputFocusChecked(pane_id, false));

        assert!(browser.address_editing.is_none());
    }

    #[test]
    fn column_address_editing_uses_deepest_open_directory_as_base() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let current_dir = PathBuf::from("/workspace");
        let deepest_open_directory = current_dir.join("project/src");
        browser.current_dir = current_dir;
        browser.view_mode = crate::model::BrowserViewMode::Columns;
        browser.deepest_open_column_directory = Some(deepest_open_directory.clone());

        let _ = browser.begin_address_editing(BrowserPaneId::PRIMARY);

        assert_eq!(
            browser
                .address_editing
                .as_ref()
                .map(|session| session.draft.clone()),
            Some(deepest_open_directory.to_string_lossy().into_owned())
        );

        let _ = browser.update_address_draft(BrowserPaneId::PRIMARY, "child".to_owned());
        let _ = browser.submit_address_editing(BrowserPaneId::PRIMARY);

        assert_eq!(browser.current_dir, deepest_open_directory.join("child"));
    }

    #[test]
    fn trash_view_never_begins_address_editing() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.is_trash_view = true;

        let _ = browser.begin_address_editing(BrowserPaneId::PRIMARY);

        assert!(browser.address_editing.is_none());
    }
}
