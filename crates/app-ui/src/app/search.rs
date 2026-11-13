use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;
use file_search::{
    SearchError, SearchExcludeRules, SearchHit, SearchMatchMode, SearchQuery, SearchScope,
    SearchTextScope,
};
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::commands::{
    directory_fallback_search_command, read_directory_fallback_preferences, search_command,
    search_service_recovery_command, search_service_status_command,
    search_with_scope_root_check_command,
};
use crate::model::search::{SearchRunId, SEARCH_RESULT_WINDOW};
use crate::model::{
    BrowserPaneId, BrowserViewMode, ContextMenuState, DirectoryFallbackOutcome,
    IndexedSearchOutcome, IndexedSearchRequest, Message, NavigationMode, SearchDateField,
    SearchDatePreset, SearchDirectoryScope, SearchEntryTypeMenuState, SearchEntryTypePreset,
    SearchInputStabilizationRequest, SearchInputStabilizationSubject, SearchServiceDiagnostic,
    SearchServiceDiagnosticKind, SearchServiceRecoveryAction, SearchServiceStatusRequest,
    SearchSizePreset, SearchWorkspaceSessionId, SearchWorkspaceState,
};

const SEARCH_INPUT_STABILIZATION_DELAY: Duration = Duration::from_millis(120);

impl FileBrowser {
    /// 活动标签上的搜索（标签在搜索结果里时）。
    pub(crate) fn active_search(&self) -> Option<&SearchWorkspaceState> {
        self.tab().location.search()
    }

    fn active_search_mut(&mut self) -> Option<&mut SearchWorkspaceState> {
        self.tab_mut().location.search_mut()
    }

    pub(super) fn update_search_input(&mut self, value: String) -> Task<Message> {
        self.edit_active_search(|search| search.replace_input(value))
    }

    pub(super) fn update_search_custom_extensions(&mut self, value: String) -> Task<Message> {
        self.edit_active_search(|search| search.replace_custom_extensions(value))
    }

    pub(super) fn update_search_custom_size_min(&mut self, value: String) -> Task<Message> {
        self.edit_active_search(|search| {
            let max = search.filters.custom_size_max.clone();
            search.replace_custom_size(value, max)
        })
    }

    pub(super) fn update_search_custom_size_max(&mut self, value: String) -> Task<Message> {
        self.edit_active_search(|search| {
            let min = search.filters.custom_size_min.clone();
            search.replace_custom_size(min, value)
        })
    }

    /// 打字类输入：不在搜索里就先进入搜索，改完等输入停下再搜。
    fn edit_active_search(
        &mut self,
        edit: impl FnOnce(&mut SearchWorkspaceState) -> SearchInputStabilizationRequest,
    ) -> Task<Message> {
        let entered = match self.ensure_active_search() {
            Ok(entered) => entered,
            Err(message) => {
                self.show_global_error(message);
                return Task::none();
            }
        };
        let Some(search) = self.active_search_mut() else {
            return entered;
        };
        let request = edit(search);
        Task::batch([entered, search_input_stabilization_command(request)])
    }

