//! 拖拽悬停目录自动打开(spring-loaded)的进度环。
//!
//! 只做视觉:圆环围绕条目图标槽,进度弧从顶部顺时针生长,转满即到
//! 打开阈值。计时与触发在 `app::selection::spring_open`,环本体不承担
//! 状态,每次 view 重建按当前进度重画。

use std::f32::consts::FRAC_PI_2;
use std::f32::consts::TAU;

use iced::widget::canvas;
use iced::{Element, Length, Point, Rectangle, Theme};

use crate::matugen_theme::ui_colors;
use crate::model::Message;

const RING_STROKE_WIDTH: f32 = 2.0;
/// 进度弧用折线逼近:整圆 64 段,3 秒内视觉连续。
const RING_ARC_SEGMENTS: usize = 64;

pub(crate) fn file_drag_spring_ring(progress: f32) -> Element<'static, Message> {
    canvas(FileDragSpringRingOverlay { progress })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

struct FileDragSpringRingOverlay {
    progress: f32,
}

impl canvas::Program<Message> for FileDragSpringRingOverlay {
    type State = ();

    // 纯视觉叠层:不拦截事件,不改变指针交互,让下层条目照常响应。
    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        iced::mouse::Interaction::default()
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let colors = ui_colors(theme);
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) / 2.0 - RING_STROKE_WIDTH).max(1.0);

        let track = canvas::Path::circle(center, radius);
        frame.stroke(
            &track,
            canvas::Stroke::default()
                .with_color(with_alpha(colors.outline_variant, 0.55))
                .with_width(RING_STROKE_WIDTH),
        );

        let progress = self.progress.clamp(0.0, 1.0);
        if progress > f32::EPSILON {
            frame.stroke(
                &arc_path(center, radius, progress),
                canvas::Stroke::default()
                    .with_color(colors.primary)
                    .with_width(RING_STROKE_WIDTH),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// 进度弧:顶部起点(12 点钟)顺时针生长。
fn arc_path(center: Point, radius: f32, progress: f32) -> canvas::Path {
    let start_angle = -FRAC_PI_2;
    let sweep = progress * TAU;
    let steps = ((RING_ARC_SEGMENTS as f32 * progress).ceil() as usize).max(1);
    canvas::Path::new(|builder| {
        builder.move_to(point_on_circle(center, radius, start_angle));
        for step in 1..=steps {
            let angle = start_angle + sweep * (step as f32 / steps as f32);
            builder.line_to(point_on_circle(center, radius, angle));
        }
    })
}

fn point_on_circle(center: Point, radius: f32, angle: f32) -> Point {
    Point::new(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}

fn with_alpha(color: iced::Color, alpha: f32) -> iced::Color {
    iced::Color { a: alpha, ..color }
}
