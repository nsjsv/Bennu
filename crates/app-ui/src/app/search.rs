use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Local;
use file_search::{
    SearchError, SearchExcludeRules, SearchHit, SearchMatchMode, SearchQuery, SearchScope,
    SearchTextScope,
};
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::{FileBrowser, DOUBLE_CLICK_THRESHOLD};
use crate::commands::{
    directory_fallback_search_command, open_file_command, read_search_path_configuration,
    search_command, search_service_recovery_command, search_service_status_command,
    search_with_scope_root_check_command,
};
use crate::model::search::SEARCH_RESULT_WINDOW;
use crate::model::{
    BrowserViewMode, ContextMenuState, DestructiveActionConfirmation, DirectoryFallbackOutcome,
    IndexedSearchOutcome, IndexedSearchRequest, LastActivationClick, Message, NavigationMode,
    SearchContextMenuState, SearchDateField, SearchDatePreset, SearchDirectoryScope,
    SearchEntryTypeMenuState, SearchEntryTypePreset, SearchInputStabilizationRequest,
    SearchInputStabilizationSubject, SearchKeyboardSelection, SearchSelectionGesture,
    SearchSelectionStep, SearchServiceDiagnostic, SearchServiceDiagnosticKind,
    SearchServiceRecoveryAction, SearchServiceStatusRequest, SearchWorkspaceSessionId,
    SearchWorkspaceState,
};
use crate::shortcuts::{FileSelectionDirection, ShortcutAction};

const SEARCH_INPUT_STABILIZATION_DELAY: Duration = Duration::from_millis(120);
const SEARCH_PAGE_PREFETCH_ROWS: usize = 12;

impl FileBrowser {
    pub(crate) fn invoke_search_workspace_shortcut(
        &mut self,
        action: ShortcutAction,
    ) -> Task<Message> {
        match action {
            ShortcutAction::OpenSelected => self.activate_selected_search_result(),
            ShortcutAction::MoveSelection(direction) => {
                self.move_search_result_selection(direction)
            }
            ShortcutAction::SelectAll => self.select_all_search_results(),
            ShortcutAction::Copy => self.copy_selected(),
            ShortcutAction::Cut => self.move_selected(),
            ShortcutAction::Delete => self.trash_selected(),
            ShortcutAction::Refresh => self.submit_search(),
            ShortcutAction::Undo => self.undo_file_operation(),
            ShortcutAction::Redo => self.redo_file_operation(),
            ShortcutAction::Escape
                if self
                    .search_history_interaction
                    .popup_is_visible(&self.user_config.search_history) =>
            {
                self.search_history_interaction.dismiss_popup();
                Task::none()
            }
            ShortcutAction::Escape if !self.file_browser_content_shortcuts_enabled() => {
                self.handle_focused_window_escape_pressed()
            }
            ShortcutAction::Escape => self.close_search_workspace(),
            ShortcutAction::RenameSelected
            | ShortcutAction::FocusPathInput
            | ShortcutAction::NavigateBack
            | ShortcutAction::NavigateForward
            | ShortcutAction::NavigateUp
            | ShortcutAction::FileProperties
            | ShortcutAction::Preview
            | ShortcutAction::ToggleRightPreviewPanel
            | ShortcutAction::ToggleTerminal
            | ShortcutAction::TerminalNewTab
            | ShortcutAction::TerminalCloseTab
            | ShortcutAction::DuplicateSelected
            | ShortcutAction::NewFolderFromSelection
            | ShortcutAction::Paste => Task::none(),
        }
    }

    pub(super) fn update_search_input(&mut self, value: String) -> Task<Message> {
        if let Err(message) = self.ensure_search_workspace() {
            self.show_global_error(message);
            return Task::none();
        }
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        let request = workspace.replace_input(value);
        search_input_stabilization_command(request)
    }

    pub(super) fn update_search_custom_extensions(&mut self, value: String) -> Task<Message> {
        if let Err(message) = self.ensure_search_workspace() {
            self.show_global_error(message);
            return Task::none();
        }
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        let request = workspace.replace_custom_extensions(value);
        search_input_stabilization_command(request)
    }

