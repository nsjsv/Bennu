//! 文本预览的搜索条与跳行条：把条上的操作交给文档，打开条时键盘给输入框，关掉后还给
//! 查看器。

use iced::Task;

use super::text::text_read_tasks;
use crate::preview_message::PreviewMessage;
use crate::text_document::FindEvent;
use crate::text_find_bar::focus_find_input;
use crate::text_preview_viewer::focus_text_viewer;

impl super::PreviewEngine {
    pub(crate) fn handle_find_event(&mut self, event: FindEvent) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let was_open = document.find_bar().is_some();
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
        let reads = text_read_tasks(document, reads);
        let focus = if focus_input {
            focus_find_input()
        } else if closed {
            focus_text_viewer()
        } else {
            Task::none()
        };
        Task::batch([reads, focus])
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
            return Task::batch([reads, focus_text_viewer()]);
        }
        reads
    }
}
