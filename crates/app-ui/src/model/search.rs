use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use file_search::text_highlight::{content_match_ranges, name_match_ranges};
use file_search::text_matching::SearchTerms;
use file_search::{
    MatchSource, SearchCursor, SearchHit, SearchMatchMode, SearchOrder, SearchQuery,
    SearchResultBatch, SearchTextScope,
};
use tokio_util::sync::CancellationToken;

mod filters;
pub(crate) use filters::{
    SearchDateField, SearchDatePreset, SearchEntryTypePreset, SearchFilterPresetState,
    SearchSizePreset,
};
mod history;
pub(crate) use history::SearchHistory;
mod order;
mod scope;
pub(crate) use scope::{LastSearchScope, SearchDirectoryScope, SearchRootSnapshot};

pub(crate) const SEARCH_RESULT_WINDOW: usize = 100;
/// 结果总量硬上界：无论翻页多少页，最多保留这么多条，超限丢弃最旧页，
/// 保证长会话滚动加载不会让内存无限增长。
pub(crate) const SEARCH_RESULT_TOTAL_LIMIT: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl SearchInputStabilizationRequest {
    /// 发出请求的那次搜索：输入停下时标签可能已不是活动的，按它找回。
    pub(crate) fn session_id(&self) -> SearchWorkspaceSessionId {
        self.workspace_session_id
    }
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
    /// 服务里定格的排序结果已释放或服务重启过，这次搜索只能从头再来。
    SnapshotExpired,
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

/// 一次查询的身份：哪个标签的哪次搜索（会话号）、第几次发起（代号）。异步回信凭它找回
/// 标签，标签关了、换了搜索或重新发起后旧回信自然对不上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchRunId {
    pub(crate) session_id: SearchWorkspaceSessionId,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexedSearchRequest {
    pub(crate) run: SearchRunId,
    pub(crate) cursor: Option<SearchCursor>,
}

#[derive(Debug)]
pub(crate) struct SearchRunState {
    pub(crate) generation: u64,
    pub(crate) active_query: Option<SearchQuery>,
    pub(crate) provider: Option<SearchProvider>,
    pub(crate) next_cursor: Option<SearchCursor>,
    pub(crate) pending_indexed_request: Option<IndexedSearchRequest>,
    cancellation: Option<CancellationToken>,
}

/// 副本不持有在途请求：只有原件能取消它、收它的回信，副本被丢弃时也不会取消原件的查询。
impl Clone for SearchRunState {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            active_query: self.active_query.clone(),
            provider: self.provider,
            next_cursor: self.next_cursor,
            pending_indexed_request: None,
            cancellation: None,
        }
    }
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

/// 这次搜索的进度与提示；结果本身是标签的条目（见 `search_results`）。
#[derive(Debug, Clone)]
pub(crate) struct SearchResultWindow {
    pub(crate) is_loading: bool,
    pub(crate) failure: Option<String>,
    pub(crate) completion: Option<SearchResultCompletion>,
    pub(crate) truncated: bool,
}

impl SearchResultWindow {
    fn new() -> Self {
        Self {
            is_loading: false,
            failure: None,
            completion: None,
            truncated: false,
        }
    }

    fn begin_loading(&mut self) {
        self.is_loading = true;
        self.failure = None;
        self.completion = None;
        self.truncated = false;
    }
}

/// 一条结果里条目装不下的信息：正文摘要、命中来源，以及名字和摘要里要高亮的位置
/// （字节区间，按显示的文字算）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchHitDetail {
    pub(crate) snippet: Option<String>,
    pub(crate) match_source: MatchSource,
    pub(crate) name_highlights: Vec<Range<usize>>,
    pub(crate) snippet_highlights: Vec<Range<usize>>,
}

/// 名字高亮与匹配同一规则：正则模式按正则命中的位置，否则按规范化后的子串。
struct SearchHighlighter {
    terms: SearchTerms,
    name_regex: Option<regex::Regex>,
}

impl SearchHighlighter {
    fn name_ranges(&self, display_name: &str) -> Vec<Range<usize>> {
        match &self.name_regex {
            Some(regex) => regex
                .find_iter(display_name)
                .filter(|found| !found.is_empty())
                .map(|found| found.range())
                .collect(),
            None => name_match_ranges(display_name, &self.terms),
        }
    }
}

/// 一页结果落到标签上的方式：第一页换掉全部，后续页接在后面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchHitsPlacement {
    Replace,
    Append,
}

