use iced::{mouse, Point, Size};

/// 图片预览的缩放/平移视口。scale 相对"适应窗口"尺寸（1.0 = fit），
/// offset 是图片中心相对媒体区中心的偏移（媒体区坐标）。
/// 所有权在 FileBrowser：open_preview_for_resolved_path 开启新会话时重置，
/// 同一会话内缩略图→原图的内容替换保留；pointer 缓存供滚轮缩放取锚点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ImagePreviewViewport {
    pub(crate) scale: f32,
    pub(crate) offset: Point,
    pub(crate) pointer: Option<Point>,
    pub(crate) panning: bool,
}

impl Default for ImagePreviewViewport {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: Point::ORIGIN,
            pointer: None,
            panning: false,
        }
    }
}

impl ImagePreviewViewport {
    pub(crate) fn is_zoomed(&self) -> bool {
        self.scale != 1.0 || self.offset != Point::ORIGIN
    }

    /// 缩放：scale > 1 时以锚点（指针位置）为中心，缩放前后锚点下的图像
    /// 内容不动；锚点缺失时退化为围绕媒体区中心缩放。
    /// scale 缩到不大于 1 后图片四周已有空隙，锚点保持只会把图片推向
    /// 窗口边缘方向，因此一律归零 offset 围绕中心居中缩小。
    pub(crate) fn apply_zoom(&mut self, multiplier: f32, anchor: Option<Point>, panel: Size, fit: Size) {
        let panel_center = Point::new(panel.width / 2.0, panel.height / 2.0);
        let next_scale =
            (self.scale * multiplier).clamp(IMAGE_PREVIEW_MIN_SCALE, IMAGE_PREVIEW_MAX_SCALE);
        if next_scale <= 1.0 {
            self.offset = Point::ORIGIN;
            self.scale = next_scale;
            return;
        }
        let anchor = anchor.unwrap_or(panel_center);
        let anchor_relative_x = anchor.x - panel_center.x - self.offset.x;
        let anchor_relative_y = anchor.y - panel_center.y - self.offset.y;
        self.offset = Point::new(
            anchor.x - panel_center.x - anchor_relative_x * next_scale / self.scale,
            anchor.y - panel_center.y - anchor_relative_y * next_scale / self.scale,
        );
        self.scale = next_scale;
        self.clamp_offset(panel, fit);
    }

