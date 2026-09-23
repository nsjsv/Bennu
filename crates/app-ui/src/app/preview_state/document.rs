//! 文档预览状态机已迁 bennu-preview 的 PreviewEngine
//! （engine/document.rs，逐字节保真）；此处保留宿主侧入口与薄转发：
//! 排版视口（面板/窗口几何归宿主）、滚动部件 id（smooth_scroll 管道
//! 归宿主）与大小上限（实时读 user_config）由转发层注入，滚动条临时
//! 显示与浏览器会话保存是宿主域。

use std::path::PathBuf;

use iced::Task;

use crate::app::smooth_scroll::smooth_scroll_id;
use crate::app::FileBrowser;
use crate::document_preview::{DocumentPageRenderOutcome, DocumentPreviewMessage};
use crate::model::{Message, ScrollbarRegion};

impl FileBrowser {
    pub(in crate::app) fn handle_document_preview_message(
        &mut self,
        message: DocumentPreviewMessage,
    ) -> Task<Message> {
        match message {
            DocumentPreviewMessage::Prepared(outcome) => {
                self.accept_document_preview_prepared(outcome)
            }
            DocumentPreviewMessage::PageRendered(outcome) => {
                self.accept_document_page_rendered(outcome)
            }
            DocumentPreviewMessage::Scrolled {
                key,
                offset_y,
                viewport_height,
                content_height,
            } => self.handle_document_preview_scrolled(
                key,
                offset_y,
                viewport_height,
                content_height,
            ),
        }
    }

    pub(in crate::app) fn start_document_preview(&mut self, path: PathBuf) -> Task<Message> {
        let max_file_bytes = self.user_config.preview_size_limits.document_bytes;
        let command = self
            .preview_engine
            .start_document_preview(path, max_file_bytes);
        self.clear_global_error();
        self.sync_preview_window_focus();
        let session_save = self.request_browser_session_save();
        Task::batch([command.map(Message::Preview), session_save])
    }

    pub(in crate::app) fn accept_document_preview_prepared(
        &mut self,
        outcome: crate::document_preview::DocumentPrepareOutcome,
    ) -> Task<Message> {
        // 先取几何快照再借用引擎：方法实参求值借用整个 &self，与接收者
        // 的 &mut preview_engine 字段借用重叠，先绑定局部值隔开两者。
        let layout_size = self.preview_surface_viewport();
        let scroll_region_id = smooth_scroll_id(&ScrollbarRegion::PreviewDocument);
        let command = self.preview_engine.accept_document_preview_prepared(
            outcome,
            layout_size,
            scroll_region_id,
        );
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(in crate::app) fn accept_document_page_rendered(
        &mut self,
        outcome: DocumentPageRenderOutcome,
    ) -> Task<Message> {
        self.preview_engine
            .accept_document_page_rendered(outcome)
            .map(Message::Preview)
    }

    pub(in crate::app) fn handle_document_preview_scrolled(
        &mut self,
        key: crate::document_preview::DocumentViewportKey,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Task<Message> {
        match self.preview_engine.handle_document_preview_scrolled(
            key,
            offset_y,
            viewport_height,
            content_height,
        ) {
            Some(command) => Task::batch([
                self.show_scrollbars_temporarily(ScrollbarRegion::PreviewDocument),
                command.map(Message::Preview),
            ]),
            None => Task::none(),
        }
    }

    pub(in crate::app) fn resize_document_preview(&mut self) -> Task<Message> {
        let scroll_region_id = smooth_scroll_id(&ScrollbarRegion::PreviewDocument);
        self.preview_engine
            .resize_document_preview(scroll_region_id)
            .map(Message::Preview)
    }
}

#[cfg(test)]
#[path = "document/tests.rs"]
mod tests;