    pub(super) fn toggle_search_custom_extensions(&mut self) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.toggle_custom_extensions();
        self.restart_active_search()
    }

    pub(super) fn select_search_size_preset(
        &mut self,
        size_preset: SearchSizePreset,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        // 选任何档位（含进入/离开 Custom）都立即 restart；离开 Custom 由
        // select_size_preset 清空两侧输入。
        workspace.filters.select_size_preset(size_preset);
        self.restart_active_search()
    }

    pub(super) fn accept_search_input_stabilization(
        &mut self,
        request: SearchInputStabilizationRequest,
    ) -> Task<Message> {
        // 输入停下时标签可能已经不是活动的了：按会话号找回它。
        let session_id = request.session_id();
        let accepted = self
            .search_tab(session_id)
            .and_then(|tab| tab.location.search())
            .is_some_and(|search| match request.subject {
                SearchInputStabilizationSubject::Terms => {
                    search.accepts_input_stabilization(&request)
                }
                SearchInputStabilizationSubject::CustomExtensions => {
                    search.accepts_custom_extensions_stabilization(&request)
                }
                SearchInputStabilizationSubject::CustomSize => {
                    search.accepts_custom_size_stabilization(&request)
                }
            });
        if !accepted {
            return Task::none();
        }
        self.restart_search(session_id)
    }

    pub(super) fn submit_search_input(&mut self) -> Task<Message> {
        let entered = match self.ensure_active_search() {
            Ok(entered) => entered,
            Err(message) => {
                self.show_global_error(message);
                return Task::none();
            }
        };
        let keyword = self
            .active_search()
            .map(|search| search.input.clone())
            .unwrap_or_default();
        let persist = if self.user_config.search_history.record_submission(&keyword) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        };
        self.search_history_interaction.dismiss_popup();
        Task::batch([entered, persist, self.restart_active_search()])
    }

    pub(super) fn select_search_history_keyword(&mut self, keyword: String) -> Task<Message> {
        if !self.user_config.search_history.contains(&keyword) {
            return Task::none();
        }
        let entered = match self.ensure_active_search() {
            Ok(entered) => entered,
            Err(message) => {
                self.show_global_error(message);
                return Task::none();
            }
        };
        let Some(search) = self.active_search_mut() else {
            return entered;
        };
        search.replace_input_immediately(keyword.clone());
        self.search_history_interaction.reset();
        let persist = if self.user_config.search_history.record_submission(&keyword) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        };
        Task::batch([entered, persist, self.restart_active_search()])
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
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.input.clear();
        self.restart_active_search()
    }

    pub(super) fn open_search_entry_types_menu(&mut self) -> Task<Message> {
        if self.active_search().is_none()
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
                position: self.cursor_or_origin(),
            },
        ));
        Task::none()
    }

    pub(super) fn toggle_search_entry_type(
        &mut self,
        entry_type: SearchEntryTypePreset,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.toggle_entry_type(entry_type);
        self.restart_active_search()
    }

    pub(super) fn select_search_directory_scope(
        &mut self,
        scope: SearchDirectoryScope,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        if !workspace.root.select_scope(scope) {
            return Task::none();
        }
        self.restart_active_search()
    }

    /// 把这次搜索的选定范围写入持久化范围记忆；仅在真实变化时保存。
    /// 只在 restart_search 真实构造查询时调用。
    fn remember_last_search_scope(
        &mut self,
        scope: crate::model::LastSearchScope,
    ) -> Task<Message> {
        if self.user_config.last_search_scope.as_ref() == Some(&scope) {
            return Task::none();
        }
        self.user_config.last_search_scope = Some(scope);
        self.persist_user_preferences_command()
    }

    pub(super) fn select_search_text_scope(
        &mut self,
        text_scope: SearchTextScope,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.text_scope = text_scope;
        self.restart_active_search()
    }

    pub(super) fn toggle_search_regex(&mut self) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.match_mode = match workspace.filters.match_mode {
            SearchMatchMode::Plain => SearchMatchMode::Regex,
            SearchMatchMode::Regex => SearchMatchMode::Plain,
        };
        self.restart_active_search()
    }

    pub(super) fn select_search_date_field(
        &mut self,
        date_field: SearchDateField,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.date_field = date_field;
        self.restart_active_search()
    }

    pub(super) fn select_search_date_preset(
        &mut self,
        date_preset: SearchDatePreset,
    ) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.date_preset = date_preset;
        self.restart_active_search()
    }

    pub(super) fn reset_search_filters(&mut self) -> Task<Message> {
        let Some(workspace) = self.active_search_mut() else {
            return Task::none();
        };
        workspace.filters.reset();
        self.restart_active_search()
    }

    /// Esc 或关闭按钮：回到发起搜索的文件夹，这次搜索记进历史。
    pub(super) fn close_search_workspace(&mut self) -> Task<Message> {
        let Some(origin) = self
            .active_search()
            .map(|search| search.origin().to_path_buf())
        else {
            return Task::none();
        };
        self.navigate_to(origin, NavigationMode::RecordHistory)
    }

    /// 标签离开搜索（导航、关闭）后搜索专属的浮层与记忆跟着收掉。
    pub(super) fn forget_search_interaction(&mut self) {
        self.search_history_interaction.reset();
        if matches!(
            self.context_menu,
            Some(ContextMenuState::SearchEntryTypes(_))
        ) {
            self.context_menu = None;
        }
    }

    /// 活动标签还不在搜索里时，从它的文件夹进入一次新搜索。
    fn ensure_active_search(&mut self) -> Result<Task<Message>, String> {
        if self.active_search().is_some() {
            return Ok(Task::none());
        }
        let Some(root) = self.current_search_directory() else {
            return Err(tr!("Search is available from a local folder"));
        };
        if self.path_is_remote_mount(&root) {
            return Err(tr!("Search is unavailable for remote folders"));
        }
        let state = SearchWorkspaceState::new(
            root,
            self.home_dir.clone(),
            self.user_config.last_search_scope.clone(),
            self.next_search_session_id(),
        );
        self.clear_pointer_driven_interaction_state();
        Ok(self.enter_search_location(state, NavigationMode::RecordHistory))
    }

    pub(super) fn next_search_session_id(&mut self) -> SearchWorkspaceSessionId {
        let session_id = SearchWorkspaceSessionId(self.next_search_workspace_session_id);
        self.next_search_workspace_session_id =
            self.next_search_workspace_session_id.wrapping_add(1);
        session_id
    }

    /// 刷新、回车、筛选变化：活动标签的这次搜索按当前意图重搜。
    pub(super) fn restart_active_search(&mut self) -> Task<Message> {
        match self.active_search().map(SearchWorkspaceState::session_id) {
            Some(session_id) => self.restart_search(session_id),
            None => Task::none(),
        }
    }

    /// 真实查询在这里唯一发起：回车、历史点击、范围切换、稳定输入、刷新、筛选与顺序
    /// 变化都经过这里。空输入与被拒绝的查询不会走到发起这一步，也不改写范围记忆。
    pub(super) fn restart_search(&mut self, session_id: SearchWorkspaceSessionId) -> Task<Message> {
        let Some(tab) = self.search_tab_mut(session_id) else {
            return Task::none();
        };
        let Some(search) = tab.location.search_mut() else {
            return Task::none();
        };
        search.invalidate_input_stabilization();
        if search.input.trim().is_empty()
            && !search.filters.custom_extensions_are_active()
            && !search.filters.custom_size_has_intent()
        {
            tab.clear_search_query();
            return self.search_results_changed(session_id);
        }
        search.drop_order_of_previous_search();
        let generation = search.next_generation();
        let scope = search.root.query_scope();
        let remembered_scope = search.root.last_search_scope();
        let terms = search.input.trim().to_owned();
        let match_mode = search.filters.match_mode;
        let filters = match search.filters.query_filters_at(Local::now()) {
            Ok(filters) => filters,
            Err(message) => {
                tab.reject_search_query(message);
                return self.search_results_changed(session_id);
            }
        };
        if let SearchMatchMode::Regex = match_mode {
            if let Err(SearchError::InvalidQuery(message)) = match_mode.name_regex(&terms) {
                tab.reject_search_query(tr!("Invalid regular expression: %1", message));
                return self.search_results_changed(session_id);
            }
        }
        let query = SearchQuery {
            query_id: generation,
            terms,
            // 正则只在名称上执行，内容范围对正则语义不可用。
            text_scope: if match_mode == SearchMatchMode::Regex {
                SearchTextScope::NameOnly
            } else {
                search.filters.text_scope
            },
            match_mode,
            scope,
            recursive: true,
            filters,
            order: search.order(),
            limit: SEARCH_RESULT_WINDOW,
            cursor: None,
        };
        let Some((request, cancellation)) = tab.begin_search_query(query.clone()) else {
            return Task::none();
        };
        Task::batch([
            self.remember_last_search_scope(remembered_scope),
            search_with_scope_root_check_command(request, query, cancellation),
            self.search_results_changed(session_id),
        ])
    }

    pub(super) fn accept_search_scope_root_validation(
        &mut self,
        request: IndexedSearchRequest,
        query: SearchQuery,
        cancellation: CancellationToken,
        root_is_available: bool,
    ) -> Task<Message> {
        let session_id = request.run.session_id;
        let Some(tab) = self.search_tab_mut(session_id) else {
            return Task::none();
        };
        if !tab
            .location
            .search()
            .is_some_and(|search| search.accepts_indexed_outcome(request))
        {
            // 过期校验结果：期间查询已重启，直接丢弃。
            return Task::none();
        }
        if !root_is_available {
            let message = match &query.scope {
                SearchScope::Directory(root) => {
                    tr!("Search root is unavailable: %1", root.display())
                }
                _ => tr!("Search root is unavailable"),
            };
            tab.reject_search_query(message);
            return self.search_results_changed(session_id);
        }
        search_command(request, query, cancellation)
    }

    pub(super) fn accept_search_results(
        &mut self,
        request: IndexedSearchRequest,
        outcome: IndexedSearchOutcome,
    ) -> Task<Message> {
        let session_id = request.run.session_id;
        let Some(tab) = self.search_tab_mut(session_id) else {
            return Task::none();
        };
        if !tab
            .location
            .search()
            .is_some_and(|search| search.accepts_indexed_outcome(request))
        {
            return Task::none();
        }
        match outcome {
            IndexedSearchOutcome::Cancelled => {
                if let Some(search) = tab.location.search_mut() {
                    search.apply_indexed_cancellation(request);
                }
            }
            IndexedSearchOutcome::Batch(batch) => {
                tab.accept_search_batch(request, batch);
            }
            IndexedSearchOutcome::TransportUnavailable(message) => {
                self.search_service
                    .observe_query_transport_failure(&message);
                if request.cursor.is_none() {
                    return self.switch_to_directory_fallback(session_id, message);
                }
                if let Some(tab) = self.search_tab_mut(session_id) {
                    tab.accept_search_failure(request, message);
                }
            }
            IndexedSearchOutcome::ProviderUnavailable(message) => {
                if request.cursor.is_none() {
                    return self.switch_to_directory_fallback(session_id, message);
                }
                tab.accept_search_failure(request, message);
            }
            IndexedSearchOutcome::InvalidQuery(message) | IndexedSearchOutcome::Fatal(message) => {
                tab.accept_search_failure(request, message);
            }
            // 接着翻会重复或漏条目，所以不报错，直接从头重新搜，列表回到顶部。
            IndexedSearchOutcome::SnapshotExpired => return self.restart_search(session_id),
        }
        self.search_results_changed(session_id)
    }

    /// 窗格滚到结果末尾附近时取下一页。
    pub(super) fn load_more_search_results(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return Task::none();
        };
        let Some((request, query, cancellation)) = pane.tab_mut().begin_next_search_page() else {
            return Task::none();
        };
        search_command(request, query, cancellation)
    }

    pub(super) fn accept_directory_search_batch(
        &mut self,
        run: SearchRunId,
        hits: Vec<SearchHit>,
    ) -> Task<Message> {
        let Some(tab) = self.search_tab_mut(run.session_id) else {
            return Task::none();
        };
        tab.accept_fallback_hits(run, hits);
        self.search_results_changed(run.session_id)
    }

    pub(super) fn accept_directory_search_finished(
        &mut self,
        run: SearchRunId,
        completion: DirectoryFallbackOutcome,
    ) -> Task<Message> {
        if let Some(tab) = self.search_tab_mut(run.session_id) {
            tab.finish_fallback_search(run, completion);
        }
        Task::none()
    }

    /// 打开结果所在的文件夹并选中它；这次搜索记进历史。
    pub(super) fn open_search_containing_directory(&mut self, path: PathBuf) -> Task<Message> {
        self.context_menu = None;
        let Some(parent) = path.parent().map(std::path::Path::to_path_buf) else {
            self.fail_active_search(tr!("The result has no containing directory"));
            return Task::none();
        };
        if !parent.is_dir() {
            self.fail_active_search(tr!(
                "Containing directory is unavailable: %1",
                parent.display()
            ));
            return Task::none();
        }
        let navigation = self.navigate_to(parent, NavigationMode::RecordHistory);
        self.select_path(path);
        Task::batch([navigation, self.request_browser_session_save()])
    }

    fn fail_active_search(&mut self, message: String) {
        if let Some(search) = self.active_search_mut() {
            search.window.failure = Some(message);
            search.window.is_loading = false;
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

    fn switch_to_directory_fallback(
        &mut self,
        session_id: SearchWorkspaceSessionId,
        unavailable_message: String,
    ) -> Task<Message> {
        let Some(search) = self
            .search_tab(session_id)
            .and_then(|tab| tab.location.search())
        else {
            return Task::none();
        };
        let fallback_root = search.run.active_query.as_ref().and_then(|query| {
            let SearchScope::Directory(directory) = &query.scope else {
                return None;
            };
            (query.recursive && query.cursor.is_none()).then(|| directory.clone())
        });
        let run = search.run_id();
        let Some(_) = fallback_root.filter(|root| !self.path_is_remote_mount(root)) else {
            self.fail_search(session_id, unavailable_message);
            return Task::none();
        };
        Task::perform(
            read_directory_fallback_preferences(file_core::app_dirs::user_config_dir()),
            move |outcome| {
                Message::SearchDirectoryFallbackConfigurationLoaded(
                    run,
                    unavailable_message,
                    outcome,
                )
            },
        )
    }

    pub(super) fn start_verified_directory_fallback(
        &mut self,
        run: SearchRunId,
        unavailable_message: String,
        outcome: Result<file_search::SearchPathPreferences, String>,
    ) -> Task<Message> {
        let preferences = match outcome {
            Ok(preferences) => preferences,
            Err(error) => {
                self.fail_search(
                    run.session_id,
                    format!("{unavailable_message}; could not read search exclusions: {error}"),
                );
                return Task::none();
            }
        };
        let rules =
            match SearchExcludeRules::for_directory_fallback(self.home_dir.clone(), preferences) {
                Ok(rules) => rules,
                Err(message) => {
                    self.fail_search(
                        run.session_id,
                        format!("{unavailable_message}; search path rules are invalid: {message}"),
                    );
                    return Task::none();
                }
            };

        let Some(query) = self
            .search_tab(run.session_id)
            .and_then(|tab| tab.location.search())
            .filter(|search| search.run_id() == run)
            .and_then(|search| search.run.active_query.clone())
        else {
            return Task::none();
        };
        let SearchScope::Directory(directory) = &query.scope else {
            return Task::none();
        };
        if !query.recursive || query.cursor.is_some() || self.path_is_remote_mount(directory) {
            return Task::none();
        }
        let Some(search) = self
            .search_tab_mut(run.session_id)
            .and_then(|tab| tab.location.search_mut())
        else {
            return Task::none();
        };
        let cancellation = search.begin_directory_fallback();
        Task::batch([
            directory_fallback_search_command(run, query, rules, cancellation),
            self.search_results_changed(run.session_id),
        ])
    }

    fn fail_search(&mut self, session_id: SearchWorkspaceSessionId, message: String) {
        if let Some(search) = self
            .search_tab_mut(session_id)
            .and_then(|tab| tab.location.search_mut())
        {
            search.window.failure = Some(message);
            search.window.is_loading = false;
        }
    }

    /// 搜索的起点目录；回收站不是目录，不能从那里搜。
    fn current_search_directory(&self) -> Option<PathBuf> {
        let current_directory = self.tab().current_directory()?;
        if self.tab().view_mode() == BrowserViewMode::Columns {
            if let Some(directory) = self.last_pointer_clicked_rendered_column_directory() {
                return Some(directory);
            }
        }
        Some(current_directory.to_path_buf())
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
