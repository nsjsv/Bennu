//! mac 式滚动条视觉层：透明的原生 `Scrollable` 命中层 + canvas 浮层拇指、
//! hover 帧驱动展开动画与布局探针。本模块不依赖应用的 Message/区域模型：
//! 拇指颜色经 [`crate::ui_colors`] 读活动主题语义色，探针回信经 `map`
//! 闭包交回调用方构造自己的消息。

use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Operation, Outcome, Scrollable as ScrollableOperation};
use iced::time::{Duration as IcedDuration, Instant as IcedInstant};
use iced::widget::{canvas, container, Stack};
use iced::{
    alignment::{Horizontal, Vertical},
    mouse, Element, Length, Point, Rectangle, Size, Theme, Vector,
};

use crate::styles::{ScrollbarVisibility, SCROLLBAR_HOVER_WIDTH};
use crate::ui_colors;

/// 滑块最短长度：内容极长时滑块也不得小于该值，实际长度仍受轨道钳制。
pub const SCROLLBAR_MIN_THUMB_LENGTH: f32 = 28.0;

/// hover 展开缓动时长；与主软件滚动条 reveal/hide 状态机无耦合。
const SCROLLBAR_HOVER_DURATION: IcedDuration = IcedDuration::from_millis(140);
/// hover 展开动画的帧推进步长：与主软件 60Hz 帧时钟同频。
const SCROLLBAR_HOVER_FRAME_INTERVAL: IcedDuration = IcedDuration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

/// `Scrollable` 视口快照：滚动偏移与视口/内容尺寸，供 thumb 几何与
/// 溢出判定使用。每个滚动区域只保留最近一次回调快照。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarViewport {
    pub offset_x: f32,
    pub offset_y: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub content_width: f32,
    pub content_height: f32,
}

pub fn enhanced_scrollbar<'a, Message: 'static>(
    scrollable: impl Into<Element<'a, Message>>,
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    axis: ScrollbarAxis,
    base_width: f32,
) -> Element<'a, Message> {
    let max_width = base_width.max(SCROLLBAR_HOVER_WIDTH);
    let base: Element<'a, Message> = scrollable.into();
    let overlay = canvas::Canvas::new(ScrollbarOverlay {
        visibility,
        viewport,
        axis,
        base_width,
    });
    let overlay: Element<'a, Message> = match axis {
        ScrollbarAxis::Vertical => {
            container(overlay.width(Length::Fixed(max_width)).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Right)
                .into()
        }
        ScrollbarAxis::Horizontal => {
            container(overlay.width(Length::Fill).height(Length::Fixed(max_width)))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(Vertical::Bottom)
                .into()
        }
    };

    // 视觉层不参与命中测试，避免覆盖 Scrollable 原生的轨道点击和拖动区域。
    // Stack 必须跟随 base 高度：Fill 会在无界浮层（任务面板、历史弹窗）里把宿主撑满整窗。
    Stack::with_children([base, overlay])
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
}

/// 双向（竖向 + 横向）增强滚动条：两个视觉 overlay 叠在 Scrollable 上。
pub fn enhanced_scrollbar_both<'a, Message: 'static>(
    scrollable: impl Into<Element<'a, Message>>,
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    base_width: f32,
) -> Element<'a, Message> {
    let max_width = base_width.max(SCROLLBAR_HOVER_WIDTH);
    let base: Element<'a, Message> = scrollable.into();
    let overlay = |axis| {
        canvas::Canvas::new(ScrollbarOverlay {
            visibility,
            viewport,
            axis,
            base_width,
        })
    };
    let vertical_overlay: Element<'a, Message> = container(
        overlay(ScrollbarAxis::Vertical)
            .width(Length::Fixed(max_width))
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Horizontal::Right)
    .into();
    let horizontal_overlay: Element<'a, Message> = container(
        overlay(ScrollbarAxis::Horizontal)
            .width(Length::Fill)
            .height(Length::Fixed(max_width)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Vertical::Bottom)
    .into();

    // 同上：跟随 base 高度，避免在无界浮层中撑满整窗。
    Stack::with_children([base, vertical_overlay, horizontal_overlay])
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
}

