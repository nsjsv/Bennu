use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Operation, Outcome, Scrollable as ScrollableOperation};
use iced::time::{Duration as IcedDuration, Instant as IcedInstant};
use iced::widget::{canvas, container, scrollable, Stack};
use iced::{
    alignment::{Horizontal, Vertical},
    mouse, Element, Length, Point, Rectangle, Size, Task, Theme, Vector,
};

use super::runtime::scrollbar_auto_hide_command;
use super::smooth_scroll::smooth_scroll_id;
use super::FileBrowser;
use crate::animation::{ease_out_cubic, elapsed_fraction as scrollbar_animation_progress};
use crate::matugen_theme::ui_colors;
use crate::model::{
    Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, SCROLLBAR_HOVER_WIDTH,
    SCROLLBAR_MIN_THUMB_LENGTH,
};

pub(super) const SCROLLBAR_ANIMATION_INTERVAL: Duration = crate::ui_pacing::FRAME_INTERVAL_60HZ;

const SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(96);
const SCROLLBAR_HIDE_DURATION: Duration = Duration::from_millis(300);
const SCROLLBAR_MIN_REVEAL_OPACITY: f32 = 0.12;
const SCROLLBAR_HOVER_DURATION: IcedDuration = IcedDuration::from_millis(140);
const SCROLLBAR_HOVER_FRAME_INTERVAL: IcedDuration = crate::ui_pacing::FRAME_INTERVAL_60HZ;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy)]
enum ScrollbarAnimation {
    Revealing {
        started_at: Instant,
        initial_opacity: f32,
    },
    Hiding {
        started_at: Instant,
        initial_opacity: f32,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ScrollbarState {
    active_region: Option<ScrollbarRegion>,
    visibility: ScrollbarVisibility,
    auto_hide_generation: u64,
    animation: Option<ScrollbarAnimation>,
    viewport_by_region: HashMap<ScrollbarRegion, ScrollbarViewport>,
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self {
            active_region: None,
            visibility: ScrollbarVisibility::Hidden,
            auto_hide_generation: 0,
            animation: None,
            viewport_by_region: HashMap::new(),
        }
    }
}

impl FileBrowser {
    pub(super) fn scrollbar_animation_is_active(&self) -> bool {
        self.scrollbar.animation.is_some()
    }

    pub(crate) fn scrollbar_visibility_for(&self, region: &ScrollbarRegion) -> ScrollbarVisibility {
        if self.scrollbar.active_region.as_ref() == Some(region) {
            self.scrollbar.visibility
        } else {
            ScrollbarVisibility::Hidden
        }
    }

    pub(crate) fn scrollbar_viewport_for(
        &self,
        region: &ScrollbarRegion,
    ) -> Option<ScrollbarViewport> {
        self.scrollbar.viewport_by_region.get(region).copied()
    }

    pub(super) fn remember_scrollbar_viewport(
        &mut self,
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
    ) {
        self.scrollbar.viewport_by_region.insert(region, viewport);
    }

    pub(super) fn forget_scrollbar_viewport(&mut self, region: &ScrollbarRegion) {
        self.scrollbar.viewport_by_region.remove(region);
    }

    // 显示入口保持唯一：先探实时布局再决定是否淡入。iced 在内容塞得下时永不发布
    // on_scroll，缓存里的溢出数据可能过期，"能不能滚"必须以探针读到的当帧布局为准。
    pub(super) fn show_scrollbars_temporarily(&mut self, region: ScrollbarRegion) -> Task<Message> {
        self.verify_scrollbar_layout(region)
    }

    // 只探测不预设显示意图：把当帧布局写回缓存自愈过期数据；是否淡入由
    // 回信核实决定（无溢出只刷新缓存，thumb 随之消失）。视口重钉等让
    // 内容高度骤变的路径用它同步滚动条，不经过 hover 显示入口。
    pub(super) fn verify_scrollbar_layout(&mut self, region: ScrollbarRegion) -> Task<Message> {
        advanced_widget::operate(ScrollbarLayoutProbe::new(region))
    }

    // 探针回信：把当帧布局写回缓存（自愈过期数据），核实溢出后才淡入。
    pub(super) fn handle_scrollbar_layout_verified(
        &mut self,
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
    ) -> Task<Message> {
        self.remember_scrollbar_viewport(region.clone(), viewport);
        if !scrollbar_viewport_has_overflow(viewport) {
            return Task::none();
        }
        self.start_scrollbar_reveal(region)
    }

