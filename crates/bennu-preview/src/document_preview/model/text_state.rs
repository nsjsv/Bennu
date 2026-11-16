//! 分页文档的文字层状态：抽没抽、搜索条、拖选。都挂在文档上，关掉文档一起丢。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{DocumentPreviewRequestKey, PagedDocumentPreview};
use crate::commands::document_preview::DocumentTextRequest;
use crate::document_preview::grapheme_edges::{grapheme_edges, nearest_edge};
use crate::document_preview::text_layer::{
    grapheme_count, word_near, DocumentSearch, DocumentSelection, DocumentText, DocumentTextLayer,
    PageRect, TextPosition,
};
use crate::text_document::{SearchDirection, SearchSummary};

/// 匹配滚到视口上面三分之一处，上下文看得见。
const MATCH_VIEWPORT_POSITION: f32 = 1.0 / 3.0;

#[derive(Debug, Clone)]
pub(super) struct TextState {
    layer: DocumentTextLayer,
    search: Option<DocumentSearch>,
    selection: Option<DocumentSelection>,
    /// 指针最近在哪页哪（页内比例）：按下时从这里起选。
    pointer: Option<(usize, f32, f32)>,
    selecting: bool,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            layer: DocumentTextLayer::NotLoaded,
            search: None,
            selection: None,
            pointer: None,
            selecting: false,
        }
    }
}

/// 一页上要画的标记：匹配、当前匹配、选中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageMarkKind {
    Match,
    CurrentMatch,
    Selection,
}

impl PagedDocumentPreview {
    /// 还没抽过文字就去抽（只抽一次）；已经在抽或抽好了返回 None。
    pub fn request_text(&mut self, cancellation: CancellationToken) -> Option<DocumentTextRequest> {
        if !matches!(self.text.layer, DocumentTextLayer::NotLoaded) {
            return None;
        }
        self.text.layer = DocumentTextLayer::Loading;
        Some(DocumentTextRequest {
            key: self.key.clone(),
            workspace: self.workspace.clone(),
            page_sizes: self
                .pages
                .iter()
                .map(|page| (page.width, page.height))
                .collect(),
            cancellation,
        })
    }

    /// 文字抽好了：搜索条开着就按当前词找一遍。返回是不是这份文档的结果。
    pub fn accept_text(
        &mut self,
        key: &DocumentPreviewRequestKey,
        outcome: Result<Arc<DocumentText>, String>,
    ) -> bool {
        if key != &self.key {
            return false;
        }
        self.text.layer = match outcome {
            Ok(text) => DocumentTextLayer::Ready(text),
            Err(error) => DocumentTextLayer::Failed(error),
        };
        self.refresh_search();
        true
    }

    pub fn text_layer(&self) -> &DocumentTextLayer {
        &self.text.layer
    }

    pub fn search(&self) -> Option<&DocumentSearch> {
        self.text.search.as_ref()
    }

    pub fn search_summary(&self) -> Option<SearchSummary> {
        self.text.search.as_ref()?.summary(&self.text.layer)
    }

    pub fn open_search(&mut self) {
        self.text.search.get_or_insert_with(DocumentSearch::default);
    }

    pub fn close_search(&mut self) {
        self.text.search = None;
    }

    pub fn set_search_query(&mut self, query: String) {
        if let Some(search) = self.text.search.as_mut() {
            search.query = query;
        }
        self.refresh_search();
    }

    pub fn toggle_search_case(&mut self) {
        if let Some(search) = self.text.search.as_mut() {
            search.case_sensitive = !search.case_sensitive;
        }
        self.refresh_search();
    }

    pub fn step_search(&mut self, direction: SearchDirection) {
        if let Some(search) = self.text.search.as_mut() {
            search.step(direction);
        }
    }

    fn refresh_search(&mut self) {
        let from_page = self.current_page();
        if let (Some(search), DocumentTextLayer::Ready(text)) =
            (self.text.search.as_mut(), &self.text.layer)
        {
            search.refresh(text, from_page);
        }
    }

