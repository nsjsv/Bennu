//! 分页文档的横向尺寸、当前页、翻页和页码框。

use super::{
    clamp_viewport_offset, page_at_offset, DocumentPageTurn, PagedDocumentPreview,
    DOCUMENT_PAGE_SIDE_MARGIN,
};

impl PagedDocumentPreview {
    /// 内容宽：页宽加两边留白，不比视口窄（缩小后页居中）。
    pub fn content_width(&self) -> f32 {
        (self.page_width + 2.0 * DOCUMENT_PAGE_SIDE_MARGIN).max(self.preview_width)
    }

    /// 放大后横向滚到正中的位置。
    pub fn centered_horizontal_offset(&self) -> f32 {
        ((self.content_width() - self.preview_width) / 2.0).max(0.0)
    }

    pub fn horizontal_offset(&self) -> f32 {
        self.horizontal_offset
            .min((self.content_width() - self.preview_width).max(0.0))
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// 视口中线落在哪页（页码框显示的那页，从 0 数）。
    pub fn current_page(&self) -> usize {
        page_at_offset(
            &self.page_tops,
            &self.display_heights,
            self.viewport_offset + self.viewport_height / 2.0,
        )
    }

    /// 翻页的目标页：往后是顶部那页的下一页；往前时顶部那页没从页头开始看就回它的页头。
    pub fn page_after_turn(&self, turn: DocumentPageTurn) -> usize {
        let last = self.pages.len() - 1;
        let top_page = page_at_offset(
            &self.page_tops,
            &self.display_heights,
            self.viewport_offset + 1.0,
        );
        match turn {
            DocumentPageTurn::First => 0,
            DocumentPageTurn::Last => last,
            DocumentPageTurn::Next => (top_page + 1).min(last),
            DocumentPageTurn::Previous if self.viewport_offset > self.page_tops[top_page] + 1.0 => {
                top_page
            }
            DocumentPageTurn::Previous => top_page.saturating_sub(1),
        }
    }

    /// 滚到某页页头（到底了就停在最底）；返回新的纵向位置。
    pub fn scroll_to_page(&mut self, page_index: usize) -> f32 {
        let page_index = page_index.min(self.pages.len() - 1);
        self.scroll_to_offset(self.page_tops[page_index])
    }

    /// 程序滚动：先自己更新视口和渲染集合，不等滚动回流。
    pub(super) fn scroll_to_offset(&mut self, offset: f32) -> f32 {
        self.viewport_offset =
            clamp_viewport_offset(offset, self.viewport_height, self.content_height);
        self.replace_render_window();
        self.viewport_offset
    }

    pub fn page_number_draft(&self) -> Option<&str> {
        self.page_number_draft.as_deref()
    }

    pub fn edit_page_number(&mut self, draft: String) {
        // 只收数字：页码框里打别的没有意义。
        self.page_number_draft = Some(draft.chars().filter(char::is_ascii_digit).collect());
    }

    /// 回车：按输入的页码（从 1 数）定目标页，越界的夹到首尾；没输或输了 0 不跳。
    pub fn take_page_number(&mut self) -> Option<usize> {
        let number = self.page_number_draft.take()?.parse::<usize>().ok()?;
        (number > 0).then(|| (number - 1).min(self.pages.len() - 1))
    }
}
