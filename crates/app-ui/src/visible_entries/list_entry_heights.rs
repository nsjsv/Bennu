//! 列表条目行的高度：文件夹里等高，搜索结果里有摘要的行多一行摘要。

use file_core::DirectoryEntry;

use crate::model::{BrowserLocation, SearchWorkspaceState};

/// 列表条目行的高度。文件夹里每行等高；搜索结果里有正文摘要的行多一行摘要。行流和各处
/// 视口范围（缩略图、文件夹摘要、元数据调度）都从这里取，行高只有一个来源。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ListEntryHeights<'a> {
    row_height: f32,
    snippets: Option<(&'a SearchWorkspaceState, f32)>,
}

impl<'a> ListEntryHeights<'a> {
    #[cfg(test)]
    pub(crate) fn uniform(row_height: f32) -> Self {
        Self {
            row_height,
            snippets: None,
        }
    }

    pub(crate) fn at(location: &'a BrowserLocation, row_height: f32) -> Self {
        Self {
            row_height,
            snippets: location.search().map(|search| {
                (
                    search,
                    crate::list_view::list_snippet_line_height(row_height),
                )
            }),
        }
    }

    pub(crate) fn row_height(&self) -> f32 {
        self.row_height
    }

    /// 条目名字下面那行摘要的高度；没有摘要为 0。
    pub(crate) fn snippet_height(&self, entry: &DirectoryEntry) -> f32 {
        match self.snippets {
            Some((search, height))
                if search
                    .detail(&entry.path)
                    .is_some_and(|detail| detail.snippet.is_some()) =>
            {
                height
            }
            _ => 0.0,
        }
    }

    pub(super) fn is_uniform(&self) -> bool {
        self.snippets.is_none()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::ListEntryHeights;
    use crate::model::BrowserLocation;
    use crate::visible_entries::list_entry_range_for_viewport;

    #[test]
    fn search_rows_with_a_snippet_are_taller_in_the_viewport_range() {
        use file_search::{MatchSource, SearchFileKind, SearchHit, SearchQuery, SearchResultBatch};

        use crate::model::search::{SearchWorkspaceSessionId, SearchWorkspaceState};
        use crate::model::BrowserTab;

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
        let mut tab = BrowserTab::directory(1, PathBuf::from("/workspace"));
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
        let heights = ListEntryHeights::at(&tab.location, 46.0);
        let snippet = crate::list_view::list_snippet_line_height(46.0);
        assert_eq!(heights.snippet_height(&tab.entries[0]), snippet);
        assert_eq!(heights.snippet_height(&tab.entries[1]), 0.0);

        // 视口从第一行之后开始：第一行比普通行高出一行摘要。
        let range = list_entry_range_for_viewport(
            &tab.entries,
            &HashMap::new(),
            heights,
            0.0,
            46.0 + snippet,
            10.0,
            0,
        );
        assert_eq!(range.start, 1);
        assert_eq!(range.before_height, 46.0 + snippet);
        assert_eq!(range.after_height, 46.0);
    }
}