    pub(super) fn toggle_search_custom_extensions(&mut self) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.toggle_custom_extensions();
        self.restart_search_workspace()
    }

    pub(super) fn accept_search_input_stabilization(
        &mut self,
        request: SearchInputStabilizationRequest,
    ) -> Task<Message> {
        let accepted = match request.subject {
            SearchInputStabilizationSubject::Terms => self
                .search_workspace
                .as_ref()
                .is_some_and(|workspace| workspace.accepts_input_stabilization(&request)),
            SearchInputStabilizationSubject::CustomExtensions => {
                self.search_workspace.as_ref().is_some_and(|workspace| {
                    workspace.accepts_custom_extensions_stabilization(&request)
                })
            }
        };
        if !accepted {
            return Task::none();
        }
        self.restart_search_workspace()
    }

    pub(super) fn submit_search_input(&mut self) -> Task<Message> {
        if let Err(message) = self.ensure_search_workspace() {
            self.show_global_error(message);
            return Task::none();
        }
        let keyword = self
            .search_workspace
            .as_ref()
            .map(|workspace| workspace.input.clone())
            .unwrap_or_default();
        let persist = if self.user_config.search_history.record_submission(&keyword) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        };
        self.search_history_interaction.dismiss_popup();
        Task::batch([persist, self.restart_search_workspace()])
    }

    pub(super) fn submit_search(&mut self) -> Task<Message> {
        if let Err(message) = self.ensure_search_workspace() {
            self.show_global_error(message);
            return Task::none();
        }
        self.restart_search_workspace()
    }

    pub(super) fn select_search_history_keyword(&mut self, keyword: String) -> Task<Message> {
        if !self.user_config.search_history.contains(&keyword) {
            return Task::none();
        }
        if let Err(message) = self.ensure_search_workspace() {
            self.show_global_error(message);
            return Task::none();
        }
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.replace_input_immediately(keyword.clone());
        self.search_history_interaction.reset();
        let persist = if self.user_config.search_history.record_submission(&keyword) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        };
        Task::batch([persist, self.restart_search_workspace()])
    }

    pub(super) fn remove_search_history_keyword(&mut self, keyword: &str) -> Task<Message> {
        if self.user_config.search_history.remove(keyword) {
            if self.user_config.search_history.entries().is_empty() {
                self.search_history_interaction.dismiss_popup();
            }
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn clear_search_history(&mut self) -> Task<Message> {
        if self.user_config.search_history.clear() {
            self.search_history_interaction.dismiss_popup();
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn clear_search_keyword(&mut self) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.input.clear();
        self.restart_search_workspace()
    }

    pub(super) fn open_search_entry_types_menu(&mut self) -> Task<Message> {
        if self.search_workspace.is_none()
            || self
                .user_config
                .context_menus
                .search_entry_type_items()
                .is_empty()
        {
            return Task::none();
        }
        self.context_menu = Some(ContextMenuState::SearchEntryTypes(
            SearchEntryTypeMenuState {
                position: self.cursor_position,
            },
        ));
        Task::none()
    }

    pub(super) fn toggle_search_entry_type(
        &mut self,
        entry_type: SearchEntryTypePreset,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.toggle_entry_type(entry_type);
        self.restart_search_workspace()
    }

    pub(super) fn select_search_directory_scope(
        &mut self,
        scope: SearchDirectoryScope,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        if !workspace.root.select_scope(scope) {
            return Task::none();
        }
        self.restart_search_workspace()
    }

    pub(super) fn select_search_text_scope(
        &mut self,
        text_scope: SearchTextScope,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.text_scope = text_scope;
        self.restart_search_workspace()
    }

    pub(super) fn toggle_search_regex(&mut self) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.match_mode = match workspace.filters.match_mode {
            SearchMatchMode::Plain => SearchMatchMode::Regex,
            SearchMatchMode::Regex => SearchMatchMode::Plain,
        };
        self.restart_search_workspace()
    }

    pub(super) fn select_search_date_field(
        &mut self,
        date_field: SearchDateField,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.date_field = date_field;
        self.restart_search_workspace()
    }

    pub(super) fn select_search_date_preset(
        &mut self,
        date_preset: SearchDatePreset,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.date_preset = date_preset;
        self.restart_search_workspace()
    }

    pub(super) fn reset_search_filters(&mut self) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.filters.reset();
        self.restart_search_workspace()
    }

    pub(super) fn close_search_workspace(&mut self) -> Task<Message> {
        self.discard_search_workspace();
        Task::none()
    }

    pub(super) fn discard_search_workspace(&mut self) {
        self.search_workspace = None;
        self.search_history_interaction.reset();
        self.last_activation_click = None;
        if matches!(
            self.context_menu,
            Some(ContextMenuState::Search(_) | ContextMenuState::SearchEntryTypes(_))
        ) {
            self.context_menu = None;
        }
    }

    fn ensure_search_workspace(&mut self) -> Result<(), String> {
        if self.search_workspace.is_some() {
            return Ok(());
        }
        if self.is_trash_view {
            return Err("Search is available from a local folder".to_owned());
        }
        let root = self.current_search_directory();
        if self.path_is_remote_mount(&root) {
            return Err("Search is unavailable for remote folders".to_owned());
        }
        let session_id = SearchWorkspaceSessionId(self.next_search_workspace_session_id);
        self.next_search_workspace_session_id =
            self.next_search_workspace_session_id.wrapping_add(1);
        let home = self.home_dir.clone();
        self.clear_pointer_driven_interaction_state();
        self.search_workspace = Some(SearchWorkspaceState::new(root, home, session_id));
        Ok(())
    }

    pub(super) fn restart_search_workspace(&mut self) -> Task<Message> {
        if let Some(workspace) = self.search_workspace.as_mut() {
            workspace.invalidate_input_stabilization();
        }
        let Some(workspace) = self.search_workspace.as_ref() else {
            return Task::none();
        };
        if workspace.input.trim().is_empty() && !workspace.filters.custom_extensions_are_active() {
            if let Some(workspace) = self.search_workspace.as_mut() {
                workspace.clear_query();
            }
            return Task::none();
        }
        let generation = workspace.next_generation();
        let scope = workspace.root.query_scope();
        let filters = match workspace.filters.query_filters_at(Local::now()) {
            Ok(filters) => filters,
            Err(message) => {
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.reject_query(message);
                }
                return Task::none();
            }
        };
        let terms = workspace.input.trim().to_owned();
        let match_mode = workspace.filters.match_mode;
        if let SearchMatchMode::Regex = match_mode {
            if let Err(SearchError::InvalidQuery(message)) = match_mode.name_regex(&terms) {
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.reject_query(format!("Invalid regular expression: {message}"));
                }
                return Task::none();
            }
        }
        let query = SearchQuery {
            query_id: generation,
            terms,
            // 正则只在名称上执行，内容范围对正则语义不可用。
            text_scope: if match_mode == SearchMatchMode::Regex {
                SearchTextScope::NameOnly
            } else {
                workspace.filters.text_scope
            },
            match_mode,
            scope,
            recursive: true,
            filters,
            limit: SEARCH_RESULT_WINDOW,
            cursor: None,
        };
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        let (request, cancellation) = workspace.begin_indexed_query(query.clone());
        search_with_scope_root_check_command(request, query, cancellation)
    }

    pub(super) fn accept_search_scope_root_validation(
        &mut self,
        request: IndexedSearchRequest,
        query: SearchQuery,
        cancellation: CancellationToken,
        root_is_available: bool,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_ref() else {
            return Task::none();
        };
        if !workspace.accepts_indexed_outcome(request.clone()) {
            // 过期校验结果：期间查询已重启，直接丢弃。
            return Task::none();
        }
        if !root_is_available {
            let message = match &query.scope {
                SearchScope::Directory(root) => {
                    format!("Search root is unavailable: {}", root.display())
                }
                _ => "Search root is unavailable".to_owned(),
            };
            if let Some(workspace) = self.search_workspace.as_mut() {
                workspace.reject_query(message);
            }
            return Task::none();
        }
        search_command(request, query, cancellation)
    }

    pub(super) fn accept_search_results(
        &mut self,
        request: IndexedSearchRequest,
        outcome: IndexedSearchOutcome,
    ) -> Task<Message> {
        if !self
            .search_workspace
            .as_ref()
            .is_some_and(|workspace| workspace.accepts_indexed_outcome(request))
        {
            return Task::none();
        }
        match outcome {
            IndexedSearchOutcome::Cancelled => {
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.apply_indexed_cancellation(request);
                }
            }
            IndexedSearchOutcome::Batch(batch) => {
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.apply_indexed_batch(request, batch);
                }
            }
            IndexedSearchOutcome::TransportUnavailable(message) => {
                self.search_service
                    .observe_query_transport_failure(&message);
                if request.cursor.is_none() {
                    return self.switch_to_directory_fallback(message);
                }
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.apply_indexed_failure(request, message);
                }
            }
            IndexedSearchOutcome::ProviderUnavailable(message) => {
                if request.cursor.is_none() {
                    return self.switch_to_directory_fallback(message);
                }
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.apply_indexed_failure(request, message);
                }
            }
            IndexedSearchOutcome::InvalidQuery(message) | IndexedSearchOutcome::Fatal(message) => {
                if let Some(workspace) = self.search_workspace.as_mut() {
                    workspace.apply_indexed_failure(request, message);
                }
            }
        }
        Task::none()
    }

    pub(super) fn update_search_results_viewport(
        &mut self,
        offset_y: f32,
        viewport_height: f32,
    ) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.update_viewport(offset_y, viewport_height);
        let content_height =
            workspace.window.hits.len() as f32 * crate::view::SEARCH_RESULT_ROW_HEIGHT;
        let remaining_height = content_height - offset_y - viewport_height;
        if remaining_height
            > SEARCH_PAGE_PREFETCH_ROWS as f32 * crate::view::SEARCH_RESULT_ROW_HEIGHT
            || !workspace.indexed_next_page_is_available()
        {
            return Task::none();
        }
        let Some((request, query, cancellation)) = workspace.begin_next_indexed_page() else {
            return Task::none();
        };
        search_command(request, query, cancellation)
    }

    pub(super) fn accept_directory_search_batch(
        &mut self,
        generation: u64,
        hits: Vec<SearchHit>,
    ) -> Task<Message> {
        if let Some(workspace) = self
            .search_workspace
            .as_mut()
            .filter(|workspace| workspace.accepts_directory_fallback(generation))
        {
            workspace.apply_directory_batch(hits);
        }
        Task::none()
    }

    pub(super) fn accept_directory_search_finished(
        &mut self,
        generation: u64,
        completion: DirectoryFallbackOutcome,
    ) -> Task<Message> {
        if let Some(workspace) = self
            .search_workspace
            .as_mut()
            .filter(|workspace| workspace.accepts_directory_fallback(generation))
        {
            workspace.finish_directory_fallback(completion);
        }
        Task::none()
    }

    pub(super) fn press_search_result(&mut self, path: PathBuf) -> Task<Message> {
        let has_selection_modifier =
            self.keyboard_modifiers.control() || self.keyboard_modifiers.shift();
        let now = Instant::now();
        let is_double_click = !has_selection_modifier
            && self
                .last_activation_click
                .as_ref()
                .is_some_and(|last_click| {
                    last_click.path == path
                        && now.duration_since(last_click.at) <= DOUBLE_CLICK_THRESHOLD
                });
        let gesture = match (
            self.keyboard_modifiers.control(),
            self.keyboard_modifiers.shift(),
        ) {
            (true, true) => SearchSelectionGesture::AdditiveRange,
            (false, true) => SearchSelectionGesture::Range,
            (true, false) => SearchSelectionGesture::Toggle,
            (false, false) => SearchSelectionGesture::Plain,
        };
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace
            .selection
            .select(&workspace.window.hits, &path, gesture);
        self.context_menu = None;
        self.last_activation_click = if has_selection_modifier {
            None
        } else {
            Some(LastActivationClick {
                path: path.clone(),
                at: now,
            })
        };

        if is_double_click {
            self.activate_search_path(path)
        } else {
            Task::none()
        }
    }

    pub(super) fn right_click_search_result(&mut self, path: PathBuf) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        if !workspace.selection.is_selected(&path) {
            workspace.selection.select(
                &workspace.window.hits,
                &path,
                SearchSelectionGesture::Plain,
            );
        }
        if !self.user_config.context_menus.search_items().is_empty() {
            self.context_menu = Some(ContextMenuState::Search(SearchContextMenuState {
                target: path,
                position: self.cursor_position,
            }));
        }
        Task::none()
    }

    pub(crate) fn activate_selected_search_result(&mut self) -> Task<Message> {
        let Some(path) = self
            .search_workspace
            .as_ref()
            .and_then(|workspace| workspace.selection.focused_path())
            .map(Path::to_path_buf)
        else {
            return Task::none();
        };
        self.activate_search_path(path)
    }

    pub(crate) fn move_search_result_selection(
        &mut self,
        direction: FileSelectionDirection,
    ) -> Task<Message> {
        let step = match direction {
            FileSelectionDirection::Up => SearchSelectionStep::Previous,
            FileSelectionDirection::Down => SearchSelectionStep::Next,
            FileSelectionDirection::Left | FileSelectionDirection::Right => return Task::none(),
        };
        let keyboard_selection = if self.keyboard_modifiers.shift() {
            SearchKeyboardSelection::Extend
        } else {
            SearchKeyboardSelection::Replace
        };
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        let Some(target) =
            workspace
                .selection
                .move_focus(&workspace.window.hits, step, keyboard_selection)
        else {
            return Task::none();
        };
        let Some(index) = workspace
            .window
            .hits
            .iter()
            .position(|hit| hit.path == target)
        else {
            return Task::none();
        };
        Task::batch([
            iced::widget::operation::scroll_to(
                crate::app::smooth_scroll::smooth_scroll_id(
                    &crate::model::ScrollbarRegion::SearchResults,
                ),
                iced::widget::scrollable::AbsoluteOffset {
                    x: 0.0,
                    y: index as f32 * crate::view::SEARCH_RESULT_ROW_HEIGHT,
                },
            ),
            self.show_scrollbars_temporarily(crate::model::ScrollbarRegion::SearchResults),
        ])
    }

    pub(crate) fn select_all_search_results(&mut self) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        workspace.selection.select_all(&workspace.window.hits);
        Task::none()
    }

    pub(crate) fn active_search_selection(&self) -> Option<Vec<PathBuf>> {
        self.search_workspace
            .as_ref()
            .map(SearchWorkspaceState::selected_paths_in_result_order)
    }

    pub(super) fn delete_search_selection_permanently(&mut self) -> Task<Message> {
        self.context_menu = None;
        let Some(paths) = self.active_search_selection() else {
            return Task::none();
        };
        if paths.is_empty() {
            return Task::none();
        }
        self.request_destructive_action_confirmation(
            DestructiveActionConfirmation::DeletePermanently { paths },
        );
        Task::none()
    }

    pub(super) fn open_search_containing_directory(&mut self, path: PathBuf) -> Task<Message> {
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            self.fail_search_workspace("The result has no containing directory".to_owned());
            return Task::none();
        };
        if !parent.is_dir() {
            self.fail_search_workspace(format!(
                "Containing directory is unavailable: {}",
                parent.display()
            ));
            return Task::none();
        }

        self.discard_search_workspace();
        let navigation = self.navigate_to(parent, NavigationMode::RecordHistory);
        self.select_path(path);
        Task::batch([navigation, self.request_browser_session_save()])
    }

    fn activate_search_path(&mut self, path: PathBuf) -> Task<Message> {
        let Some(hit) = self
            .search_workspace
            .as_ref()
            .and_then(|workspace| workspace.hit_for_path(&path))
            .cloned()
        else {
            return Task::none();
        };
        if hit.kind == file_search::SearchFileKind::Directory {
            self.discard_search_workspace();
            self.navigate_to(hit.path, NavigationMode::RecordHistory)
        } else {
            open_file_command(hit.path, self.terminal_emulator)
        }
    }

    fn fail_search_workspace(&mut self, message: String) {
        if let Some(workspace) = self.search_workspace.as_mut() {
            workspace.window.failure = Some(message);
            workspace.window.is_loading = false;
        }
    }

    pub(super) fn restart_search_service(&mut self) -> Task<Message> {
        self.search_service
            .begin_restart()
            .map(search_service_recovery_command)
            .unwrap_or_else(Task::none)
    }

    pub(super) fn press_force_restart_search_service(&mut self) -> Task<Message> {
        self.search_service
            .press_force_restart()
            .map(search_service_recovery_command)
            .unwrap_or_else(Task::none)
    }

    pub(super) fn accept_search_service_recovery(
        &mut self,
        action: SearchServiceRecoveryAction,
        outcome: Result<file_search::SearchServiceStatus, SearchServiceDiagnostic>,
    ) -> Task<Message> {
        self.search_service
            .accept_recovery_completion(action, outcome);
        Task::none()
    }

    pub(super) fn refresh_search_service_status(&mut self) -> Task<Message> {
        self.search_service
            .request_status_refresh()
            .map(search_service_status_command)
            .unwrap_or_else(Task::none)
    }

    pub(super) fn accept_search_service_status(
        &mut self,
        request: SearchServiceStatusRequest,
        outcome: Result<file_search::SearchServiceStatus, SearchServiceDiagnostic>,
    ) -> Task<Message> {
        self.search_service.accept_status_request(request, outcome);
        Task::none()
    }

    pub(super) fn toggle_search_service_incident_details(
        &mut self,
        kind: SearchServiceDiagnosticKind,
    ) -> Task<Message> {
        self.search_service.toggle_incident_technical_detail(kind);
        Task::none()
    }

    pub(super) fn copy_search_service_incident_details(
        &self,
        kind: SearchServiceDiagnosticKind,
    ) -> Task<Message> {
        self.search_service
            .incident_technical_detail(kind)
            .map(iced::clipboard::write)
            .unwrap_or_else(Task::none)
    }

    pub(super) fn toggle_search_content_indexing(&mut self) -> Task<Message> {
        self.user_config.search_content_indexing_enabled =
            !self.user_config.search_content_indexing_enabled;
        self.persist_app_config_command()
    }

    fn switch_to_directory_fallback(&mut self, unavailable_message: String) -> Task<Message> {
        let Some(workspace) = self.search_workspace.as_ref() else {
            return Task::none();
        };
        let Some(query) = workspace.run.active_query.as_ref() else {
            self.fail_search_workspace(unavailable_message);
            return Task::none();
        };
        if !query.recursive {
            self.fail_search_workspace(unavailable_message);
            return Task::none();
        }
        let SearchScope::Directory(directory) = &query.scope else {
            self.fail_search_workspace(unavailable_message);
            return Task::none();
        };
        if query.cursor.is_some() || self.path_is_remote_mount(directory) {
            self.fail_search_workspace(unavailable_message);
            return Task::none();
        }

        let generation = workspace.run.generation;
        Task::perform(read_search_path_configuration(), move |outcome| {
            Message::SearchDirectoryFallbackConfigurationLoaded(
                generation,
                unavailable_message,
                outcome,
            )
        })
    }

    pub(super) fn start_verified_directory_fallback(
        &mut self,
        generation: u64,
        unavailable_message: String,
        outcome: Result<
            (
                file_search::VersionedSearchPathPreferences,
                file_search::SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ) -> Task<Message> {
        let (_, status) = match outcome {
            Ok(snapshot) => snapshot,
            Err(diagnostic) => {
                self.fail_search_workspace(format!(
                    "{unavailable_message}; could not verify search exclusions: {}",
                    diagnostic.technical_detail
                ));
                return Task::none();
            }
        };
        let rules = match SearchExcludeRules::for_directory_fallback(
            self.home_dir.clone(),
            status.effective_preferences.clone(),
        ) {
            Ok(rules) => rules,
            Err(message) => {
                self.fail_search_workspace(format!(
                    "{unavailable_message}; effective search path rules are invalid: {message}"
                ));
                return Task::none();
            }
        };
        self.update_search_path_configuration_status(status);

        let Some(workspace) = self.search_workspace.as_ref() else {
            return Task::none();
        };
        if workspace.run.generation != generation {
            return Task::none();
        }
        let Some(query) = workspace.run.active_query.clone() else {
            return Task::none();
        };
        let SearchScope::Directory(directory) = &query.scope else {
            return Task::none();
        };
        if !query.recursive || query.cursor.is_some() || self.path_is_remote_mount(directory) {
            return Task::none();
        }

        let Some(workspace) = self.search_workspace.as_mut() else {
            return Task::none();
        };
        let cancellation = workspace.begin_directory_fallback();
        directory_fallback_search_command(generation, query, rules, cancellation)
    }

    fn current_search_directory(&self) -> PathBuf {
        if self.view_mode == BrowserViewMode::Columns {
            if let Some(directory) = self.last_pointer_clicked_rendered_column_directory() {
                return directory;
            }
        }
        self.pane_by_id(self.active_pane_id())
            .map(|pane| pane.current_dir.clone())
            .unwrap_or_else(|| self.current_dir.clone())
    }
}

fn search_input_stabilization_command(request: SearchInputStabilizationRequest) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(SEARCH_INPUT_STABILIZATION_DELAY).await;
            request
        },
        Message::SearchInputStabilized,
    )
}

#[cfg(test)]
#[path = "search_operations_tests.rs"]
mod operation_tests;

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
