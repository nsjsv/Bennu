use iced::Element;

use super::FileBrowser;
use crate::model::{Message, ScrollbarRegion};
use crate::view::view_preview_window;

impl FileBrowser {
    pub(super) fn preview_window_content(
        &self,
        preview_bottom_controls_opacity: f32,
    ) -> Element<'_, Message> {
        let preview_engine = &self.preview_engine;
        view_preview_window(
            preview_engine.preview.as_ref(),
            preview_engine.text_preview_document.as_ref(),
            preview_engine.sqlite_preview.as_ref(),
            preview_engine.preview_size,
            &preview_engine.preview_image_viewport,
            preview_engine.audio_preview.as_ref(),
            preview_engine.video_preview.as_ref(),
            preview_bottom_controls_opacity,
            self.operation_progress_animation_frame,
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewDirectory),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewDirectory),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewArchive),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewArchive),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewDocument),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewDocument),
            self.scrollbar_visibility_for(&ScrollbarRegion::TextPreview),
            self.scrollbar_viewport_for(&ScrollbarRegion::TextPreview),
            preview_engine.text_preview_content_height,
            self.scrollbar_visibility_for(&ScrollbarRegion::MarkdownPreview),
            self.scrollbar_viewport_for(&ScrollbarRegion::MarkdownPreview),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewSqliteTables),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewSqliteTables),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewSqliteData),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewSqliteData),
        )
    }
}
