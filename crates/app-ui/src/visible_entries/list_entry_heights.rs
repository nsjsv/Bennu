//! 列表条目行的高度：文件夹里等高，搜索结果里有摘要的行多一行摘要。

use std::hash::{Hash, Hasher};

use file_core::DirectoryEntry;

use crate::model::{BrowserLocation, SearchWorkspaceState};

/// 列表条目行的高度。文件夹里每行等高；搜索结果里有正文摘要的行多一行摘要。行流和各处
/// 视口范围（缩略图、文件夹摘要、元数据调度）都从这里取，行高只有一个来源。摘要行高与行高
/// 各自独立（都只由档位定），所以两个高度都由调用方给。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ListEntryHeights<'a> {
    row_height: f32,
    snippet_line_height: f32,
    search: Option<&'a SearchWorkspaceState>,
}

impl<'a> ListEntryHeights<'a> {
    pub(crate) fn at(
        location: &'a BrowserLocation,
        row_height: f32,
        snippet_line_height: f32,
    ) -> Self {
        Self {
            row_height,
            snippet_line_height,
            search: location.search(),
        }
    }

    pub(crate) fn row_height(&self) -> f32 {
        self.row_height
    }

    pub(crate) fn snippet_line_height(&self) -> f32 {
        self.snippet_line_height
    }

    /// 条目名字下面那行摘要的高度；没有摘要为 0。
    pub(crate) fn snippet_height(&self, entry: &DirectoryEntry) -> f32 {
        match self.search {
            Some(search)
                if search
                    .detail(entry.path())
                    .is_some_and(|detail| detail.snippet.is_some()) =>
            {
                self.snippet_line_height
            }
            _ => 0.0,
        }
    }
}

/// 排好的行流按它判断行高变没变：行高、摘要行高，搜索里再加摘要表的版本号。
impl Hash for ListEntryHeights<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.row_height.to_bits().hash(state);
        self.snippet_line_height.to_bits().hash(state);
        self.search
            .map(SearchWorkspaceState::details_revision)
            .hash(state);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ListEntryHeights;
    use crate::model::BrowserLocation;

    #[test]
    fn search_rows_with_a_snippet_are_taller_in_the_viewport_range() {
        use file_search::{MatchSource, SearchFileKind, SearchHit, SearchQuery, SearchResultBatch};

        use crate::model::search::{SearchWorkspaceSessionId, SearchWorkspaceState};

        let hit = |name: &str, snippet: Option<&str>| SearchHit {
            path: PathBuf::from(format!("/workspace/{name}")),
            display_name: name.to_owned(),
            kind: SearchFileKind::File,
            size: 0,
            modified_ms: None,
            accessed_ms: None,
            created_ms: None,
            rank: 1.0,
            snippet: snippet.map(str::to_owned),
            match_source: MatchSource::Content,
        };
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        let tab = browser.tab_mut();
        tab.location = BrowserLocation::Search(Box::new(SearchWorkspaceState::new(
            PathBuf::from("/workspace"),
            PathBuf::from("/home/test"),
            None,
            SearchWorkspaceSessionId(1),
        )));
        let (request, _) = tab
            .begin_search_query(SearchQuery {
                query_id: 1,
                terms: "report".to_owned(),
                text_scope: file_search::SearchTextScope::NameAndContent,
                match_mode: file_search::SearchMatchMode::Plain,
                scope: file_search::SearchScope::Directory(PathBuf::from("/workspace")),
                recursive: true,
                filters: Default::default(),
                order: file_search::SearchOrder::Relevance,
                limit: 100,
                cursor: None,
            })
            .unwrap();
        tab.accept_search_batch(
            request,
            SearchResultBatch {
                query_id: 1,
                hits: vec![
                    hit("a.txt", Some("a report line")),
                    hit("b.txt", None),
                    hit("c.txt", None),
                ],
                next_cursor: None,
                finished: true,
            },
        );
        let row = crate::list_view::LIST_ROW_HEIGHT;
        let snippet =
            crate::list_view::list_snippet_line_height(crate::config::ViewDensityLevel::DEFAULT);
        let heights = ListEntryHeights::at(&tab.location, row, snippet);
        assert_eq!(heights.snippet_height(&tab.entries[0]), snippet);
        assert_eq!(heights.snippet_height(&tab.entries[1]), 0.0);

        // 视口从第一行之后开始：第一行比普通行高出一行摘要。
        let range = crate::transfer_placeholder_view::list_row_layout(
            &browser,
            browser.pane_view(browser.active_pane_id()).unwrap(),
            row,
        )
        .range_for_viewport(0.0, row + snippet, 10.0, 0);
        assert_eq!(range.start, 1);
        assert_eq!(range.before_height, row + snippet);
        assert_eq!(range.after_height, row);
    }
}
