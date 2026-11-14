use iced::Element;

use super::FileBrowser;
use crate::model::Message;
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
            self.user_config.text_preview_view,
            bennu_preview::text_highlight::HighlightPalette::for_theme(
                &self.theme(self.main_window),
            ),
        )
    }
}
