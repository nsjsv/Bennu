//! 文本预览的搜索条与跳行条：把条上的操作交给文档，打开条时键盘给输入框，关掉后还给
//! 文本预览；Markdown 渲染模式里当前匹配换了，就把它所在的块滚进视口。

use iced::Task;

use super::text::text_read_tasks;
use crate::markdown_preview::reveal_markdown_block;
use crate::preview_message::PreviewMessage;
use crate::text_document::FindEvent;
use crate::text_find_bar::focus_find_input;
use crate::text_preview::MarkdownPreviewMode;
use crate::text_preview_viewer::focus_preview_keyboard;

impl super::PreviewEngine {
    pub(crate) fn handle_find_event(&mut self, event: FindEvent) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let was_open = document.find_bar().is_some();
        let rendered_before = document.rendered_search().map(|(_, current)| current);
        let (reads, focus_input) = match event {
            FindEvent::OpenSearch { seed } => (document.open_search(seed), true),
            FindEvent::OpenGoto => {
                document.open_goto();
                (Vec::new(), true)
            }
            FindEvent::Close => {
                document.close_find();
                (Vec::new(), false)
            }
            FindEvent::QueryChanged(text) => (document.set_search_text(text), false),
            FindEvent::CaseToggled => (document.toggle_search_case(), false),
            // F3 在条没开时先把条打开。
            FindEvent::Step(direction) => (document.step_search(direction), !was_open),
            FindEvent::GotoInputChanged(input) => {
                document.set_goto_input(input);
                (Vec::new(), false)
            }
            FindEvent::GotoSubmitted => (document.submit_goto(), false),
        };
        let closed = was_open && document.find_bar().is_none();
        let rendered_after = document.rendered_search().map(|(_, current)| current);
        let reveal = match document.rendered_current_block() {
            Some(block) if rendered_after != rendered_before => reveal_markdown_block(block),
            _ => Task::none(),
        };
        let reads = text_read_tasks(document, reads);
        let focus = if focus_input {
            focus_find_input()
        } else if closed {
            focus_preview_keyboard()
        } else {
            Task::none()
        };
        Task::batch([reads, focus, reveal])
    }

    /// Markdown 渲染/原文切换：搜索按新模式重算，渲染模式里跳到第一个匹配所在的块。
    pub(crate) fn select_markdown_mode(
        &mut self,
        mode: MarkdownPreviewMode,
    ) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let reads = document.select_markdown_preview_mode(mode);
        let reveal = match document.rendered_current_block() {
            Some(block) => reveal_markdown_block(block),
            None => Task::none(),
        };
        Task::batch([text_read_tasks(document, reads), reveal])
    }

    /// 跳行定位到了：跳过去，条关掉，键盘回到查看器。
    pub(super) fn accept_line_start(
        &mut self,
        line: u64,
        outcome: Result<u64, String>,
    ) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let was_open = document.find_bar().is_some();
        let reads = document.accept_line_start(line, outcome);
        let reads = text_read_tasks(document, reads);
        if was_open && document.find_bar().is_none() {
            return Task::batch([reads, focus_preview_keyboard()]);
        }
        reads
    }
}