/// 后退/前进回到一次搜索所需的全部意图；回去时按它重搜。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchRecall {
    pub(crate) origin: PathBuf,
    pub(crate) scope: SearchDirectoryScope,
    pub(crate) last_location: Option<LastSearchScope>,
    pub(crate) input: String,
    pub(crate) filters: SearchFilterPresetState,
    pub(crate) order: SearchOrder,
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
    /// 点表头选的顺序，见 `order()`。
    chosen_order: Option<order::ChosenSearchOrder>,
    pub(crate) run: SearchRunState,
    pub(crate) window: SearchResultWindow,
    /// 已到结果的摘要与高亮，按路径找；与标签条目一一对应。
    pub(crate) details: HashMap<PathBuf, SearchHitDetail>,
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
            chosen_order: None,
            run: SearchRunState::new(),
            window: SearchResultWindow::new(),
            details: HashMap::new(),
        }
    }

    /// 按历史里记下的意图重建一次搜索（新会话号）；调用方随后重搜。
    pub(crate) fn from_recall(
        recall: SearchRecall,
        home: PathBuf,
        session_id: SearchWorkspaceSessionId,
    ) -> Self {
        let mut state = Self::new(recall.origin, home, recall.last_location, session_id);
        state.root.select_scope(recall.scope);
        state.input = recall.input;
        state.filters = recall.filters;
        state.choose_order(recall.order);
        state
    }

    pub(crate) fn recall(&self) -> SearchRecall {
        SearchRecall {
            origin: self.origin().to_path_buf(),
            scope: self.root.selected_scope(),
            last_location: self.root.last_location.clone(),
            input: self.input.clone(),
            filters: self.filters.clone(),
            order: self.order(),
        }
    }

    /// 发起搜索的文件夹：关闭搜索回到这里，会话恢复也回到这里。
    pub(crate) fn origin(&self) -> &Path {
        &self.root.current_folder
    }

    /// 拆分窗格复制出的副本换一个会话号，与原件分开收回信。
    pub(crate) fn rebind_session(&mut self, session_id: SearchWorkspaceSessionId) {
        self.session_id = session_id;
    }

    pub(crate) fn session_id(&self) -> SearchWorkspaceSessionId {
        self.session_id
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
            run: SearchRunId {
                session_id: self.session_id,
                generation: query.query_id,
            },
            cursor: None,
        };
        self.run.cancellation = Some(cancellation.clone());
        self.run.generation = query.query_id;
        self.run.active_query = Some(query);
        self.run.provider = Some(SearchProvider::Indexed);
        self.run.next_cursor = None;
        self.run.pending_indexed_request = Some(request);
        self.window.begin_loading();
        self.details.clear();
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
            run: self.run_id(),
            cursor: Some(cursor),
        };
        let cancellation = CancellationToken::new();
        self.run.cancellation = Some(cancellation.clone());
        self.run.pending_indexed_request = Some(request);
        self.window.is_loading = true;
        self.window.failure = None;
        Some((request, query, cancellation))
    }

    pub(crate) fn run_id(&self) -> SearchRunId {
        SearchRunId {
            session_id: self.session_id,
            generation: self.run.generation,
        }
    }

    pub(crate) fn accepts_indexed_outcome(&self, request: IndexedSearchRequest) -> bool {
        request.run == self.run_id()
            && self.run.provider == Some(SearchProvider::Indexed)
            && self.run.pending_indexed_request == Some(request)
    }

    /// 收下一页：返回这页的命中和它们落到标签上的方式。
    pub(crate) fn accept_indexed_batch(
        &mut self,
        request: IndexedSearchRequest,
        mut batch: SearchResultBatch,
    ) -> (SearchHitsPlacement, Vec<SearchHit>) {
        self.run.cancellation = None;
        self.run.pending_indexed_request = None;
        batch.hits.truncate(SEARCH_RESULT_WINDOW);
        let placement = if request.cursor.is_none() {
            self.window.truncated = false;
            SearchHitsPlacement::Replace
        } else {
            SearchHitsPlacement::Append
        };
        self.run.next_cursor = batch.next_cursor;
        self.window.is_loading = false;
        self.window.failure = None;
        self.window.completion = Some(if batch.finished {
            SearchResultCompletion::Complete
        } else {
            SearchResultCompletion::MoreAvailable
        });
        (placement, batch.hits)
    }

    /// 第一页失败时之前的结果不再算数，返回 true 让标签清空结果。
    pub(crate) fn apply_indexed_failure(
        &mut self,
        request: IndexedSearchRequest,
        message: String,
    ) -> bool {
        self.run.cancellation = None;
        self.run.pending_indexed_request = None;
        self.window.is_loading = false;
        self.window.failure = Some(message);
        request.cursor.is_none()
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

    /// `loaded` 是标签上已有的结果数。
    pub(crate) fn indexed_next_page_is_available(&self, loaded: usize) -> bool {
        self.run.provider == Some(SearchProvider::Indexed)
            && self.window.completion == Some(SearchResultCompletion::MoreAvailable)
            && self.run.next_cursor.is_some()
            && self.run.pending_indexed_request.is_none()
            && loaded < SEARCH_RESULT_TOTAL_LIMIT
    }

    pub(crate) fn begin_directory_fallback(&mut self) -> CancellationToken {
        self.run.cancel();
        let cancellation = CancellationToken::new();
        self.run.cancellation = Some(cancellation.clone());
        self.run.provider = Some(SearchProvider::DirectoryFallback);
        self.run.next_cursor = None;
        self.run.pending_indexed_request = None;
        self.window.begin_loading();
        self.details.clear();
        cancellation
    }

    pub(crate) fn accepts_directory_fallback(&self, run: SearchRunId) -> bool {
        run == self.run_id() && self.run.provider == Some(SearchProvider::DirectoryFallback)
    }

    pub(crate) fn accept_directory_batch(&mut self) {
        self.window.failure = None;
    }

    /// 命中转成详情（摘要、高亮）记下；高亮按这次查询的词与匹配方式算。
    pub(crate) fn remember_details(&mut self, hits: &[SearchHit], display_names: &[String]) {
        let highlighter = self.highlighter();
        for (hit, display_name) in hits.iter().zip(display_names) {
            let snippet = hit.snippet.clone().filter(|snippet| !snippet.is_empty());
            let snippet_highlights = snippet
                .as_deref()
                .map(|snippet| content_match_ranges(snippet, &highlighter.terms))
                .unwrap_or_default();
            self.details.insert(
                hit.path.clone(),
                SearchHitDetail {
                    snippet,
                    match_source: hit.match_source,
                    name_highlights: highlighter.name_ranges(display_name),
                    snippet_highlights,
                },
            );
        }
    }

    /// 结果改名或移动：详情跟到新路径，名字高亮按新名字重算。每项是（旧路径、新路径、
    /// 新的显示名）。
    pub(crate) fn migrate_details(&mut self, moved: Vec<(PathBuf, PathBuf, String)>) {
        let highlighter = self.highlighter();
        let details = moved
            .into_iter()
            .filter_map(|(previous, migrated, display_name)| {
                let mut detail = self.details.remove(&previous)?;
                detail.name_highlights = highlighter.name_ranges(&display_name);
                Some((migrated, detail))
            })
            .collect::<Vec<_>>();
        self.details.extend(details);
    }

    fn highlighter(&self) -> SearchHighlighter {
        let (terms, match_mode) = self
            .run
            .active_query
            .as_ref()
            .map(|query| (query.terms.as_str(), query.match_mode))
            .unwrap_or(("", SearchMatchMode::Plain));
        SearchHighlighter {
            terms: SearchTerms::parse(terms),
            name_regex: match match_mode {
                SearchMatchMode::Regex => match_mode.name_regex(terms).ok(),
                SearchMatchMode::Plain => None,
            },
        }
    }

    /// 超出总量上界时丢掉了最旧的若干条。
    pub(crate) fn note_truncated(&mut self) {
        self.window.truncated = true;
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
    }

    pub(crate) fn clear_query(&mut self) {
        self.run.cancel();
        self.run.generation = self.run.generation.saturating_add(1);
        self.run.active_query = None;
        self.run.provider = None;
        self.run.next_cursor = None;
        self.run.pending_indexed_request = None;
        self.window.is_loading = false;
        self.window.failure = None;
        self.window.completion = None;
        self.window.truncated = false;
        self.details.clear();
    }

    pub(crate) fn reject_query(&mut self, message: String) {
        self.clear_query();
        self.window.failure = Some(message);
    }

    pub(crate) fn detail(&self, path: &Path) -> Option<&SearchHitDetail> {
        self.details.get(path)
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