    pub(super) fn start_scrollbar_reveal(&mut self, region: ScrollbarRegion) -> Task<Message> {
        let scrollbar = &mut self.scrollbar;
        scrollbar.active_region = Some(region);
        scrollbar.auto_hide_generation = scrollbar.auto_hide_generation.wrapping_add(1);

        let current_opacity = scrollbar.visibility.opacity();
        if (1.0 - current_opacity) <= f32::EPSILON {
            scrollbar.visibility = ScrollbarVisibility::Visible;
            scrollbar.animation = None;
        } else if !matches!(
            scrollbar.animation,
            Some(ScrollbarAnimation::Revealing { .. })
        ) {
            let initial_opacity = current_opacity.max(SCROLLBAR_MIN_REVEAL_OPACITY);
            scrollbar.visibility = ScrollbarVisibility::with_opacity(initial_opacity);
            scrollbar.animation = Some(ScrollbarAnimation::Revealing {
                started_at: Instant::now(),
                initial_opacity,
            });
        }

        scrollbar_auto_hide_command(scrollbar.auto_hide_generation)
    }

    pub(super) fn start_global_scrollbar_hide(&mut self, generation: u64) {
        if self.scrollbar.auto_hide_generation != generation {
            return;
        }

        let initial_opacity = self.scrollbar.visibility.opacity();
        if initial_opacity <= f32::EPSILON {
            self.scrollbar.active_region = None;
            self.scrollbar.visibility = ScrollbarVisibility::Hidden;
            self.scrollbar.animation = None;
            return;
        }

        self.scrollbar.animation = Some(ScrollbarAnimation::Hiding {
            started_at: Instant::now(),
            initial_opacity,
        });
    }

    pub(super) fn advance_scrollbar_animation(&mut self) -> Task<Message> {
        let Some(animation) = self.scrollbar.animation else {
            return Task::none();
        };

        let still_active = match animation {
            ScrollbarAnimation::Revealing {
                started_at,
                initial_opacity,
            } => advance_scrollbar_reveal(&mut self.scrollbar, started_at, initial_opacity),
            ScrollbarAnimation::Hiding {
                started_at,
                initial_opacity,
            } => advance_scrollbar_hide(&mut self.scrollbar, started_at, initial_opacity),
        };

        if !still_active {
            self.scrollbar.active_region = None;
            self.scrollbar.visibility = ScrollbarVisibility::Hidden;
            self.scrollbar.animation = None;
        }

        Task::none()
    }
}

pub(crate) fn scrollbar_on_scroll(
    region: ScrollbarRegion,
    event: impl Fn(scrollable::Viewport) -> Message + 'static,
) -> impl Fn(scrollable::Viewport) -> Message {
    move |viewport| {
        let absolute_offset = viewport.absolute_offset();
        let bounds = viewport.bounds();
        let content_bounds = viewport.content_bounds();
        Message::ScrollbarViewportChanged {
            region: region.clone(),
            viewport: ScrollbarViewport {
                offset_x: absolute_offset.x,
                offset_y: absolute_offset.y,
                viewport_width: bounds.width,
                viewport_height: bounds.height,
                content_width: content_bounds.width,
                content_height: content_bounds.height,
            },
            event: Box::new(event(viewport)),
        }
    }
}

pub(crate) fn enhanced_scrollbar<'a>(
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
pub(crate) fn enhanced_scrollbar_both<'a>(
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

impl canvas::Program<Message> for ScrollbarOverlay {
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

fn scrollbar_has_overflow(axis: ScrollbarAxis, viewport: ScrollbarViewport) -> bool {
    match axis {
        ScrollbarAxis::Vertical => viewport.content_height > viewport.viewport_height,
        ScrollbarAxis::Horizontal => viewport.content_width > viewport.viewport_width,
    }
}

// 显示门槛看任一轴：逐轴 thumb 计算对无溢出轴本就返回 None，不会多画。
fn scrollbar_viewport_has_overflow(viewport: ScrollbarViewport) -> bool {
    viewport.content_height > viewport.viewport_height
        || viewport.content_width > viewport.viewport_width
}

// 只读不改：按 smooth_scroll_id 定位目标滚动区，把当帧布局带回给 update。
struct ScrollbarLayoutProbe {
    region: ScrollbarRegion,
    target: advanced_widget::Id,
    viewport: Option<ScrollbarViewport>,
}

impl ScrollbarLayoutProbe {
    fn new(region: ScrollbarRegion) -> Self {
        Self {
            target: smooth_scroll_id(&region).into(),
            region,
            viewport: None,
        }
    }
}

impl Operation<Message> for ScrollbarLayoutProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
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