    /// 当前匹配滚到视口上部；没有匹配或当前匹配已经整个看得见时 None（不乱跳）。
    pub fn scroll_to_current_match(&mut self) -> Option<f32> {
        let found = self.text.search.as_ref()?.current_match()?;
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        let bounds = text
            .pages
            .get(found.page)?
            .words
            .get(found.first_word)?
            .bounds;
        let layout = self.page_layout(found.page)?;
        let top = layout.top + bounds.top * layout.height;
        let bottom = layout.top + bounds.bottom * layout.height;
        if top >= self.viewport_offset && bottom <= self.viewport_offset + self.viewport_height {
            return None;
        }
        Some(self.scroll_to_offset(top - self.viewport_height * MATCH_VIEWPORT_POSITION))
    }

    pub fn pointer_moved(&mut self, page: usize, x: f32, y: f32) {
        self.text.pointer = Some((page, x, y));
        if !self.text.selecting {
            return;
        }
        if let (Some(focus), Some(selection)) = (self.position_at_pointer(), self.text.selection) {
            self.text.selection = Some(DocumentSelection { focus, ..selection });
        }
    }

    /// 按下：在离指着处最近的字边界放下起点，什么都还没选（按在空白处就是清掉选区）；拖着走才选。
    /// 文字还没抽好时什么也不选（按下会去抽）。
    pub fn press(&mut self) {
        self.text.selection = self
            .position_at_pointer()
            .map(|position| DocumentSelection {
                anchor: position,
                focus: position,
            });
        self.text.selecting = matches!(self.text.layer, DocumentTextLayer::Ready(_));
    }

    /// 双击：选中指着的那个词。
    pub fn double_click(&mut self) {
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return;
        };
        if let Some((page, word)) = self.word_at_pointer() {
            self.text.selection = DocumentSelection::word(page, word, text);
        }
    }

    pub fn release(&mut self) {
        self.text.selecting = false;
    }

    /// 正按着拖选：视图这时才要逐次的指针移动。
    pub fn is_selecting(&self) -> bool {
        self.text.selecting
    }

    /// 选中的文字；只点了一下没拖（两端在同一处）就是没选。
    pub fn selected_text(&self) -> Option<String> {
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        let copied = self.text.selection?.text(text);
        (!copied.is_empty()).then_some(copied)
    }

    /// 某页要画的标记（页内比例）。
    pub fn page_marks(&self, page: usize) -> Vec<(PageRect, PageMarkKind)> {
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return Vec::new();
        };
        let Some(words) = text.pages.get(page).map(|page| &page.words) else {
            return Vec::new();
        };
        let mut marks = Vec::new();
        if let Some(search) = &self.text.search {
            for (index, found) in search.matches.iter().enumerate() {
                if found.page != page {
                    continue;
                }
                let kind = if search.current == Some(index) {
                    PageMarkKind::CurrentMatch
                } else {
                    PageMarkKind::Match
                };
                marks.extend(
                    words[found.first_word..=found.last_word]
                        .iter()
                        .map(|word| (word.bounds, kind)),
                );
            }
        }
        if let Some(selection) = self.text.selection {
            for (index, range) in selection.graphemes_on_page(page, text) {
                let word = &words[index];
                let count = grapheme_count(&word.text);
                // 整个词选中就是词框；只选了一段（选区两端那两个词）才去量字宽。
                let bounds = if range.start == 0 && range.end == count {
                    word.bounds
                } else {
                    let edges = grapheme_edges(&word.text);
                    let width = word.bounds.right - word.bounds.left;
                    PageRect {
                        left: word.bounds.left + edges[range.start] * width,
                        right: word.bounds.left + edges[range.end] * width,
                        ..word.bounds
                    }
                };
                marks.push((bounds, PageMarkKind::Selection));
            }
        }
        marks
    }

    fn word_at_pointer(&self) -> Option<(usize, usize)> {
        let (page, x, y) = self.text.pointer?;
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        Some((page, word_near(text.pages.get(page)?, x, y)?))
    }

    /// 离指着处最近的字边界：先找最近的词，再按这个词的字宽找词里最近的边界（在词左边就是词首，
    /// 在右边就是词尾）。
    fn position_at_pointer(&self) -> Option<TextPosition> {
        let (page, word) = self.word_at_pointer()?;
        let (_, x, _) = self.text.pointer?;
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        let found = &text.pages[page].words[word];
        let width = found.bounds.right - found.bounds.left;
        let fraction = if width > 0.0 {
            (x - found.bounds.left) / width
        } else {
            0.0
        };
        Some(TextPosition {
            page,
            word,
            grapheme: nearest_edge(&grapheme_edges(&found.text), fraction),
        })
    }
}