#[derive(Debug)]
struct ScrollbarOverlay {
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    axis: ScrollbarAxis,
    base_width: f32,
}

#[derive(Debug, Default)]
struct ScrollbarOverlayState {
    hovered: bool,
    pressed: bool,
    expansion: f32,
    animation_started_at: Option<IcedInstant>,
    animation_initial: f32,
}

impl<Message> canvas::Program<Message> for ScrollbarOverlay {
    type State = ScrollbarOverlayState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let interactive = self.visibility.opacity() > f32::EPSILON
            && self
                .viewport
                .is_some_and(|viewport| scrollbar_has_overflow(self.axis, viewport));
        let cursor_over_track = cursor.is_over(bounds);
        let hovered = interactive && cursor_over_track;
        let button_pressed = matches!(
            event,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        );
        let button_released = matches!(
            event,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        );
        if button_pressed && interactive && cursor_over_track {
            state.pressed = true;
        } else if button_released {
            state.pressed = false;
        }
        let now = IcedInstant::now();

        let expanded = hovered || state.pressed;
        if state.hovered != expanded {
            state.hovered = expanded;
            state.animation_initial = state.expansion;
            state.animation_started_at = Some(now);
        }

        let Some(started_at) = state.animation_started_at else {
            return None;
        };

        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / SCROLLBAR_HOVER_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        let target = if state.hovered { 1.0 } else { 0.0 };
        state.expansion =
            state.animation_initial + (target - state.animation_initial) * ease_out_cubic(progress);

        if progress >= 1.0 {
            state.expansion = target;
            state.animation_started_at = None;
            None
        } else {
            Some(canvas::Action::request_redraw_at(
                now + SCROLLBAR_HOVER_FRAME_INTERVAL,
            ))
        }
    }

    // 底层 Scrollable 必须继续接收鼠标事件，Canvas 只承担动画视觉。
    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        mouse::Interaction::default()
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let Some(viewport) = self.viewport else {
            return vec![frame.into_geometry()];
        };
        if self.visibility.opacity() <= f32::EPSILON {
            return vec![frame.into_geometry()];
        }

        let Some(thumb_bounds) = scrollbar_thumb_bounds(
            self.axis,
            viewport,
            bounds.size(),
            self.base_width,
            state.expansion,
        ) else {
            return vec![frame.into_geometry()];
        };

        let radius = (thumb_bounds.width.min(thumb_bounds.height) / 2.0).into();
        let thumb = canvas::Path::rounded_rectangle(
            Point::new(thumb_bounds.x, thumb_bounds.y),
            thumb_bounds.size(),
            radius,
        );
        let opacity = (self.visibility.opacity() + if state.hovered { 0.18 } else { 0.0 }).min(1.0);
        let color = iced::Color {
            a: 0.42 * opacity,
            ..ui_colors(theme).on_surface
        };
        frame.fill(&thumb, color);

        vec![frame.into_geometry()]
    }
}

// 与主软件动画模块同式的 ease-out-cubic：视觉层自包含，避免为这一行
// 纯函数把 app-ui 的动画模块整体拖进共享层。
fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

pub fn scrollbar_has_overflow(axis: ScrollbarAxis, viewport: ScrollbarViewport) -> bool {
    match axis {
        ScrollbarAxis::Vertical => viewport.content_height > viewport.viewport_height,
        ScrollbarAxis::Horizontal => viewport.content_width > viewport.viewport_width,
    }
}

// 显示门槛看任一轴：逐轴 thumb 计算对无溢出轴本就返回 None，不会多画。
pub fn scrollbar_viewport_has_overflow(viewport: ScrollbarViewport) -> bool {
    viewport.content_height > viewport.viewport_height
        || viewport.content_width > viewport.viewport_width
}

// 只读不改：按目标 Id 定位滚动区，把当帧布局经 `map` 交回调用方。
// 溢出裁决与缓存自愈留在调用方状态机，本层不预设显示意图。
struct ScrollbarLayoutProbe<V> {
    target: advanced_widget::Id,
    viewport: Option<ScrollbarViewport>,
    map: Box<dyn Fn(ScrollbarViewport) -> V + Send>,
}