    fn finish(&self) -> Outcome<Message> {
        match self.viewport {
            Some(viewport) => Outcome::Some(Message::ScrollbarLayoutVerified {
                region: self.region.clone(),
                viewport,
            }),
            None => Outcome::None,
        }
    }
}

fn scrollbar_thumb_bounds(
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

fn advance_scrollbar_reveal(
    scrollbar: &mut ScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = scrollbar_animation_progress(started_at, SCROLLBAR_REVEAL_DURATION);
    if progress >= 1.0 {
        scrollbar.visibility = ScrollbarVisibility::Visible;
        scrollbar.animation = None;
        return true;
    }

    let opacity = initial_opacity + (1.0 - initial_opacity) * ease_out_cubic(progress);
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn advance_scrollbar_hide(
    scrollbar: &mut ScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = scrollbar_animation_progress(started_at, SCROLLBAR_HIDE_DURATION);
    if progress >= 1.0 {
        return false;
    }

    let opacity = initial_opacity * (1.0 - smoothstep(progress));
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    progress * progress * (3.0 - 2.0 * progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn reveal_curve_starts_fast_and_finishes_at_one() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!(ease_out_cubic(0.35) > 0.65);
        assert_eq!(ease_out_cubic(1.0), 1.0);
    }

    #[test]
    fn hide_curve_keeps_endpoints_stable() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert!((smoothstep(0.5) - 0.5).abs() <= f32::EPSILON);
        assert_eq!(smoothstep(1.0), 1.0);
    }

    #[test]
    fn scrollbar_visibility_only_applies_to_active_region() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let active_region = ScrollbarRegion::Sidebar;
        let inactive_region = ScrollbarRegion::Settings;

        drop(browser.start_scrollbar_reveal(active_region.clone()));

        assert!(browser.scrollbar_visibility_for(&active_region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_visibility_for(&inactive_region),
            ScrollbarVisibility::Hidden
        );
    }

    #[test]
    fn changing_scrollbar_region_moves_active_visibility() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let first_region = ScrollbarRegion::Sidebar;
        let second_region = ScrollbarRegion::Settings;

        drop(browser.start_scrollbar_reveal(first_region.clone()));
        drop(browser.start_scrollbar_reveal(second_region.clone()));

        assert_eq!(
            browser.scrollbar_visibility_for(&first_region),
            ScrollbarVisibility::Hidden
        );
        assert!(browser.scrollbar_visibility_for(&second_region).opacity() > 0.0);
    }

    #[test]
    fn stale_scrollbar_hide_does_not_hide_region_after_new_scroll() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::Sidebar;

        drop(browser.start_scrollbar_reveal(region.clone()));
        drop(browser.start_scrollbar_reveal(region.clone()));
        browser.start_global_scrollbar_hide(1);

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
    }

    #[test]
    fn scrollbar_hide_uses_global_generation() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.start_scrollbar_reveal(ScrollbarRegion::Sidebar));
        browser.start_global_scrollbar_hide(1);

        assert!(matches!(
            browser.scrollbar.animation,
            Some(ScrollbarAnimation::Hiding { .. })
        ));
    }

    #[test]
    fn icon_grid_uses_global_auto_hide_scrollbar_state() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneIcons(crate::model::BrowserPaneId::PRIMARY);

        drop(browser.start_scrollbar_reveal(region.clone()));

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_visibility_for(&ScrollbarRegion::Sidebar),
            ScrollbarVisibility::Hidden
        );
    }

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
    fn verified_viewport_without_overflow_heals_cache_and_skips_reveal() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneList(crate::model::BrowserPaneId::PRIMARY);
        // 预置过期缓存：旧布局曾溢出，真实布局已塞得下。
        browser.remember_scrollbar_viewport(region.clone(), viewport_for(1500.0, 600.0));

        drop(browser.handle_scrollbar_layout_verified(region.clone(), viewport_for(600.0, 600.0)));

        assert_eq!(
            browser.scrollbar_visibility_for(&region),
            ScrollbarVisibility::Hidden
        );
        assert_eq!(
            browser.scrollbar_viewport_for(&region),
            Some(viewport_for(600.0, 600.0))
        );
    }

    #[test]
    fn verified_viewport_with_overflow_reveals_scrollbar() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneList(crate::model::BrowserPaneId::PRIMARY);

        drop(browser.handle_scrollbar_layout_verified(region.clone(), viewport_for(1500.0, 600.0)));

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_viewport_for(&region),
            Some(viewport_for(1500.0, 600.0))
        );
    }

    #[test]
    fn probe_without_target_produces_no_reply() {
        let probe = ScrollbarLayoutProbe::new(ScrollbarRegion::Settings);

        assert!(matches!(probe.finish(), Outcome::None));
    }

    #[test]
    fn probe_with_target_replies_with_region_and_viewport() {
        let region = ScrollbarRegion::Settings;
        let mut probe = ScrollbarLayoutProbe::new(region.clone());
        probe.viewport = Some(viewport_for(900.0, 600.0));

        match probe.finish() {
            Outcome::Some(Message::ScrollbarLayoutVerified {
                region: replied_region,
                viewport,
            }) => {
                assert_eq!(replied_region, region);
                assert_eq!(viewport, viewport_for(900.0, 600.0));
            }
            _ => panic!("probe must reply with the verified viewport"),
        }
    }
}
