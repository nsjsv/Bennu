//! 常驻细线滚动条：透明的原生 `Scrollable` 负责命中、拖动和偏移，本模块的
//! 包装 widget 每次 update 都从子 Scrollable 读当帧布局，再在其上画细线滑块；
//! 指针进入命中区或按住拖动时滑块展开。滑块几何只来自当帧布局，宿主不保存
//! 任何绘制用的视口缓存，内容骤变后也就不存在需要补刷的过期滑块。

use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::operation::{Operation, Outcome, Scrollable as ScrollableOperation};
use iced::advanced::widget::{self as advanced_widget, tree, Tree};
use iced::advanced::{layout, overlay, Clipboard, Layout, Shell, Widget};
use iced::time::{Duration, Instant};
use iced::{mouse, window, Border, Element, Event, Length, Rectangle, Size, Vector};

use crate::styles::SCROLLBAR_HIT_WIDTH;
use crate::ui_colors;

/// 滑块最短长度：内容极长时滑块也不得小于该值，实际长度仍受轨道钳制。
pub const SCROLLBAR_MIN_THUMB_LENGTH: f32 = 28.0;

/// 平时的细线厚度：常驻显示，必须细到不干扰阅读内容边缘。
const SCROLLBAR_THIN_THICKNESS: f32 = 3.0;
/// 指针进入命中区或按住拖动时的厚度：给拖动一个看得清的目标。
const SCROLLBAR_EXPANDED_THICKNESS: f32 = 8.0;
/// 滑块离命中区外缘的距离：细线贴边，但不压住面板圆角和窗口边框。
const SCROLLBAR_EDGE_INSET: f32 = 3.0;
/// 细线与展开态的不透明度：细线常驻所以更淡，展开表示“可以拖”所以更实。
const SCROLLBAR_THIN_ALPHA: f32 = 0.35;
const SCROLLBAR_EXPANDED_ALPHA: f32 = 0.6;

/// 展开/收回缓动时长与帧推进步长（与主软件 60Hz 帧时钟同频）。
const SCROLLBAR_EXPANSION_DURATION: Duration = Duration::from_millis(140);
const SCROLLBAR_EXPANSION_FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

/// `Scrollable` 视口快照：滚动偏移与视口/内容尺寸。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarViewport {
    pub offset_x: f32,
    pub offset_y: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub content_width: f32,
    pub content_height: f32,
}

/// 单轴细线滚动条。`scrollable` 必须是 Scrollable 本身（或只隔着不含其它
/// Scrollable 的包装层）：滑块取的是子树里第一个 Scrollable 的布局。
pub fn enhanced_scrollbar<'a, Message: 'a>(
    scrollable: impl Into<Element<'a, Message>>,
    axis: ScrollbarAxis,
) -> Element<'a, Message> {
    let axes: &'static [ScrollbarAxis] = match axis {
        ScrollbarAxis::Vertical => &[ScrollbarAxis::Vertical],
        ScrollbarAxis::Horizontal => &[ScrollbarAxis::Horizontal],
    };
    Element::new(ScrollbarFrame {
        content: scrollable.into(),
        axes,
    })
}

/// 双向细线滚动条：竖向与横向滑块各自独立展开。
pub fn enhanced_scrollbar_both<'a, Message: 'a>(
    scrollable: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(ScrollbarFrame {
        content: scrollable.into(),
        axes: &[ScrollbarAxis::Vertical, ScrollbarAxis::Horizontal],
    })
}

struct ScrollbarFrame<'a, Message> {
    content: Element<'a, Message>,
    axes: &'static [ScrollbarAxis],
}

/// 探针读到的子 Scrollable 当帧布局：自身边框（滑块轨道贴着它的右/下缘，
/// 外面再包 container 也能与原生 scroller 对齐）加视口快照。
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProbedScroll {
    bounds: Rectangle,
    viewport: ScrollbarViewport,
}

