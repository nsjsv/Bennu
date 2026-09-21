use std::path::PathBuf;

use iced::widget::scrollable;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::app::smooth_scroll::smooth_scroll_id;
use crate::app::FileBrowser;
use crate::commands::{prepare_document_command, render_document_page_command};
use crate::document_preview::{
    DocumentPageRenderOutcome, DocumentPrepareOutcome, DocumentPrepareRequest,
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentViewportKey, PagedDocumentPreview,
    PendingDocumentPreview,
};
use crate::model::{Message, PreviewContent, PreviewState, PreviewWindowProfile, ScrollbarRegion};

const DOCUMENT_RENDER_REQUESTS_PER_PREVIEW: usize = 2;
const DOCUMENT_CONTENT_HEIGHT_TOLERANCE: f32 = 1.0;

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
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.clear_preview();
        self.document_preview_generation = self.document_preview_generation.wrapping_add(1);
        let key = DocumentPreviewRequestKey {
            source_path: path.clone(),
            document_generation: self.document_preview_generation,
        };
        let cancellation = CancellationToken::new();
        self.pending_document_preview = Some(PendingDocumentPreview {
            key: key.clone(),
            cancellation: cancellation.clone(),
        });
        self.preview = Some(PreviewState::Loading(path));
        self.clear_global_error();

        Task::batch([
            window_command,
            prepare_document_command(DocumentPrepareRequest {
                key,
                max_file_bytes: self.user_config.preview_size_limits.document_bytes,
                cancellation,
            }),
            self.request_browser_session_save(),
        ])
    }

    pub(in crate::app) fn accept_document_preview_prepared(
        &mut self,
        outcome: DocumentPrepareOutcome,
    ) -> Task<Message> {
        let key = outcome.key().clone();
        let is_current = self
            .pending_document_preview
            .as_ref()
            .is_some_and(|pending| pending.key == key)
            && matches!(
                &self.preview,
                Some(PreviewState::Loading(path)) if path == &key.source_path
            );
        if !is_current {
            return Task::none();
        }
        let Some(pending) = self.pending_document_preview.take() else {
            return Task::none();
        };

        match outcome {
            DocumentPrepareOutcome::Ready(prepared) => {
                // 页面按呈现面视口排版:独立窗口用窗口尺寸,面板用面板宽度。
                let layout_size = self.preview_surface_viewport();
                let document = match PagedDocumentPreview::new(
                    prepared,
                    pending.cancellation,
                    layout_size.width,
                    layout_size.height,
                ) {
                    Ok(document) => document,
                    Err(error) => {
                        self.preview = Some(PreviewState::Error(error));
                        return Task::none();
                    }
                };
                self.text_preview_document = None;
                self.clear_audio_preview();
                self.clear_video_preview();
                self.preview = Some(PreviewState::Ready(PreviewContent::PagedDocument(
                    Box::new(document),
                )));
                let reset_scroll = iced::widget::operation::scroll_to(
                    smooth_scroll_id(&ScrollbarRegion::PreviewDocument),
                    scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
                reset_scroll.chain(self.schedule_document_page_renders())
            }
            DocumentPrepareOutcome::Failed(_, error) => {
                self.text_preview_document = None;
                self.clear_audio_preview();
                self.clear_video_preview();
                self.preview = Some(PreviewState::Error(error));
                Task::none()
            }
            DocumentPrepareOutcome::Cancelled(_) => Task::none(),
        }
    }

    pub(in crate::app) fn accept_document_page_rendered(
        &mut self,
        outcome: DocumentPageRenderOutcome,
    ) -> Task<Message> {
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        if !document.accept_page_outcome(outcome) {
            return Task::none();
        }
        self.schedule_document_page_renders()
    }

    pub(in crate::app) fn handle_document_preview_scrolled(
        &mut self,
        key: DocumentViewportKey,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Task<Message> {
        if !offset_y.is_finite()
            || !viewport_height.is_finite()
            || !content_height.is_finite()
            || viewport_height <= 0.0
            || content_height <= 0.0
        {
            return Task::none();
        }
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        if (document.content_height() - content_height).abs() > DOCUMENT_CONTENT_HEIGHT_TOLERANCE
            || !document.update_viewport(&key, offset_y, viewport_height)
        {
            return Task::none();
        }
        Task::batch([
            self.show_scrollbars_temporarily(ScrollbarRegion::PreviewDocument),
            self.schedule_document_page_renders(),
        ])
    }

    pub(in crate::app) fn resize_document_preview(&mut self) -> Task<Message> {
        let preview_size = self.preview_size;
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        let offset = match document.resize(preview_size.width, preview_size.height) {
            Ok(offset) => offset,
            Err(error) => {
                document.cancel();
                self.preview = Some(PreviewState::Error(error));
                return Task::none();
            }
        };
        let scroll = iced::widget::operation::scroll_to(
            smooth_scroll_id(&ScrollbarRegion::PreviewDocument),
            scrollable::AbsoluteOffset { x: 0.0, y: offset },
        );
        Task::batch([scroll, self.schedule_document_page_renders()])
    }

    pub(super) fn cancel_document_preview(&mut self) {
        if let Some(pending) = self.pending_document_preview.take() {
            pending.cancellation.cancel();
        }
        if let Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) = &self.preview {
            document.cancel();
        }
    }

    fn schedule_document_page_renders(&mut self) -> Task<Message> {
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        let requests = document.drain_render_requests(DOCUMENT_RENDER_REQUESTS_PER_PREVIEW);
        Task::batch(requests.into_iter().map(render_document_page_command))
    }

    fn active_document_preview_mut(&mut self) -> Option<&mut PagedDocumentPreview> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) => {
                Some(document.as_mut())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "document/tests.rs"]
mod tests;
