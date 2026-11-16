//! 文档文字层的交互：搜索条（Ctrl+F）、拖选、Ctrl+C。文字第一次要用时才在后台抽；
//! 要滚动（跳到匹配）的地方用宿主给的滚动部件 id。

use iced::widget::scrollable;
use iced::{widget::Id as WidgetId, Task};

use crate::commands::document_preview::extract_document_text_command;
use crate::document_preview::DocumentTextMessage;
use crate::preview::{PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;
use crate::text_find_bar::focus_find_input;
use crate::text_preview_viewer::focus_preview_keyboard;

impl super::PreviewEngine {
    pub fn handle_document_text(
        &mut self,
        message: DocumentTextMessage,
        scroll_region_id: WidgetId,
    ) -> Task<PreviewMessage> {
        let Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) = &mut self.preview
        else {
            return Task::none();
        };
        let text_request = match message {
            DocumentTextMessage::Extracted { key, outcome } => {
                if !document.accept_text(&key, outcome) {
                    return Task::none();
                }
                None
            }
            DocumentTextMessage::SearchOpened => {
                document.open_search();
                let request = document.request_text(document.document_cancellation());
                return Task::batch([
                    focus_find_input(),
                    request.map_or_else(Task::none, extract_document_text_command),
                ]);
            }
            DocumentTextMessage::QueryChanged(query) => {
                document.set_search_query(query);
                None
            }
            DocumentTextMessage::CaseToggled => {
                document.toggle_search_case();
                None
            }
            DocumentTextMessage::Stepped(direction) => {
                document.step_search(direction);
                None
            }
            DocumentTextMessage::SearchClosed => {
                document.close_search();
                return focus_preview_keyboard();
            }
            DocumentTextMessage::PointerMoved { page, x, y } => {
                document.pointer_moved(page, x, y);
                return Task::none();
            }
            // 第一次按下才去抽文字；抽好之前点了不选。
            DocumentTextMessage::Pressed { page, x, y } => {
                document.pointer_moved(page, x, y);
                document.press();
                document.request_text(document.document_cancellation())
            }
            DocumentTextMessage::DoubleClicked { page, x, y } => {
                document.pointer_moved(page, x, y);
                document.double_click();
                return Task::none();
            }
            DocumentTextMessage::Released => {
                document.release();
                return Task::none();
            }
            DocumentTextMessage::CopyRequested => {
                return document
                    .selected_text()
                    .map_or_else(Task::none, iced::clipboard::write);
            }
        };
        let extract = text_request.map_or_else(Task::none, extract_document_text_command);
        let Some(offset) = document.scroll_to_current_match() else {
            return extract;
        };
        let scroll = iced::widget::operation::scroll_to(
            scroll_region_id,
            scrollable::AbsoluteOffset {
                x: document.horizontal_offset(),
                y: offset,
            },
        );
        Task::batch([extract, scroll, self.schedule_document_page_renders()])
    }
}
