use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_search::{
    SearchCursor, SearchHit, SearchQuery, SearchResultBatch, SearchScope, SearchTextScope,
};
use tokio_util::sync::CancellationToken;

mod filters;
pub(crate) use filters::{
    SearchDateField, SearchDatePreset, SearchEntryTypePreset, SearchFilterPresetState,
    SearchSizePreset,
};
mod history;
pub(crate) use history::SearchHistory;

pub(crate) const SEARCH_RESULT_WINDOW: usize = 100;
/// 结果总量硬上界：无论翻页多少页，窗口最多保留这么多条，超限丢弃最旧页，
/// 保证长会话滚动加载不会让内存无限增长。
pub(crate) const SEARCH_RESULT_TOTAL_LIMIT: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchWorkspaceSessionId(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchInputStabilizationSubject {
    Terms,
    CustomExtensions,
    CustomSize,
}

/// 稳定化请求携带的原始输入：文本类 subject 用 `Text`，
/// 自定义大小两侧输入是同一概念的两个投影，合并为一个值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchStabilizationInput {
    Text(String),
    SizeRange { min: String, max: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchInputStabilizationRequest {
    workspace_session_id: SearchWorkspaceSessionId,
    input_revision: u64,
    pub(crate) input: SearchStabilizationInput,
    pub(crate) subject: SearchInputStabilizationSubject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchInputFocus {
    Focused,
    Unfocused,
}

impl From<bool> for SearchInputFocus {
    fn from(is_focused: bool) -> Self {
        if is_focused {
            Self::Focused
        } else {
            Self::Unfocused
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchInputFocusCheckOrigin {
    Pointer,
    KeyboardTraversal,
    MainWindowFocused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchInputFocusCheckRequest {
    generation: u64,
    origin: SearchInputFocusCheckOrigin,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SearchHistoryInteraction {
    focus_check_generation: u64,
    input_is_focused: bool,
    pointer_is_over_popup: bool,
    popup_is_dismissed: bool,
}

impl SearchHistoryInteraction {
    pub(crate) fn begin_input_focus_check(
        &mut self,
        origin: SearchInputFocusCheckOrigin,
    ) -> SearchInputFocusCheckRequest {
        self.focus_check_generation = self.focus_check_generation.wrapping_add(1);
        SearchInputFocusCheckRequest {
            generation: self.focus_check_generation,
            origin,
        }
    }

    pub(crate) fn accept_input_focus_check(
        &mut self,
        request: SearchInputFocusCheckRequest,
        focus: SearchInputFocus,
    ) {
        if request.generation == self.focus_check_generation {
            self.input_is_focused = focus == SearchInputFocus::Focused;
            if !matches!(
                (request.origin, focus),
                (
                    SearchInputFocusCheckOrigin::Pointer,
                    SearchInputFocus::Unfocused
                )
            ) {
                self.popup_is_dismissed = false;
            }
        }
    }

    pub(crate) fn enter_popup(&mut self) {
        self.pointer_is_over_popup = true;
    }

    pub(crate) fn pointer_is_over_popup(&self) -> bool {
        self.pointer_is_over_popup
    }

    pub(crate) fn exit_popup(&mut self) {
        self.pointer_is_over_popup = false;
    }

    pub(crate) fn popup_is_visible(&self, history: &SearchHistory) -> bool {
        !self.popup_is_dismissed
            && !history.entries().is_empty()
            && (self.input_is_focused || self.pointer_is_over_popup)
    }

    pub(crate) fn dismiss_popup(&mut self) {
        self.focus_check_generation = self.focus_check_generation.wrapping_add(1);
        self.pointer_is_over_popup = false;
        self.popup_is_dismissed = true;
    }

    pub(crate) fn reset(&mut self) {
        self.focus_check_generation = self.focus_check_generation.wrapping_add(1);
        self.input_is_focused = false;
        self.pointer_is_over_popup = false;
        self.popup_is_dismissed = false;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IndexedSearchOutcome {
    Batch(SearchResultBatch),
    Cancelled,
    TransportUnavailable(String),
    ProviderUnavailable(String),
    InvalidQuery(String),
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryFallbackOutcome {
    Completed(file_search::DirectoryFallbackCompletion),
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchProvider {
    Indexed,
    DirectoryFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchResultCompletion {
    Complete,
    MoreAvailable,
    Partial { inspected_entries: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchDirectoryScope {
    CurrentFolder,
    LastLocation,
    Home,
    AllIndexedLocations,
}

impl SearchDirectoryScope {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::CurrentFolder => "Current folder",
            Self::LastLocation => "Last location",
            Self::Home => "Home",
            Self::AllIndexedLocations => "All indexed locations",
        }
    }
}

/// 最近一次搜索实际使用的范围，随用户偏好持久化；打开下一个搜索工作区时
/// 作为默认范围恢复（访达"使用上一次的搜索范围"语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LastSearchScope {
    Global,
    Directory(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct SearchRootSnapshot {
    current_folder: PathBuf,
    home: PathBuf,
    last_location: Option<LastSearchScope>,
    selected_scope: SearchDirectoryScope,
}

impl SearchRootSnapshot {
    pub(crate) fn new(
        current_folder: PathBuf,
        home: PathBuf,
        last_location: Option<LastSearchScope>,
    ) -> Self {
        // 默认档恢复上次范围；无记录时沿用"在 Home 默认 Home，否则当前文件夹"。
        // 目录记录若与当前文件夹或 Home 相同，直接归并到对应现有档，不产生重复选项。
        let selected_scope = match &last_location {
            Some(LastSearchScope::Global) => SearchDirectoryScope::AllIndexedLocations,
            Some(LastSearchScope::Directory(directory))
                if directory.as_path() == current_folder.as_path() =>
            {
                SearchDirectoryScope::CurrentFolder
            }
            Some(LastSearchScope::Directory(directory)) if directory.as_path() == home.as_path() => {
                SearchDirectoryScope::Home
            }
            Some(_) => SearchDirectoryScope::LastLocation,
            None if current_folder == home => SearchDirectoryScope::Home,
            None => SearchDirectoryScope::CurrentFolder,
        };
        Self {
            current_folder,
            home,
            last_location,
            selected_scope,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => &self.current_folder,
            SearchDirectoryScope::Home | SearchDirectoryScope::AllIndexedLocations => &self.home,
            // 不变量：selected_scope == LastLocation 当且仅当 last_location 是
            // 一个不等于 current_folder/home 的目录记录。
            SearchDirectoryScope::LastLocation => self.last_location_directory().unwrap_or(&self.home),
        }
    }

    pub(crate) fn query_scope(&self) -> SearchScope {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => {
                SearchScope::Directory(self.current_folder.clone())
            }
            SearchDirectoryScope::Home => SearchScope::Directory(self.home.clone()),
            SearchDirectoryScope::AllIndexedLocations => SearchScope::Global,
            SearchDirectoryScope::LastLocation => match &self.last_location {
                Some(LastSearchScope::Directory(directory)) => {
                    SearchScope::Directory(directory.clone())
                }
                // 同 path()：LastLocation 档只在有目录记录时可用。
                _ => SearchScope::Global,
            },
        }
    }

    /// LastLocation 档指向的目录；仅在持有目录记录时存在。
    pub(crate) fn last_location_directory(&self) -> Option<&Path> {
        match &self.last_location {
            Some(LastSearchScope::Directory(directory)) => Some(directory.as_path()),
            _ => None,
        }
    }

    /// 当前选定范围的持久化事实，用于随用户偏好落盘。
    pub(crate) fn last_search_scope(&self) -> LastSearchScope {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => {
                LastSearchScope::Directory(self.current_folder.clone())
            }
            SearchDirectoryScope::Home => LastSearchScope::Directory(self.home.clone()),
            SearchDirectoryScope::AllIndexedLocations => LastSearchScope::Global,
            SearchDirectoryScope::LastLocation => match &self.last_location {
                Some(directory @ LastSearchScope::Directory(_)) => directory.clone(),
                _ => LastSearchScope::Global,
            },
        }
    }

    pub(crate) fn selected_scope(&self) -> SearchDirectoryScope {
        self.selected_scope
    }

    pub(crate) fn available_scopes(&self) -> Vec<SearchDirectoryScope> {
        let mut scopes = if self.current_folder == self.home {
            vec![
                SearchDirectoryScope::Home,
                SearchDirectoryScope::AllIndexedLocations,
            ]
        } else {
            vec![
                SearchDirectoryScope::CurrentFolder,
                SearchDirectoryScope::Home,
                SearchDirectoryScope::AllIndexedLocations,
            ]
        };
        // 上次位置只在"不与现有档重复"时出现：等于当前文件夹并入 CurrentFolder，
        // 等于 Home 并入 Home，全局并入 AllIndexedLocations。
        let last_directory = self.last_location_directory();
        if let Some(directory) = last_directory {
            if directory != self.current_folder && directory != self.home {
                let insert_at = if self.current_folder == self.home { 0 } else { 1 };
                scopes.insert(insert_at, SearchDirectoryScope::LastLocation);
            }
        }
        scopes
    }

    pub(crate) fn select_scope(&mut self, scope: SearchDirectoryScope) -> bool {
        if self.selected_scope == scope || !self.available_scopes().contains(&scope) {
            return false;
        }
        self.selected_scope = scope;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexedSearchRequest {
    pub(crate) generation: u64,
    pub(crate) cursor: Option<SearchCursor>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchRunState {
    pub(crate) generation: u64,
    pub(crate) active_query: Option<SearchQuery>,
    pub(crate) provider: Option<SearchProvider>,
    pub(crate) next_cursor: Option<SearchCursor>,
    pub(crate) pending_indexed_request: Option<IndexedSearchRequest>,
    cancellation: Option<CancellationToken>,
}

impl SearchRunState {
    fn new() -> Self {
        Self {
            generation: 0,
            active_query: None,
            provider: None,
            next_cursor: None,
            pending_indexed_request: None,
            cancellation: None,
        }
    }

    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResultWindow {
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) is_loading: bool,
    pub(crate) failure: Option<String>,
    pub(crate) completion: Option<SearchResultCompletion>,
    pub(crate) truncated: bool,
    pub(crate) viewport_offset_y: f32,
    pub(crate) viewport_height: f32,
}

impl SearchResultWindow {
    fn new() -> Self {
        Self {
            hits: Vec::new(),
            is_loading: false,
            failure: None,
            completion: None,
            truncated: false,
            viewport_offset_y: 0.0,
            viewport_height: 0.0,
        }
    }

    fn begin_loading(&mut self) {
        self.hits.clear();
        self.is_loading = true;
        self.failure = None;
        self.completion = None;
        self.truncated = false;
        self.viewport_offset_y = 0.0;
        self.viewport_height = 0.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSelectionGesture {
    Plain,
    Toggle,
    Range,
    AdditiveRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSelectionStep {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchKeyboardSelection {
    Replace,
    Extend,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResultSelection {
    selected_paths: HashSet<PathBuf>,
    anchor: Option<PathBuf>,
    focused: Option<PathBuf>,
}

impl SearchResultSelection {
    fn new() -> Self {
        Self {
            selected_paths: HashSet::new(),
            anchor: None,
            focused: None,
        }
    }

    pub(crate) fn is_selected(&self, path: &Path) -> bool {
        self.selected_paths.contains(path)
    }

    pub(crate) fn focused_path(&self) -> Option<&Path> {
        self.focused.as_deref()
    }

    pub(crate) fn selected_paths_in_result_order(&self, hits: &[SearchHit]) -> Vec<PathBuf> {
        hits.iter()
            .filter(|hit| self.selected_paths.contains(&hit.path))
            .map(|hit| hit.path.clone())
            .collect()
    }

    pub(crate) fn select(
        &mut self,
        hits: &[SearchHit],
        target: &Path,
        gesture: SearchSelectionGesture,
    ) {
        let Some(target_index) = hits.iter().position(|hit| hit.path == target) else {
            return;
        };
        match gesture {
            SearchSelectionGesture::Plain => {
                self.selected_paths.clear();
                self.selected_paths.insert(target.to_path_buf());
                self.anchor = Some(target.to_path_buf());
            }
            SearchSelectionGesture::Toggle => {
                if !self.selected_paths.remove(target) {
                    self.selected_paths.insert(target.to_path_buf());
                }
                self.anchor = Some(target.to_path_buf());
            }
            SearchSelectionGesture::Range | SearchSelectionGesture::AdditiveRange => {
                let anchor = self
                    .anchor
                    .as_deref()
                    .and_then(|path| hits.iter().position(|hit| hit.path == path))
                    .unwrap_or(target_index);
                if gesture == SearchSelectionGesture::Range {
                    self.selected_paths.clear();
                }
                let start = anchor.min(target_index);
                let end = anchor.max(target_index);
                self.selected_paths
                    .extend(hits[start..=end].iter().map(|hit| hit.path.clone()));
                self.anchor = Some(hits[anchor].path.clone());
            }
        }
        self.focused = Some(target.to_path_buf());
    }

    pub(crate) fn move_focus(
        &mut self,
        hits: &[SearchHit],
        step: SearchSelectionStep,
        keyboard_selection: SearchKeyboardSelection,
    ) -> Option<PathBuf> {
        if hits.is_empty() {
            return None;
        }
        let current_index = self
            .focused
            .as_deref()
            .and_then(|path| hits.iter().position(|hit| hit.path == path));
        let target_index = match (current_index, step) {
            (Some(index), SearchSelectionStep::Previous) => index.saturating_sub(1),
            (Some(index), SearchSelectionStep::Next) => (index + 1).min(hits.len() - 1),
            (None, SearchSelectionStep::Previous) => hits.len() - 1,
            (None, SearchSelectionStep::Next) => 0,
        };
        let target = hits[target_index].path.clone();
        let gesture = match keyboard_selection {
            SearchKeyboardSelection::Replace => SearchSelectionGesture::Plain,
            SearchKeyboardSelection::Extend => SearchSelectionGesture::Range,
        };
        self.select(hits, &target, gesture);
        Some(target)
    }

    pub(crate) fn select_all(&mut self, hits: &[SearchHit]) {
        self.selected_paths = hits.iter().map(|hit| hit.path.clone()).collect();
        self.anchor = hits.first().map(|hit| hit.path.clone());
        self.focused = hits.first().map(|hit| hit.path.clone());
    }

    fn clear(&mut self) {
        self.selected_paths.clear();
        self.anchor = None;
        self.focused = None;
    }

    fn reconcile(&mut self, hits: &[SearchHit]) {
        let visible_paths = hits
            .iter()
            .map(|hit| hit.path.clone())
            .collect::<HashSet<_>>();
        self.selected_paths
            .retain(|path| visible_paths.contains(path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path))
        {
            self.anchor = None;
        }
        if self
            .focused
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path))
        {
            self.focused = hits
                .iter()
                .find(|hit| self.selected_paths.contains(&hit.path))
                .map(|hit| hit.path.clone());
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchWorkspaceState {
    session_id: SearchWorkspaceSessionId,
    input_revision: u64,
    custom_extensions_revision: u64,
    custom_size_revision: u64,
    pub(crate) root: SearchRootSnapshot,
    pub(crate) input: String,
    pub(crate) filters: SearchFilterPresetState,
    pub(crate) run: SearchRunState,
    pub(crate) window: SearchResultWindow,
    pub(crate) selection: SearchResultSelection,
}

impl SearchWorkspaceState {
    pub(crate) fn new(
        current_folder: PathBuf,
        home: PathBuf,
        last_location: Option<LastSearchScope>,
        session_id: SearchWorkspaceSessionId,
    ) -> Self {
        Self {
            session_id,
            input_revision: 0,
            custom_extensions_revision: 0,
            custom_size_revision: 0,
            root: SearchRootSnapshot::new(current_folder, home, last_location),
            input: String::new(),
            filters: SearchFilterPresetState::default(),
            run: SearchRunState::new(),
            window: SearchResultWindow::new(),
            selection: SearchResultSelection::new(),
        }
    }

    pub(crate) fn next_generation(&self) -> u64 {
        self.run.generation.saturating_add(1)
    }

    pub(crate) fn replace_input(&mut self, input: String) -> SearchInputStabilizationRequest {
        self.input = input;
        self.input_revision = self.input_revision.wrapping_add(1);
        SearchInputStabilizationRequest {
            workspace_session_id: self.session_id,
            input_revision: self.input_revision,
            input: SearchStabilizationInput::Text(self.input.clone()),
            subject: SearchInputStabilizationSubject::Terms,
        }
    }

    pub(crate) fn replace_input_immediately(&mut self, input: String) {
        self.input = input;
        self.invalidate_input_stabilization();
    }

    pub(crate) fn accepts_input_stabilization(
        &self,
        request: &SearchInputStabilizationRequest,
    ) -> bool {
        request.subject == SearchInputStabilizationSubject::Terms
            && self.session_id == request.workspace_session_id
            && self.input_revision == request.input_revision
            && request.input == SearchStabilizationInput::Text(self.input.clone())
    }

    pub(crate) fn replace_custom_extensions(
        &mut self,
        input: String,
    ) -> SearchInputStabilizationRequest {
        self.filters.custom_extensions = input;
        self.custom_extensions_revision = self.custom_extensions_revision.wrapping_add(1);
        SearchInputStabilizationRequest {
            workspace_session_id: self.session_id,
            input_revision: self.custom_extensions_revision,
            input: SearchStabilizationInput::Text(self.filters.custom_extensions.clone()),
            subject: SearchInputStabilizationSubject::CustomExtensions,
        }
    }

    pub(crate) fn accepts_custom_extensions_stabilization(
        &self,
        request: &SearchInputStabilizationRequest,
    ) -> bool {
        request.subject == SearchInputStabilizationSubject::CustomExtensions
            && self.session_id == request.workspace_session_id
            && self.custom_extensions_revision == request.input_revision
            && request.input
                == SearchStabilizationInput::Text(self.filters.custom_extensions.clone())
    }

    /// 自定义大小两侧输入共用一个 revision：任一侧变化都推进它，
    /// 使另一侧的 pending 请求作废，避免两次重查。
    pub(crate) fn replace_custom_size(
        &mut self,
        min: String,
        max: String,
    ) -> SearchInputStabilizationRequest {
        self.filters.custom_size_min = min;
        self.filters.custom_size_max = max;
        self.custom_size_revision = self.custom_size_revision.wrapping_add(1);
        SearchInputStabilizationRequest {
            workspace_session_id: self.session_id,
            input_revision: self.custom_size_revision,
            input: SearchStabilizationInput::SizeRange {
                min: self.filters.custom_size_min.clone(),
                max: self.filters.custom_size_max.clone(),
            },
            subject: SearchInputStabilizationSubject::CustomSize,
        }
    }

    pub(crate) fn accepts_custom_size_stabilization(
        &self,
        request: &SearchInputStabilizationRequest,
    ) -> bool {
        request.subject == SearchInputStabilizationSubject::CustomSize
            && self.session_id == request.workspace_session_id
            && self.custom_size_revision == request.input_revision
            && request.input
                == SearchStabilizationInput::SizeRange {
                    min: self.filters.custom_size_min.clone(),
                    max: self.filters.custom_size_max.clone(),
                }
    }

    pub(crate) fn invalidate_input_stabilization(&mut self) {
        self.input_revision = self.input_revision.wrapping_add(1);
        self.custom_extensions_revision = self.custom_extensions_revision.wrapping_add(1);
        self.custom_size_revision = self.custom_size_revision.wrapping_add(1);
    }

    pub(crate) fn begin_indexed_query(
        &mut self,
        query: SearchQuery,
    ) -> (IndexedSearchRequest, CancellationToken) {
        self.run.cancel();
        let cancellation = CancellationToken::new();
        let request = IndexedSearchRequest {
            generation: query.query_id,
            cursor: None,
        };
        self.run.cancellation = Some(cancellation.clone());
        self.run.generation = query.query_id;
        self.run.active_query = Some(query);
        self.run.provider = Some(SearchProvider::Indexed);
        self.run.next_cursor = None;
        self.run.pending_indexed_request = Some(request);
        self.window.begin_loading();
        self.selection.clear();
        (request, cancellation)
    }

    pub(crate) fn begin_next_indexed_page(
        &mut self,
    ) -> Option<(IndexedSearchRequest, SearchQuery, CancellationToken)> {
        if self.run.provider != Some(SearchProvider::Indexed)
            || self.run.pending_indexed_request.is_some()
        {
            return None;
        }
        let cursor = self.run.next_cursor?;
        let mut query = self.run.active_query.clone()?;
        query.cursor = Some(cursor);
        let request = IndexedSearchRequest {
            generation: self.run.generation,
            cursor: Some(cursor),
        };
        let cancellation = CancellationToken::new();
        self.run.cancellation = Some(cancellation.clone());
        self.run.pending_indexed_request = Some(request);
        self.window.is_loading = true;
        self.window.failure = None;
        Some((request, query, cancellation))
    }

    pub(crate) fn accepts_indexed_outcome(&self, request: IndexedSearchRequest) -> bool {
        request.generation == self.run.generation
            && self.run.provider == Some(SearchProvider::Indexed)
            && self.run.pending_indexed_request == Some(request)
    }

    pub(crate) fn apply_indexed_batch(
        &mut self,
        request: IndexedSearchRequest,
        mut batch: SearchResultBatch,
    ) {
        self.run.cancellation = None;
        self.run.pending_indexed_request = None;
        batch.hits.truncate(SEARCH_RESULT_WINDOW);
        if request.cursor.is_none() {
            self.window.hits = batch.hits;
            self.window.truncated = false;
        } else {
            self.window.hits.extend(batch.hits);
        }
        self.enforce_result_total_limit();
        self.run.next_cursor = batch.next_cursor;
        self.window.is_loading = false;
        self.window.failure = None;
        self.window.completion = Some(if batch.finished {
            SearchResultCompletion::Complete
        } else {
            SearchResultCompletion::MoreAvailable
        });
        self.selection.reconcile(&self.window.hits);
    }

    pub(crate) fn apply_indexed_failure(&mut self, request: IndexedSearchRequest, message: String) {
        self.run.cancellation = None;
        self.run.pending_indexed_request = None;
        if request.cursor.is_none() {
            self.window.hits.clear();
            self.selection.clear();
        }
        self.window.is_loading = false;
        self.window.failure = Some(message);
    }

    pub(crate) fn apply_indexed_cancellation(&mut self, request: IndexedSearchRequest) {
        self.run.cancellation = None;
        self.run.pending_indexed_request = None;
        self.window.is_loading = false;
        self.window.failure = None;
        if request.cursor.is_none() {
            self.run.next_cursor = None;
        }
    }

    pub(crate) fn update_viewport(&mut self, offset_y: f32, height: f32) {
        self.window.viewport_offset_y = offset_y.max(0.0);
        self.window.viewport_height = height.max(0.0);
    }

    pub(crate) fn indexed_next_page_is_available(&self) -> bool {
        self.run.provider == Some(SearchProvider::Indexed)
            && self.window.completion == Some(SearchResultCompletion::MoreAvailable)
            && self.run.next_cursor.is_some()
            && self.run.pending_indexed_request.is_none()
            && self.window.hits.len() < SEARCH_RESULT_TOTAL_LIMIT
    }

    pub(crate) fn begin_directory_fallback(&mut self) -> CancellationToken {
        self.run.cancel();
        let cancellation = CancellationToken::new();
        self.run.cancellation = Some(cancellation.clone());
        self.run.provider = Some(SearchProvider::DirectoryFallback);
        self.run.next_cursor = None;
        self.run.pending_indexed_request = None;
        self.window.begin_loading();
        self.selection.clear();
        cancellation
    }

    pub(crate) fn accepts_directory_fallback(&self, generation: u64) -> bool {
        generation == self.run.generation
            && self.run.provider == Some(SearchProvider::DirectoryFallback)
    }

    pub(crate) fn apply_directory_batch(&mut self, hits: Vec<SearchHit>) {
        self.window.hits.extend(hits);
        self.window.failure = None;
        self.enforce_result_total_limit();
    }

    // 结果总量硬上界：超限丢弃最旧页并置 truncated，翻页/回退两条路径共用。
    fn enforce_result_total_limit(&mut self) {
        let excess = self
            .window
            .hits
            .len()
            .saturating_sub(SEARCH_RESULT_TOTAL_LIMIT);
        if excess > 0 {
            self.window.hits.drain(0..excess);
            self.window.truncated = true;
        }
    }

    pub(crate) fn finish_directory_fallback(&mut self, outcome: DirectoryFallbackOutcome) {
        self.run.cancellation = None;
        self.window.is_loading = false;
        match outcome {
            DirectoryFallbackOutcome::Completed(completion) => {
                self.window.failure = None;
                self.window.completion = Some(match completion {
                    file_search::DirectoryFallbackCompletion::TraversalComplete { .. } => {
                        SearchResultCompletion::Complete
                    }
                    file_search::DirectoryFallbackCompletion::EntryBudgetReached {
                        inspected_entries,
                    } => SearchResultCompletion::Partial { inspected_entries },
                });
            }
            DirectoryFallbackOutcome::Cancelled => {
                self.window.failure = None;
            }
            DirectoryFallbackOutcome::Failed(message) => {
                self.window.failure = Some(message);
            }
        }
        self.selection.reconcile(&self.window.hits);
    }

    pub(crate) fn clear_query(&mut self) {
        self.run.cancel();
        self.run.generation = self.run.generation.saturating_add(1);
        self.run.active_query = None;
        self.run.provider = None;
        self.run.next_cursor = None;
        self.run.pending_indexed_request = None;
        self.window.hits.clear();
        self.window.is_loading = false;
        self.window.failure = None;
        self.window.completion = None;
        self.window.truncated = false;
        self.window.viewport_offset_y = 0.0;
        self.window.viewport_height = 0.0;
        self.selection.clear();
    }

    pub(crate) fn reject_query(&mut self, message: String) {
        self.clear_query();
        self.window.failure = Some(message);
    }

    pub(crate) fn selected_paths_in_result_order(&self) -> Vec<PathBuf> {
        self.selection
            .selected_paths_in_result_order(&self.window.hits)
    }

    pub(crate) fn hit_for_path(&self, path: &Path) -> Option<&SearchHit> {
        self.window.hits.iter().find(|hit| hit.path == path)
    }

    pub(crate) fn content_search_is_degraded(&self) -> bool {
        self.run.provider == Some(SearchProvider::DirectoryFallback)
            && self
                .run
                .active_query
                .as_ref()
                .is_some_and(|query| query.text_scope == SearchTextScope::NameAndContent)
    }
}

impl Drop for SearchWorkspaceState {
    fn drop(&mut self) {
        self.run.cancel();
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