#[derive(Debug, Default)]
struct ScrollbarFrameState {
    /// 最近一次 update 从子 Scrollable 读到的布局；iced 每次 draw 前都会先以
    /// RedrawRequested 跑一遍 update，所以 draw 时这里就是当帧布局。
    scroll: Option<ProbedScroll>,
    vertical: ThumbExpansion,
    horizontal: ThumbExpansion,
}

impl ScrollbarFrameState {
    fn expansion_mut(&mut self, axis: ScrollbarAxis) -> &mut ThumbExpansion {
        match axis {
            ScrollbarAxis::Vertical => &mut self.vertical,
            ScrollbarAxis::Horizontal => &mut self.horizontal,
        }
    }

    fn expansion(&self, axis: ScrollbarAxis) -> f32 {
        match axis {
            ScrollbarAxis::Vertical => self.vertical.progress,
            ScrollbarAxis::Horizontal => self.horizontal.progress,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ThumbExpansion {
    engaged: bool,
    pressed: bool,
    progress: f32,
    animation_from: f32,
    animation_started_at: Option<Instant>,
}

impl ThumbExpansion {
    /// 推进展开动画；动画未结束时返回下一帧时刻。
    fn advance(&mut self, engaged: bool, now: Instant) -> Option<Instant> {
        if self.engaged != engaged {
            self.engaged = engaged;
            self.animation_from = self.progress;
            self.animation_started_at = Some(now);
        }
        let started_at = self.animation_started_at?;
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / SCROLLBAR_EXPANSION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        let target = if engaged { 1.0 } else { 0.0 };
        if progress >= 1.0 {
            self.progress = target;
            self.animation_started_at = None;
            return None;
        }
        self.progress =
            self.animation_from + (target - self.animation_from) * ease_out_cubic(progress);
        Some(now + SCROLLBAR_EXPANSION_FRAME_INTERVAL)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ScrollbarFrame<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ScrollbarFrameState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ScrollbarFrameState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        let mut probe = FirstScrollableProbe::default();
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, &mut probe);
        let state = tree.state.downcast_mut::<ScrollbarFrameState>();
        if state.scroll != probe.scroll {
            state.scroll = probe.scroll;
            shell.request_redraw();
        }

        let now = Instant::now();
        for &axis in self.axes {
            let over_strip = state
                .scroll
                .filter(|scroll| has_overflow(axis, scroll.viewport))
                .is_some_and(|scroll| cursor.is_over(track_bounds(axis, scroll, self.axes)));
            let expansion = state.expansion_mut(axis);
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over_strip => {
                    expansion.pressed = true;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    expansion.pressed = false;
                }
                _ => {}
            }
            // 按住期间即使指针拖出命中区也保持展开，松开后才收回。
            let engaged = expansion.pressed || over_strip;
            if let Some(next_frame) = expansion.advance(engaged, now) {
                shell.request_redraw_at(window::RedrawRequest::At(next_frame));
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        let state = tree.state.downcast_ref::<ScrollbarFrameState>();
        let Some(scroll) = state.scroll else {
            return;
        };
        let Some(clip) = layout.bounds().intersection(viewport) else {
            return;
        };
        let on_surface = ui_colors(theme).on_surface;
        // 新 layer 保证滑块盖在内容文字之上：同一 layer 内 iced 先画 quad 后画文字。
        renderer.with_layer(clip, |renderer| {
            for &axis in self.axes {
                let track = track_bounds(axis, scroll, self.axes);
                let expansion = state.expansion(axis);
                let Some(thumb) =
                    scrollbar_thumb_bounds(axis, scroll.viewport, track.size(), expansion)
                else {
                    continue;
                };
                let thumb = thumb + Vector::new(track.x, track.y);
                let alpha = SCROLLBAR_THIN_ALPHA
                    + (SCROLLBAR_EXPANDED_ALPHA - SCROLLBAR_THIN_ALPHA) * expansion;
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: thumb,
                        border: Border {
                            radius: (thumb.width.min(thumb.height) / 2.0).into(),
                            ..Border::default()
                        },
                        ..renderer::Quad::default()
                    },
                    iced::Color {
                        a: alpha,
                        ..on_surface
                    },
                );
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

/// 命中区（也是滑块轨道）：贴 Scrollable 右/下边缘、宽 `SCROLLBAR_HIT_WIDTH`。
/// 双向且另一轴也溢出时让出拐角，与 iced 原生 scroller 的轨道长度一致，
/// 画出来的滑块才和实际可拖的 scroller 重合。
fn track_bounds(axis: ScrollbarAxis, scroll: ProbedScroll, axes: &[ScrollbarAxis]) -> Rectangle {
    let bounds = scroll.bounds;
    let other_axis_overflows =
        |other: ScrollbarAxis| axes.contains(&other) && has_overflow(other, scroll.viewport);
    match axis {
        ScrollbarAxis::Vertical => {
            let corner = if other_axis_overflows(ScrollbarAxis::Horizontal) {
                SCROLLBAR_HIT_WIDTH
            } else {
                0.0
            };
            Rectangle {
                x: bounds.x + bounds.width - SCROLLBAR_HIT_WIDTH,
                y: bounds.y,
                width: SCROLLBAR_HIT_WIDTH,
                height: (bounds.height - corner).max(0.0),
            }
        }
        ScrollbarAxis::Horizontal => {
            let corner = if other_axis_overflows(ScrollbarAxis::Vertical) {
                SCROLLBAR_HIT_WIDTH
            } else {
                0.0
            };
            Rectangle {
                x: bounds.x,
                y: bounds.y + bounds.height - SCROLLBAR_HIT_WIDTH,
                width: (bounds.width - corner).max(0.0),
                height: SCROLLBAR_HIT_WIDTH,
            }
        }
    }
}

// 与主软件动画模块同式的 ease-out-cubic：视觉层自包含，避免为这一行
// 纯函数把 app-ui 的动画模块整体拖进共享层。
fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

fn has_overflow(axis: ScrollbarAxis, viewport: ScrollbarViewport) -> bool {
    match axis {
        ScrollbarAxis::Vertical => viewport.content_height > viewport.viewport_height,
        ScrollbarAxis::Horizontal => viewport.content_width > viewport.viewport_width,
    }
}

fn viewport_from_layout(
    bounds: Rectangle,
    content_bounds: Rectangle,
    translation: Vector,
) -> ScrollbarViewport {
    ScrollbarViewport {
        offset_x: translation.x.max(0.0),
        offset_y: translation.y.max(0.0),
        viewport_width: bounds.width,
        viewport_height: bounds.height,
        content_width: content_bounds.width,
        content_height: content_bounds.height,
    }
}

/// 包装 widget 自用的探针：只取子树里第一个 Scrollable，取到后不再下钻，
/// 每个事件都跑也只走到 Scrollable 这一层，不会遍历上千行内容。
#[derive(Default)]
struct FirstScrollableProbe {
    scroll: Option<ProbedScroll>,
}

impl Operation for FirstScrollableProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.scroll.is_none() {
            operate(self);
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&advanced_widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn ScrollableOperation,
    ) {
        if self.scroll.is_none() {
            self.scroll = Some(ProbedScroll {
                bounds,
                viewport: viewport_from_layout(bounds, content_bounds, translation),
            });
        }
    }
}

// 只读不改：按目标 Id 定位滚动区，把当帧布局经 `map` 交回调用方。
// 供宿主的业务逻辑（键盘揭示、缩略图可见区间）读取视口，与滑块绘制无关。
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
            self.viewport = Some(viewport_from_layout(bounds, content_bounds, translation));
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

/// 滑块在轨道坐标系内的矩形。厚度在细线与展开之间插值，贴轨道外缘、
/// 向内长粗；长度与位置按 `offset / (content - viewport)` 映射到
/// `track - thumb`，零偏移与最大偏移分别贴合轨道首尾。
fn scrollbar_thumb_bounds(
    axis: ScrollbarAxis,
    viewport: ScrollbarViewport,
    track_size: Size,
    expansion: f32,
) -> Option<Rectangle> {
    let (offset, viewport_extent, content_extent, track_length, track_thickness) = match axis {
        ScrollbarAxis::Vertical => (
            viewport.offset_y,
            viewport.viewport_height,
            viewport.content_height,
            track_size.height,
            track_size.width,
        ),
        ScrollbarAxis::Horizontal => (
            viewport.offset_x,
            viewport.viewport_width,
            viewport.content_width,
            track_size.width,
            track_size.height,
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
    let thickness = (SCROLLBAR_THIN_THICKNESS
        + (SCROLLBAR_EXPANDED_THICKNESS - SCROLLBAR_THIN_THICKNESS) * expansion.clamp(0.0, 1.0))
    .min(track_thickness);
    let cross_offset = (track_thickness - SCROLLBAR_EDGE_INSET - thickness).max(0.0);

    Some(match axis {
        ScrollbarAxis::Vertical => Rectangle {
            x: cross_offset,
            y: thumb_offset,
            width: thickness,
            height: thumb_length,
        },
        ScrollbarAxis::Horizontal => Rectangle {
            x: thumb_offset,
            y: cross_offset,
            width: thumb_length,
            height: thickness,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::scrollable;

    fn vertical_viewport(
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y,
            viewport_width: 800.0,
            viewport_height,
            content_width: 800.0,
            content_height,
        }
    }

    #[test]
    fn vertical_thumb_keeps_minimum_length_and_maps_scroll_ends() {
        let track_size = Size::new(SCROLLBAR_HIT_WIDTH, 600.0);
        let content_height = 600_000.0;

        let top = scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            vertical_viewport(0.0, 600.0, content_height),
            track_size,
            0.0,
        )
        .expect("overflowing content must produce a thumb");
        let bottom = scrollbar_thumb_bounds(
            ScrollbarAxis::Vertical,
            vertical_viewport(content_height - 600.0, 600.0, content_height),
            track_size,
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
            Size::new(800.0, SCROLLBAR_HIT_WIDTH),
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
            Size::new(SCROLLBAR_HIT_WIDTH, 600.0),
            0.0,
        )
        .is_none());
        assert!(scrollbar_thumb_bounds(
            ScrollbarAxis::Horizontal,
            viewport,
            Size::new(800.0, SCROLLBAR_HIT_WIDTH),
            0.0,
        )
        .is_none());
    }

    #[test]
    fn thin_thumb_hugs_outer_edge_and_expands_inward() {
        let viewport = vertical_viewport(300.0, 600.0, 6_000.0);
        let track_size = Size::new(SCROLLBAR_HIT_WIDTH, 600.0);
        let thin = scrollbar_thumb_bounds(ScrollbarAxis::Vertical, viewport, track_size, 0.0)
            .expect("overflowing content must produce a thumb");
        let expanded = scrollbar_thumb_bounds(ScrollbarAxis::Vertical, viewport, track_size, 1.0)
            .expect("overflowing content must produce a thumb");

        assert_eq!(thin.height, expanded.height);
        assert_eq!(thin.y, expanded.y);
        assert_eq!(thin.width, SCROLLBAR_THIN_THICKNESS);
        assert_eq!(expanded.width, SCROLLBAR_EXPANDED_THICKNESS);
        // 两种厚度的外缘都停在同一位置：展开只向内长，不会跳出命中区。
        let outer_edge = SCROLLBAR_HIT_WIDTH - SCROLLBAR_EDGE_INSET;
        assert_eq!(thin.x + thin.width, outer_edge);
        assert_eq!(expanded.x + expanded.width, outer_edge);
    }

    #[test]
    fn expansion_animates_to_target_and_reports_next_frame() {
        let mut expansion = ThumbExpansion::default();
        let start = Instant::now();

        assert!(expansion.advance(true, start).is_some());
        assert_eq!(expansion.progress, 0.0);
        assert!(expansion
            .advance(true, start + SCROLLBAR_EXPANSION_DURATION / 2)
            .is_some());
        assert!(expansion.progress > 0.5 && expansion.progress < 1.0);
        assert!(expansion
            .advance(true, start + SCROLLBAR_EXPANSION_DURATION)
            .is_none());
        assert_eq!(expansion.progress, 1.0);

        let release = start + SCROLLBAR_EXPANSION_DURATION * 2;
        assert!(expansion.advance(false, release).is_some());
        assert!(expansion
            .advance(false, release + SCROLLBAR_EXPANSION_DURATION)
            .is_none());
        assert_eq!(expansion.progress, 0.0);
    }

    #[test]
    fn track_follows_scrollable_bounds_and_leaves_corner_only_when_other_axis_overflows() {
        // 滚动容器不在原点：轨道必须跟着它自己的边框走，而不是外层包装。
        let bounds = Rectangle::new(iced::Point::new(40.0, 30.0), Size::new(800.0, 600.0));
        let both = [ScrollbarAxis::Vertical, ScrollbarAxis::Horizontal];
        let only_vertical_overflow = ProbedScroll {
            bounds,
            viewport: vertical_viewport(0.0, 600.0, 1_200.0),
        };
        let both_overflow = ProbedScroll {
            bounds,
            viewport: ScrollbarViewport {
                content_width: 1_600.0,
                ..only_vertical_overflow.viewport
            },
        };

        let vertical = track_bounds(ScrollbarAxis::Vertical, only_vertical_overflow, &both);
        assert_eq!(vertical.height, 600.0);
        assert_eq!(vertical.x, 40.0 + 800.0 - SCROLLBAR_HIT_WIDTH);
        let vertical = track_bounds(ScrollbarAxis::Vertical, both_overflow, &both);
        assert_eq!(vertical.height, 600.0 - SCROLLBAR_HIT_WIDTH);
        let horizontal = track_bounds(ScrollbarAxis::Horizontal, both_overflow, &both);
        assert_eq!(horizontal.width, 800.0 - SCROLLBAR_HIT_WIDTH);
        assert_eq!(horizontal.y, 30.0 + 600.0 - SCROLLBAR_HIT_WIDTH);
    }

    #[test]
    fn first_scrollable_probe_reads_layout_without_descending() {
        let mut probe = FirstScrollableProbe::default();
        probe.scrollable(
            None,
            Rectangle::new(iced::Point::ORIGIN, Size::new(800.0, 600.0)),
            Rectangle::new(iced::Point::ORIGIN, Size::new(800.0, 900.0)),
            Vector::new(0.0, 150.0),
            &mut UnreachableScrollableState,
        );
        let mut descended = false;
        probe.traverse(&mut |_| descended = true);

        assert!(
            !descended,
            "probe must stop once the first scrollable is read"
        );
        assert_eq!(
            probe.scroll.map(|scroll| scroll.viewport),
            Some(vertical_viewport(150.0, 600.0, 900.0))
        );
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
            viewport: None,
            map: Box::new(std::convert::identity),
        };
        probe.scrollable(
            Some(&target),
            Rectangle::new(iced::Point::ORIGIN, Size::new(800.0, 600.0)),
            Rectangle::new(iced::Point::ORIGIN, Size::new(800.0, 900.0)),
            Vector::new(0.0, 150.0),
            &mut UnreachableScrollableState,
        );

        match probe.finish() {
            Outcome::Some(viewport) => {
                assert_eq!(viewport, vertical_viewport(150.0, 600.0, 900.0))
            }
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
