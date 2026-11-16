//! 分页文档的文字层状态：抽没抽、搜索条、拖选。都挂在文档上，关掉文档一起丢。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{DocumentPreviewRequestKey, PagedDocumentPreview};
use crate::commands::document_preview::DocumentTextRequest;
use crate::document_preview::text_layer::{
    word_near, word_under, DocumentSearch, DocumentSelection, DocumentText, DocumentTextLayer,
    PageRect,
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
    /// 按在哪：按在空白处时，拖到字上才从离这里最近的词开始选。
    pressed_at: Option<(usize, f32, f32)>,
    selecting: bool,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            layer: DocumentTextLayer::NotLoaded,
            search: None,
            selection: None,
            pointer: None,
            pressed_at: None,
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
        if self.text.selection.is_some() {
            if let Some(word) = self.word_at_pointer() {
                if let Some(selection) = self.text.selection.as_mut() {
                    selection.focus = word;
                }
            }
        } else if let Some(focus) = self.word_under_pointer() {
            // 从空白处拖到了字上：从离按下处最近的词选到这里。
            let anchor = self.word_near_press().unwrap_or(focus);
            self.text.selection = Some(DocumentSelection { anchor, focus });
        }
    }

    /// 按在字上：从这个词开始选（只点一下也算选了它）；按在空白处：清掉选区，拖到字上时再选。
    /// 文字还没抽好时什么也不选（按下会去抽）。
    pub fn press(&mut self) {
        self.text.pressed_at = self.text.pointer;
        self.text.selection = self.word_under_pointer().map(|word| DocumentSelection {
            anchor: word,
            focus: word,
        });
        self.text.selecting = matches!(self.text.layer, DocumentTextLayer::Ready(_));
    }

    pub fn release(&mut self) {
        self.text.selecting = false;
    }

    /// 正按着拖选：视图这时才要逐次的指针移动。
    pub fn is_selecting(&self) -> bool {
        self.text.selecting
    }

    /// 选中的文字；只点了一下没拖（选中一个词又松开在原处）也算选了那个词。
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
        if let Some((start, end)) = self
            .text
            .selection
            .and_then(|selection| selection.words_on_page(page, text))
        {
            marks.extend(
                words[start..=end]
                    .iter()
                    .map(|word| (word.bounds, PageMarkKind::Selection)),
            );
        }
        marks
    }

    fn word_at_pointer(&self) -> Option<(usize, usize)> {
        self.word_near_point(self.text.pointer?)
    }

    fn word_near_press(&self) -> Option<(usize, usize)> {
        self.word_near_point(self.text.pressed_at?)
    }

    fn word_near_point(&self, (page, x, y): (usize, f32, f32)) -> Option<(usize, usize)> {
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        Some((page, word_near(text.pages.get(page)?, x, y)?))
    }

    fn word_under_pointer(&self) -> Option<(usize, usize)> {
        let (page, x, y) = self.text.pointer?;
        let DocumentTextLayer::Ready(text) = &self.text.layer else {
            return None;
        };
        Some((page, word_under(text.pages.get(page)?, x, y)?))
    }
}
