use iced::Size;
use iced::Task;

use super::FileBrowser;
use crate::model::{
    image_preview_size, image_preview_zoom_multiplier, Message, PreviewContent,
    PreviewImageViewportMessage, PreviewState,
};

impl FileBrowser {
    /// 图片预览的缩放/平移交互。仅在静态图片预览显示时生效：
    /// GIF/视频等面板没有接入 mouse_area，收到迟到消息也直接忽略。
    pub(in crate::app) fn update_preview_image_viewport(
        &mut self,
        message: PreviewImageViewportMessage,
    ) -> Task<Message> {
        let (image_width, image_height) = match &self.preview {
            Some(PreviewState::Ready(PreviewContent::Image(content))) => content.dimensions(),
            _ => return Task::none(),
        };

        // 面板/独立窗口两种表面的媒体区尺寸分派，与视图渲染基准同源。
        let panel_size = self.preview_surface_viewport();
        let panel = Size::new(panel_size.width, panel_size.height);
        let (fit_width, fit_height) = image_preview_size(panel_size, image_width, image_height);
        let fit = Size::new(fit_width, fit_height);
        let viewport = &mut self.preview_image_viewport;
        match message {
            PreviewImageViewportMessage::PointerMoved(position) => {
                viewport.apply_pointer_motion(position, panel, fit);
            }
            PreviewImageViewportMessage::PanStarted => {
                tracing::debug!(target: "app_ui::preview", "[viewport] pan started");
                viewport.panning = true;
            }
            PreviewImageViewportMessage::PanEnded => {
                tracing::debug!(target: "app_ui::preview", "[viewport] pan ended");
                viewport.panning = false;
            }
            PreviewImageViewportMessage::Zoomed(delta) => {
                let multiplier = image_preview_zoom_multiplier(delta);
                let anchor = viewport.pointer;
                let before_scale = viewport.scale;
                let before_offset = viewport.offset;
                viewport.apply_zoom(multiplier, anchor, panel, fit);
                tracing::debug!(
                    target: "app_ui::preview",
                    "[viewport] zoom delta={delta:?} multiplier={multiplier:.4} anchor={anchor:?} \
                     panel={panel:.1?} fit={fit:.1?} before=(scale={before_scale:.3}, offset=({:.1},{:.1})) \
                     after=(scale={:.3}, offset=({:.1},{:.1}))",
                    before_offset.x,
                    before_offset.y,
                    viewport.scale,
                    viewport.offset.x,
                    viewport.offset.y,
                );
            }
            PreviewImageViewportMessage::ResetRequested => {
                tracing::debug!(target: "app_ui::preview", "[viewport] reset requested");
                viewport.reset();
            }
        }
        Task::none()
    }
}