    pub(crate) fn apply_pointer_motion(&mut self, position: Point, panel: Size, fit: Size) {
        if self.panning {
            if let Some(previous) = self.pointer {
                self.offset = Point::new(
                    self.offset.x + position.x - previous.x,
                    self.offset.y + position.y - previous.y,
                );
                self.clamp_offset(panel, fit);
            }
        }
        self.pointer = Some(position);
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// 标准钳制：某轴缩放后图片大于媒体区时，图片边缘最多拖到与媒体区
    /// 边缘对齐（不露背景）；不大于媒体区的轴固定居中。缩放与平移共用
    /// 这一个不变量，触边时锚点缩放的"锚点下内容不动"让位于钳制。
    fn clamp_offset(&mut self, panel: Size, fit: Size) {
        self.offset = Point::new(
            clamp_axis(self.offset.x, fit.width * self.scale, panel.width),
            clamp_axis(self.offset.y, fit.height * self.scale, panel.height),
        );
    }
}

fn clamp_axis(offset: f32, scaled: f32, panel: f32) -> f32 {
    let slack = (scaled - panel) / 2.0;
    if slack <= 0.0 {
        0.0
    } else {
        offset.clamp(-slack, slack)
    }
}

/// 滚轮增量 → 缩放倍率；行增量按每行 10%，像素增量按 40px 一行折算。
pub(crate) fn image_preview_zoom_multiplier(delta: mouse::ScrollDelta) -> f32 {
    let lines = match delta {
        mouse::ScrollDelta::Lines { y, .. } => y,
        mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
    };
    1.1_f32.powf(lines)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PreviewImageViewportMessage {
    PointerMoved(Point),
    PanStarted,
    PanEnded,
    Zoomed(mouse::ScrollDelta),
    ResetRequested,
}

const IMAGE_PREVIEW_MIN_SCALE: f32 = 0.2;
const IMAGE_PREVIEW_MAX_SCALE: f32 = 8.0;

#[cfg(test)]
mod image_preview_viewport_tests {
    use super::*;

    const PANEL: Size = Size::new(800.0, 600.0);
    const FIT: Size = Size::new(800.0, 600.0);
    const PANEL_CENTER: Point = Point::new(400.0, 300.0);

    #[test]
    fn zoom_keeps_the_anchored_image_point_under_the_cursor() {
        let mut viewport = ImagePreviewViewport::default();
        let anchor = Point::new(200.0, 150.0);

        viewport.apply_zoom(2.0, Some(anchor), PANEL, FIT);

        assert_eq!(viewport.scale, 2.0);
        // 缩放前锚点在中心左侧 200px；缩放后该图像点必须仍在锚点处。
        let before = (anchor.x - PANEL_CENTER.x, anchor.y - PANEL_CENTER.y);
        let after = (
            anchor.x - PANEL_CENTER.x - viewport.offset.x,
            anchor.y - PANEL_CENTER.y - viewport.offset.y,
        );
        assert_eq!(after, (before.0 * 2.0, before.1 * 2.0));
    }

    #[test]
    fn zoom_is_clamped_to_the_configured_range() {
        let mut viewport = ImagePreviewViewport::default();
        viewport.apply_zoom(1_000.0, None, PANEL, FIT);
        assert_eq!(viewport.scale, IMAGE_PREVIEW_MAX_SCALE);

        viewport.apply_zoom(0.0, None, PANEL, FIT);
        assert_eq!(viewport.scale, IMAGE_PREVIEW_MIN_SCALE);
    }

    #[test]
    fn pan_accumulates_only_while_panning() {
        let mut viewport = ImagePreviewViewport::default();
        viewport.apply_zoom(2.0, None, PANEL, FIT);
        viewport.apply_pointer_motion(Point::new(10.0, 10.0), PANEL, FIT);

        viewport.panning = true;
        viewport.apply_pointer_motion(Point::new(25.0, -5.0), PANEL, FIT);
        assert_eq!(viewport.offset, Point::new(15.0, -15.0));

        viewport.panning = false;
        viewport.apply_pointer_motion(Point::new(100.0, 100.0), PANEL, FIT);
        assert_eq!(viewport.offset, Point::new(15.0, -15.0));
    }

    #[test]
    fn panning_at_fit_scale_stays_centered() {
        // fit 尺寸每轴都不大于面板，无余量可拖，必须保持居中。
        let mut viewport = ImagePreviewViewport::default();
        viewport.panning = true;

        viewport.apply_pointer_motion(Point::new(30.0, 40.0), PANEL, FIT);
        viewport.apply_pointer_motion(Point::new(500.0, 500.0), PANEL, FIT);

        assert_eq!(viewport.offset, Point::ORIGIN);
    }

    #[test]
    fn reset_restores_the_fit_viewport() {
        let mut viewport = ImagePreviewViewport::default();
        viewport.apply_zoom(2.0, None, PANEL, FIT);
        viewport.panning = true;

        viewport.reset();

        assert_eq!(viewport, ImagePreviewViewport::default());
        assert!(!viewport.is_zoomed());
    }

    #[test]
    fn wheel_delta_converts_to_a_multiplicative_zoom_step() {
        assert_eq!(
            image_preview_zoom_multiplier(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
            1.1
        );
        assert_eq!(
            image_preview_zoom_multiplier(mouse::ScrollDelta::Pixels { x: 0.0, y: 40.0 }),
            1.1
        );
    }

    #[test]
    fn zooming_out_below_fit_stays_centered_instead_of_chasing_the_anchor() {
        let mut viewport = ImagePreviewViewport::default();

        // 从适应窗口连续缩小，锚点取在窗口边缘：图片必须原地居中缩小。
        viewport.apply_zoom(0.9, Some(Point::new(20.0, 20.0)), PANEL, FIT);
        assert_eq!(viewport.scale, 0.9);
        assert_eq!(viewport.offset, Point::ORIGIN);

        viewport.apply_zoom(0.9, Some(Point::new(20.0, 20.0)), PANEL, FIT);
        assert_eq!(viewport.offset, Point::ORIGIN);
    }

    #[test]
    fn zooming_back_below_fit_clears_pan_offset_left_by_zooming_in() {
        let mut viewport = ImagePreviewViewport::default();
        let anchor = Point::new(100.0, 500.0);
        viewport.apply_zoom(4.0, Some(anchor), PANEL, FIT);
        viewport.apply_pointer_motion(Point::new(120.0, 480.0), PANEL, FIT);
        viewport.panning = true;
        viewport.apply_pointer_motion(Point::new(300.0, 300.0), PANEL, FIT);
        viewport.panning = false;
        assert_ne!(viewport.offset, Point::ORIGIN);

        viewport.apply_zoom(0.2, Some(anchor), PANEL, FIT);

        assert_eq!(viewport.scale, 0.8);
        assert_eq!(viewport.offset, Point::ORIGIN);
    }

    #[test]
    fn panning_beyond_the_scaled_edges_stops_at_alignment() {
        let mut viewport = ImagePreviewViewport::default();
        viewport.apply_zoom(4.0, None, PANEL, FIT);
        viewport.apply_pointer_motion(Point::new(400.0, 300.0), PANEL, FIT);
        viewport.panning = true;

        // scale 4 后图片 3200x2400，两轴各留 (1200, 900) 的可拖余量。
        viewport.apply_pointer_motion(Point::new(10_000.0, 10_000.0), PANEL, FIT);
        assert_eq!(viewport.offset, Point::new(1200.0, 900.0));

        viewport.apply_pointer_motion(Point::new(-10_000.0, -10_000.0), PANEL, FIT);
        assert_eq!(viewport.offset, Point::new(-1200.0, -900.0));
    }

    #[test]
    fn axis_smaller_than_the_panel_stays_centered_while_panning() {
        // 竖长图：fit 宽 400 高 600，放大 2 倍后 x 轴恰好等于面板，必须锁中。
        let fit = Size::new(400.0, 600.0);
        let mut viewport = ImagePreviewViewport::default();
        viewport.apply_zoom(2.0, None, PANEL, fit);
        viewport.apply_pointer_motion(Point::new(400.0, 300.0), PANEL, fit);
        viewport.panning = true;

        viewport.apply_pointer_motion(Point::new(5_000.0, 5_000.0), PANEL, fit);

        assert_eq!(viewport.offset, Point::new(0.0, 300.0));
    }

    #[test]
    fn zooming_past_the_panel_edges_clamps_instead_of_exposing_background() {
        // 竖长图 fit 400x600 放大 4 倍：x 轴可拖余量只有 (1600-800)/2=400。
        // 以左边缘中点为锚点缩放，锚点数学想把 x offset 推到 1200，
        // 必须钳停在边缘对齐处；锚点在 y 中线上，y 保持居中。
        let fit = Size::new(400.0, 600.0);
        let mut viewport = ImagePreviewViewport::default();
        let anchor = Point::new(0.0, 300.0);

        viewport.apply_zoom(4.0, Some(anchor), PANEL, fit);

        assert_eq!(viewport.offset, Point::new(400.0, 0.0));
    }
}
