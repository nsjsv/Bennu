//! 图片预览缩放/平移交互：呈现面视口尺寸由宿主传入（预览窗口/右侧面板两种表面的几何归宿主）。
//! 放大到超过已解出的像素（先按屏幕大小解）时，在后台解一次原图换上。

use file_core::image_decode::DecodeBound;
use iced::Size;
use iced::Task;

use crate::commands::preview::original_image_preview_command;
use crate::image_preview_viewport::{
    image_preview_zoom_multiplier, ImagePreviewViewport, PreviewImageViewportMessage,
    IMAGE_PREVIEW_ZOOM_STEP,
};
use crate::preview::{
    image_preview_size, ImagePreviewContent, PreviewContent, PreviewSize, PreviewState,
};
use crate::preview_message::PreviewMessage;

/// 显示出来的像素比已解出的多这么多以内不换原图：浮点误差，不值得再解一次。
const RESOLUTION_UPGRADE_SLACK: f32 = 1.02;

impl super::PreviewEngine {
    /// 图片预览的缩放/平移交互。仅在静态图片预览显示时生效：
    /// GIF/视频等面板没有接入 mouse_area，收到迟到消息也直接忽略。
    pub fn update_preview_image_viewport(
        &mut self,
        message: PreviewImageViewportMessage,
        panel_size: PreviewSize,
    ) -> Task<PreviewMessage> {
        let Some(PreviewState::Ready(PreviewContent::Image(content))) = &self.preview else {
            return Task::none();
        };
        let (image_width, image_height) = content.dimensions();
        // 缩略图占位时尺寸是缩略图的，不知道 1:1 在哪。
        let source_width = match content {
            ImagePreviewContent::Thumbnail { .. } => None,
            _ => Some(image_width),
        };

        // 面板/独立窗口两种表面的媒体区尺寸分派，与视图渲染基准同源。
        let panel = Size::new(panel_size.width, panel_size.height);
        let (fit_width, fit_height) = image_preview_size(panel_size, image_width, image_height);
        let fit = Size::new(fit_width, fit_height);
        let display_scale = self.display_scale();
        let viewport = &mut self.preview_image_viewport;
        let actual = source_width
            .map(|width| ImagePreviewViewport::actual_size_scale(width, fit, display_scale));
        let at_actual =
            actual.is_some_and(|actual| (viewport.scale - actual).abs() <= actual * 1e-3);
        // 缩放范围只为像素图按 1:1 放宽：SVG 放大是按显示大小把整张图重画一遍，放到原始
        // 像素的 16 倍会画出几十亿像素，所以 SVG 照旧在适应窗口的 0.2–8 倍之间。
        let range = match content {
            ImagePreviewContent::OriginalRaster { .. } => actual,
            _ => None,
        };
        let zoom = |viewport: &mut ImagePreviewViewport, multiplier, anchor| {
            viewport.apply_zoom(multiplier, anchor, panel, fit, range);
        };
        match message {
            PreviewImageViewportMessage::PointerMoved(position) => {
                viewport.apply_pointer_motion(position, panel, fit);
            }
            PreviewImageViewportMessage::PointerLeft => viewport.pointer = None,
            PreviewImageViewportMessage::PanStarted => viewport.panning = true,
            PreviewImageViewportMessage::PanEnded => viewport.panning = false,
            PreviewImageViewportMessage::Zoomed(delta) => {
                let anchor = viewport.pointer;
                zoom(viewport, image_preview_zoom_multiplier(delta), anchor);
            }
            PreviewImageViewportMessage::DoubleClicked => match actual {
                _ if viewport.is_zoomed() => viewport.reset(),
                Some(actual) => {
                    let anchor = viewport.pointer;
                    zoom(viewport, actual / viewport.scale, anchor);
                }
                None => {}
            },
            PreviewImageViewportMessage::ZoomedIn => zoom(viewport, IMAGE_PREVIEW_ZOOM_STEP, None),
            PreviewImageViewportMessage::ZoomedOut => {
                zoom(viewport, 1.0 / IMAGE_PREVIEW_ZOOM_STEP, None);
            }
            PreviewImageViewportMessage::FitRequested => viewport.reset(),
            PreviewImageViewportMessage::ActualSizeRequested => {
                if let Some(actual) = actual {
                    zoom(viewport, actual / viewport.scale, None);
                }
            }
            PreviewImageViewportMessage::FitOrActualToggled => match actual {
                Some(_) if at_actual => viewport.reset(),
                Some(actual) => zoom(viewport, actual / viewport.scale, None),
                None => {}
            },
        }
        self.load_full_image_when_magnified(fit)
    }

    /// 屏幕量好了：记下物理像素（下一张图按它解）和缩放倍数（1:1、文档渲染）。
    pub(crate) fn accept_preview_display(
        &mut self,
        display: Option<crate::preview::DisplayMetrics>,
    ) -> Task<PreviewMessage> {
        if display.is_some() {
            self.display = display;
        }
        Task::none()
    }

    /// 逻辑像素换物理像素的倍数；还没量到按 1。
    pub fn display_scale(&self) -> f32 {
        self.display.map_or(1.0, |display| display.scale_factor)
    }

    /// 原图按屏幕大小解：再大窗口也放不下更多像素，放大时再解原图。
    pub fn image_decode_bound(&self) -> DecodeBound {
        let display = self
            .display
            .map_or(crate::video_preview::FALLBACK_DISPLAY, |display| {
                display.pixels
            });
        DecodeBound::within(display.width, display.height)
    }

    fn load_full_image_when_magnified(&mut self, fit: Size) -> Task<PreviewMessage> {
        let Some(PreviewState::Ready(PreviewContent::Image(ImagePreviewContent::OriginalRaster {
            path,
            raster_handle,
            width,
            pixel_width,
            ..
        }))) = &self.preview
        else {
            return Task::none();
        };
        let viewport = &self.preview_image_viewport;
        let shown_pixels = fit.width * viewport.scale * self.display_scale();
        if self.full_image_requested
            || pixel_width >= width
            || shown_pixels <= *pixel_width as f32 * RESOLUTION_UPGRADE_SLACK
        {
            return Task::none();
        }
        self.full_image_requested = true;
        // 大小已在打开时查过，这里不再限；同一代际，换图或关掉时一起作废。
        original_image_preview_command(
            path.clone(),
            self.original_image_preview_generation,
            0,
            DecodeBound::FULL,
            Some(raster_handle.clone()),
            self.original_image_preview_cancel
                .clone()
                .unwrap_or_default(),
        )
    }
}

#[cfg(test)]
mod tests;
