//! 文档预览状态机：自 app-ui 的 preview_state/document.rs 迁入
//! （impl FileBrowser → impl PreviewEngine，逐字节保真；命令直连本
//! crate 命令层）。宿主依赖以参数注入：排版视口（面板/窗口几何归宿主）、
//! 滚动部件 id（smooth_scroll 管道归宿主）、大小上限（宿主实时读配置）、
//! 会话保存与滚动条临时显示（宿主域，见转发层）。

use std::path::PathBuf;

use iced::widget::scrollable;
use iced::{widget::Id as WidgetId, Task};
use tokio_util::sync::CancellationToken;

use crate::commands::document_preview::{prepare_document_command, render_document_page_command};
use crate::document_preview::{
    DocumentPageRenderOutcome, DocumentPrepareOutcome, DocumentPrepareRequest,
    DocumentPreviewRequestKey, DocumentViewportKey, PagedDocumentPreview, PendingDocumentPreview,
};
use crate::preview::{PreviewContent, PreviewSize, PreviewState, PreviewWindowProfile};
use crate::preview_message::PreviewMessage;

const DOCUMENT_RENDER_REQUESTS_PER_PREVIEW: usize = 2;
const DOCUMENT_CONTENT_HEIGHT_TOLERANCE: f32 = 1.0;

impl super::PreviewEngine {
    pub fn start_document_preview(
        &mut self,
        path: PathBuf,
        max_file_bytes: u64,
    ) -> Task<PreviewMessage> {
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

        Task::batch([
            window_command,
            prepare_document_command(DocumentPrepareRequest {
                key,
                max_file_bytes,
                cancellation,
            }),
        ])
    }

    pub fn accept_document_preview_prepared(
        &mut self,
        outcome: DocumentPrepareOutcome,
        layout_size: PreviewSize,
        scroll_region_id: WidgetId,
    ) -> Task<PreviewMessage> {
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
                    scroll_region_id,
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

    pub fn accept_document_page_rendered(
        &mut self,
        outcome: DocumentPageRenderOutcome,
    ) -> Task<PreviewMessage> {
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        if !document.accept_page_outcome(outcome) {
            return Task::none();
        }
        self.schedule_document_page_renders()
    }

    /// 视口滚动回流：Some 表示视口确有变化，宿主需按原语义临时显示
    /// 滚动条并执行返回的渲染调度；None 表示过期/无效事件，原样空操作。
    pub fn handle_document_preview_scrolled(
        &mut self,
        key: DocumentViewportKey,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Option<Task<PreviewMessage>> {
        if !offset_y.is_finite()
            || !viewport_height.is_finite()
            || !content_height.is_finite()
            || viewport_height <= 0.0
            || content_height <= 0.0
        {
            return None;
        }
        let Some(document) = self.active_document_preview_mut() else {
            return None;
        };
        if (document.content_height() - content_height).abs() > DOCUMENT_CONTENT_HEIGHT_TOLERANCE
            || !document.update_viewport(&key, offset_y, viewport_height)
        {
            return None;
        }
        Some(self.schedule_document_page_renders())
    }

    pub fn resize_document_preview(&mut self, scroll_region_id: WidgetId) -> Task<PreviewMessage> {
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
            scroll_region_id,
            scrollable::AbsoluteOffset { x: 0.0, y: offset },
        );
        Task::batch([scroll, self.schedule_document_page_renders()])
    }

    pub(crate) fn cancel_document_preview(&mut self) {
        if let Some(pending) = self.pending_document_preview.take() {
            pending.cancellation.cancel();
        }
        if let Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) = &self.preview {
            document.cancel();
        }
    }

    fn schedule_document_page_renders(&mut self) -> Task<PreviewMessage> {
        let Some(document) = self.active_document_preview_mut() else {
            return Task::none();
        };
        let requests = document.drain_render_requests(DOCUMENT_RENDER_REQUESTS_PER_PREVIEW);
        Task::batch(requests.into_iter().map(render_document_page_command))
    }

    pub fn active_document_preview_mut(&mut self) -> Option<&mut PagedDocumentPreview> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) => {
                Some(document.as_mut())
            }
            _ => None,
        }
    }
}
