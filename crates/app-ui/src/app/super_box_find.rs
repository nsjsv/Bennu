//! 超级输入框查索引：停顿一下再查，查的时候可以取消，结果按 `generation` 交回框；回车打开结果，
//! Ctrl+Enter 把词和筛选带进搜索页。

use std::time::Duration;

use file_search::{
    SearchFileKind, SearchHit, SearchMatchMode, SearchOrder, SearchQuery, SearchScope,
};
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::super_box::SuperBoxMessage;
use super::FileBrowser;
use crate::model::{BrowserViewMode, IndexedSearchOutcome, Message, NavigationMode};
use crate::super_box::{BoxLookup, BoxMode, FoundInColumns};

/// 打字停这么久才查：连着打的时候不发一串查询。
const FIND_PAUSE: Duration = Duration::from_millis(150);

impl FileBrowser {
    /// 先等一会儿；等完了输入还没变才真的查（`FindSettled`）。
    pub(super) fn schedule_super_box_find(&self, lookup: BoxLookup) -> Task<Message> {
        Task::perform(tokio::time::sleep(FIND_PAUSE), move |()| {
            Message::SuperBox(SuperBoxMessage::FindSettled(Box::new(lookup.clone())))
        })
    }

    pub(super) fn run_super_box_find(&mut self, lookup: BoxLookup) -> Task<Message> {
        let BoxLookup::Find {
            generation,
            query,
            text_scope,
            limit,
        } = lookup
        else {
            return Task::none();
        };
        let Some(super_box) = self
            .super_box
            .as_mut()
            .filter(|super_box| super_box.generation() == generation)
        else {
            return Task::none();
        };
        let cancellation = CancellationToken::new();
        super_box.track_find(cancellation.clone());
        let query = SearchQuery {
            query_id: generation,
            terms: query.terms,
            text_scope,
            match_mode: SearchMatchMode::Plain,
            scope: SearchScope::Global,
            recursive: true,
            filters: query.search_filters,
            order: SearchOrder::Relevance,
            limit,
            cursor: None,
        };
        Task::perform(
            crate::commands::search_once(query, cancellation),
            move |outcome| {
                Message::SuperBox(SuperBoxMessage::FoundLoaded {
                    generation,
                    outcome: Box::new(outcome),
                })
            },
        )
    }

    pub(super) fn accept_super_box_found(
        &mut self,
        generation: u64,
        outcome: IndexedSearchOutcome,
    ) -> Task<Message> {
        // 换了新的查询、框关了：结果没人要。
        let Some(found) = found_or_why(outcome) else {
            return Task::none();
        };
        let Some(mut super_box) = self.super_box.take() else {
            return Task::none();
        };
        // 搜索模式本来就有别的结果：服务没在跑时不列也不提示。
        let found = match (found, super_box.mode()) {
            (Err(_), BoxMode::Search) => Ok(Vec::new()),
            (found, _) => found,
        };
        self.with_super_box_context(|context| super_box.accept_found(generation, found, context));
        self.super_box = Some(super_box);
        Task::none()
    }

    /// 回车：在文件区里定位它。
    pub(super) fn open_super_box_found(&mut self, hit: SearchHit) -> Task<Message> {
        self.show_path_in_file_area(hit.path, hit.kind == SearchFileKind::Directory)
    }

    /// 在文件区里定位一个找到的路径（超级输入框找到的、终端里 Ctrl+点击的）：进入所在文件夹并
    /// 选中它（是文件夹就进入它）。多栏视图按设置来：从第一栏往下展开、所在文件夹作第一栏、
    /// 或者直接打开文件。
    pub(crate) fn show_path_in_file_area(
        &mut self,
        path: std::path::PathBuf,
        is_directory: bool,
    ) -> Task<Message> {
        let (folder, selection) = if is_directory {
            (path, None)
        } else {
            match path.parent() {
                Some(parent) => (parent.to_path_buf(), Some(path)),
                None => return Task::none(),
            }
        };
        let columns = self.tab().view_mode() == BrowserViewMode::Columns;
        match (
            columns,
            self.user_config.super_box_found_in_columns,
            selection,
        ) {
            (true, FoundInColumns::ExpandFromFirstColumn, selection) => {
                self.reveal_in_columns(folder, selection)
            }
            (true, FoundInColumns::OpenFile, Some(file)) => {
                crate::commands::open_file_command(file, self.terminal_emulator)
            }
            (_, _, selection) => {
                let navigation = self.navigate_to(folder, NavigationMode::RecordHistory);
                let Some(selection) = selection else {
                    return navigation;
                };
                // 找到的是隐藏文件时也要在所在文件夹里列出来，才选得中。
                let revealed = self.reveal_path(selection.clone());
                self.select_path(selection);
                Task::batch([navigation, revealed])
            }
        }
    }

    /// Ctrl+Enter（文件模式）：关框，在搜索页里接着找。
    pub(super) fn continue_super_box_find_in_search_page(&mut self) -> Task<Message> {
        let Some(super_box) = self.super_box.as_ref() else {
            return Task::none();
        };
        let Ok(query) = crate::super_box::read_find_query(super_box.input()) else {
            return Task::none();
        };
        let remember = super_box.history_text();
        self.super_box = None;
        let remembered = self.remember_super_box_input(remember);
        Task::batch([
            remembered,
            self.open_search_with(query.terms, query.filters),
        ])
    }
}

/// 索引查到的，或者给用户看的查不了的原因；取消了的（没人要）是 None。
pub(super) fn found_or_why(
    outcome: IndexedSearchOutcome,
) -> Option<Result<Vec<SearchHit>, String>> {
    Some(match outcome {
        IndexedSearchOutcome::Batch(batch) => Ok(batch.hits),
        IndexedSearchOutcome::Cancelled => return None,
        IndexedSearchOutcome::TransportUnavailable(_)
        | IndexedSearchOutcome::ProviderUnavailable(_) => Err(tr!(
            "The search service is not running; start it on the Search settings page"
        )),
        IndexedSearchOutcome::SnapshotExpired => Err(tr!("The index changed; type again")),
        IndexedSearchOutcome::InvalidQuery(reason) | IndexedSearchOutcome::Fatal(reason) => {
            Err(reason)
        }
    })
}