impl<V> Operation<V> for ScrollbarLayoutProbe<V> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<V>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&advanced_widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn ScrollableOperation,
    ) {
        if id == Some(&self.target) {
            self.viewport = Some(ScrollbarViewport {
                offset_x: translation.x.max(0.0),
                offset_y: translation.y.max(0.0),
                viewport_width: bounds.width,
                viewport_height: bounds.height,
                content_width: content_bounds.width,
                content_height: content_bounds.height,
            });
        }
    }

    fn finish(&self) -> Outcome<V> {
        match self.viewport {
            Some(viewport) => Outcome::Some((self.map)(viewport)),
            None => Outcome::None,
        }
    }
}

pub fn scrollbar_layout_probe<V>(
    target: advanced_widget::Id,
    map: impl Fn(ScrollbarViewport) -> V + Send + 'static,
) -> impl Operation<V> {
    ScrollbarLayoutProbe {
        target,
        viewport: None,
        map: Box::new(map),
    }
}

pub fn scrollbar_thumb_bounds(
    axis: ScrollbarAxis,
    viewport: ScrollbarViewport,
    track_size: Size,
    base_width: f32,
    expansion: f32,
) -> Option<Rectangle> {
    let (offset, viewport_extent, content_extent, track_length) = match axis {
        ScrollbarAxis::Vertical => (
            viewport.offset_y,
            viewport.viewport_height,
            viewport.content_height,
            track_size.height,
        ),
        ScrollbarAxis::Horizontal => (
            viewport.offset_x,
            viewport.viewport_width,
            viewport.content_width,
            track_size.width,
        ),
    };
    if !offset.is_finite()
        || !viewport_extent.is_finite()
        || !content_extent.is_finite()
        || !track_length.is_finite()
        || viewport_extent <= 0.0
        || content_extent <= viewport_extent
        || track_length <= 0.0
    {
        return None;
    }

    let ratio = (viewport_extent / content_extent).clamp(0.0, 1.0);
    let thumb_length = (track_length * ratio)
        .max(SCROLLBAR_MIN_THUMB_LENGTH.min(track_length))
        .min(track_length);
    let maximum_offset = (content_extent - viewport_extent).max(0.0);
    let scroll_progress = if maximum_offset > f32::EPSILON {
        (offset.clamp(0.0, maximum_offset) / maximum_offset).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb_offset = scroll_progress * (track_length - thumb_length).max(0.0);
    let thickness =
        base_width + (SCROLLBAR_HOVER_WIDTH - base_width).max(0.0) * expansion.clamp(0.0, 1.0);

    Some(match axis {
        ScrollbarAxis::Vertical => Rectangle {
            x: ((track_size.width - thickness) / 2.0).max(0.0),
            y: thumb_offset,
            width: thickness.min(track_size.width),
            height: thumb_length,
        },
        ScrollbarAxis::Horizontal => Rectangle {
            x: thumb_offset,
            y: ((track_size.height - thickness) / 2.0).max(0.0),
            width: thumb_length,
            height: thickness.min(track_size.height),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::scrollable;

    #[test]
    fn vertical_thumb_keeps_minimum_length_and_maps_scroll_ends() {
        let track_size = Size::new(8.0, 600.0);
        let content_height = 600_000.0;
        let viewport_height = 600.0;
        let top_viewport = ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 8.0,
            viewport_height,
            content_width: 8.0,
            content_height,
        };
        let bottom_viewport = ScrollbarViewport {
            offset_y: content_height - viewport_height,
            ..top_viewport
        };

        let top =
            scrollbar_thumb_bounds(ScrollbarAxis::Vertical, top_viewport, track_size, 8.0, 0.0)
                .expect("overflowing content must produce a thumb");
        let bottom = scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            bottom_viewport,
            track_size,
            8.0,
            0.0,
        )
        .expect("overflowing content must produce a thumb");

        assert_eq!(top.height, SCROLLBAR_MIN_THUMB_LENGTH);
        assert_eq!(top.y, 0.0);
        assert!((bottom.y + bottom.height - track_size.height).abs() <= f32::EPSILON);
    }

    #[test]
    fn horizontal_thumb_uses_bounded_minimum_length() {
        let viewport = ScrollbarViewport {
            offset_x: 2_000.0,
            offset_y: 0.0,
            viewport_width: 800.0,
            viewport_height: 8.0,
            content_width: 800_000.0,
            content_height: 8.0,
        };

        let thumb = scrollbar_thumb_bounds(
            ScrollbarAxis::Horizontal,
            viewport,
            Size::new(800.0, 8.0),
            8.0,
            0.0,
        )
        .expect("overflowing content must produce a thumb");

        assert_eq!(thumb.width, SCROLLBAR_MIN_THUMB_LENGTH);
        assert!(thumb.x > 0.0);
        assert!(thumb.x + thumb.width < 800.0);
    }

    #[test]
    fn thumb_is_not_drawn_without_overflow() {
        let viewport = ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
            content_width: 800.0,
            content_height: 600.0,
        };

        assert!(scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            viewport,
            Size::new(8.0, 600.0),
            8.0,
            0.0,
        )
        .is_none());
        assert!(scrollbar_thumb_bounds(
            ScrollbarAxis::Horizontal,
            viewport,
            Size::new(800.0, 8.0),
            8.0,
            0.0,
        )
        .is_none());
    }

    #[test]
    fn hover_expansion_changes_only_thumb_thickness() {
        let viewport = ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 300.0,
            viewport_width: 8.0,
            viewport_height: 600.0,
            content_width: 8.0,
            content_height: 6_000.0,
        };
        let narrow = scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            viewport,
            Size::new(14.0, 600.0),
            8.0,
            0.0,
        )
        .expect("overflowing content must produce a thumb");
        let expanded = scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            viewport,
            Size::new(14.0, 600.0),
            8.0,
            1.0,
        )
        .expect("overflowing content must produce a thumb");

        assert_eq!(narrow.height, expanded.height);
        assert_eq!(narrow.width, 8.0);
        assert_eq!(narrow.x, 3.0);
        assert_eq!(expanded.width, SCROLLBAR_HOVER_WIDTH);
        assert_eq!(expanded.x, 0.0);
    }

    fn viewport_for(content_height: f32, viewport_height: f32) -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 800.0,
            viewport_height,
            content_width: 800.0,
            content_height,
        }
    }

    #[test]
    fn probe_without_target_produces_no_reply() {
        let probe =
            scrollbar_layout_probe(iced::advanced::widget::Id::new("no-such-scroll"), |_| ());

        assert!(matches!(probe.finish(), Outcome::None));
    }

    #[test]
    fn probe_with_target_replies_with_mapped_viewport() {
        let target = iced::advanced::widget::Id::new("any");
        let mut probe = ScrollbarLayoutProbe {
            target: target.clone(),
            viewport: Some(viewport_for(900.0, 600.0)),
            map: Box::new(std::convert::identity),
        };
        probe.scrollable(
            Some(&target),
            Rectangle::new(Point::ORIGIN, Size::new(800.0, 600.0)),
            Rectangle::new(Point::ORIGIN, Size::new(800.0, 900.0)),
            Vector::new(0.0, 150.0),
            &mut UnreachableScrollableState,
        );

        match probe.finish() {
            // scrollable 回调应把当帧布局（含 translation 的非负偏移）写入快照。
            Outcome::Some(viewport) => assert_eq!(
                viewport,
                ScrollbarViewport {
                    offset_x: 0.0,
                    offset_y: 150.0,
                    viewport_width: 800.0,
                    viewport_height: 600.0,
                    content_width: 800.0,
                    content_height: 900.0,
                }
            ),
            _ => panic!("probe must reply with the verified viewport"),
        }
    }

    struct UnreachableScrollableState;

    impl ScrollableOperation for UnreachableScrollableState {
        fn snap_to(&mut self, _offset: scrollable::RelativeOffset<Option<f32>>) {}

        fn scroll_to(&mut self, _offset: scrollable::AbsoluteOffset<Option<f32>>) {}

        fn scroll_by(
            &mut self,
            _offset: scrollable::AbsoluteOffset,
            _bounds: Rectangle,
            _content_bounds: Rectangle,
        ) {
        }
    }
}
