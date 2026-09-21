//! 会话级滚动状态机：Mos 惯性（复用 `bennu_theme::smooth_scroll`）与
//! mac 式滚动条显隐（主软件 `app/scrollbar.rs` 状态机的小型移植）。
//! 视觉层、滚轮换算与布局探针都在 bennu-theme 共享层，这里只保留
//! 显隐状态、代数戳防旧、视口缓存自愈与消息路由。
//!
//! 多窗口约束：iced 0.14 的 widget 操作（operate/scroll_by）作用于进程
//! 内所有窗口的控件树，widget Id 必须带会话命名空间（请求路径），
//! 才能保证只命中本窗口的滚动容器。

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ScrollbarViewport 需经本模块再导出给 mod.rs 的消息定义。
pub(crate) use bennu_theme::scrollbar::ScrollbarViewport;
use bennu_theme::scrollbar::{scrollbar_layout_probe, scrollbar_viewport_has_overflow};
use bennu_theme::smooth_scroll::{
    wheel_delta_for_axis, MosScrollState, SmoothScrollAxis, WheelScrollMode,
};
use bennu_theme::styles::ScrollbarVisibility;
use iced::advanced::widget as advanced_widget;
use iced::widget::scrollable;
use iced::{mouse, Task};

use super::{PickerSession, SessionMessage};

/// 滚动条淡入时长：与主软件 reveal 节奏一致。
const SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(96);
/// 滚动条淡出时长：与主软件 hide 节奏一致。
const SCROLLBAR_HIDE_DURATION: Duration = Duration::from_millis(300);
/// 淡入起始透明度下限：滚动中途反向时滑块不从零闪现。
const SCROLLBAR_MIN_REVEAL_OPACITY: f32 = 0.12;
/// 无输入后自动隐藏的延迟：与主软件一致。
const SCROLLBAR_AUTO_HIDE_DURATION: Duration = Duration::from_millis(650);

/// portal 的滚动区域：文件列表（竖向）与地址栏面包屑（横向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SessionScrollRegion {
    List,
    Breadcrumb,
}

/// 会话滚轮惯性状态机：区域枚举由本模块钉死。
pub(crate) type SmoothScrollState = MosScrollState<SessionScrollRegion>;

/// 区域 → 滚动轴向：列表竖向；面包屑是横向滚动条，主滚轮（竖向
/// 滚动）换算为横向增量，与主软件地址栏同语义。
pub(crate) fn scroll_axis(region: SessionScrollRegion) -> SmoothScrollAxis {
    match region {
        SessionScrollRegion::List => SmoothScrollAxis::Vertical,
        SessionScrollRegion::Breadcrumb => SmoothScrollAxis::HorizontalFromPrimaryWheel,
    }
}

/// 滚动容器的 widget Id：widget 操作跨全部窗口执行，请求路径做命名
/// 空间后操作只会命中创建它的那个窗口。
pub(crate) fn scroll_id(request_path: &str, region: SessionScrollRegion) -> iced::widget::Id {
    match region {
        SessionScrollRegion::List => {
            iced::widget::Id::from(format!("portal-file-list#{request_path}"))
        }
        SessionScrollRegion::Breadcrumb => {
            iced::widget::Id::from(format!("portal-breadcrumb#{request_path}"))
        }
    }
}

impl SessionMessage {
    /// 滚动/滚动条类消息由滚动子模块处理并产出 Task，不经
    /// `PickerSession::update` 的会话效果路径；main 层据此拦截路由。
    pub(crate) fn is_scroll_message(&self) -> bool {
        matches!(
            self,
            SessionMessage::WheelScrolled { .. }
                | SessionMessage::ScrollbarLayoutVerified { .. }
                | SessionMessage::ScrollbarViewportChanged { .. }
                | SessionMessage::ScrollbarEngaged { .. }
                | SessionMessage::ScrollbarAutoHideElapsed { .. }
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum ScrollbarOpacityAnimation {
    Revealing {
        started_at: Instant,
        initial_opacity: f32,
    },
    Hiding {
        started_at: Instant,
        initial_opacity: f32,
    },
}

/// mac 式滚动条显隐状态（主软件 ScrollbarState 的两区域移植）：
/// 单活跃区域、代数戳防旧 hide、视口快照按区域缓存。
#[derive(Debug)]
pub(crate) struct SessionScrollbarState {
    active_region: Option<SessionScrollRegion>,
    visibility: ScrollbarVisibility,
    auto_hide_generation: u64,
    animation: Option<ScrollbarOpacityAnimation>,
    viewport_by_region: HashMap<SessionScrollRegion, ScrollbarViewport>,
}

impl Default for SessionScrollbarState {
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

impl SessionScrollbarState {
    /// 滚动条淡入/淡出是否在播放（帧时钟挂载条件之一）。
    pub(crate) fn is_animating(&self) -> bool {
        self.animation.is_some()
    }
}

/// `Scrollable::on_scroll` 回调包装：快照视口并发 `ScrollbarViewportChanged`，
/// 内层事件继续按滚动消息路由（与主软件同名适配同构）。
pub(crate) fn scrollbar_on_scroll(
    region: SessionScrollRegion,
    event: impl Fn(scrollable::Viewport) -> SessionMessage + 'static,
) -> impl Fn(scrollable::Viewport) -> SessionMessage {
    move |viewport| {
        let absolute_offset = viewport.absolute_offset();
        let bounds = viewport.bounds();
        let content_bounds = viewport.content_bounds();
        SessionMessage::ScrollbarViewportChanged {
            region,
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

impl PickerSession {
    /// 滚动轴向换算用的 shift：视图包装层与处理端必须读同一来源。
    pub(crate) fn scroll_shift_pressed(&self) -> bool {
        self.shift_pressed
    }

    pub(crate) fn set_scroll_shift_pressed(&mut self, pressed: bool) {
        self.shift_pressed = pressed;
    }

    pub(crate) fn scrollbar_visibility_for(
        &self,
        region: &SessionScrollRegion,
    ) -> ScrollbarVisibility {
        if self.scrollbar.active_region.as_ref() == Some(region) {
            self.scrollbar.visibility
        } else {
            ScrollbarVisibility::Hidden
        }
    }

    pub(crate) fn scrollbar_viewport_for(
        &self,
        region: &SessionScrollRegion,
    ) -> Option<ScrollbarViewport> {
        self.scrollbar.viewport_by_region.get(region).copied()
    }

    /// 滚动类消息入口（main 层以 `is_scroll_message` 把关后路由）：
    /// 处理滚轮、探针回信、视口回传、auto-hide 与滚动条直接交互。
    pub(crate) fn handle_scroll_message(
        &mut self,
        message: SessionMessage,
    ) -> Vec<Task<SessionMessage>> {
        match message {
            SessionMessage::WheelScrolled { region, delta } => {
                self.handle_wheel_scrolled(region, delta)
            }
            SessionMessage::ScrollbarLayoutVerified { region, viewport } => {
                self.handle_scrollbar_layout_verified(region, viewport)
            }
            // 视口快照先写缓存（thumb 位置跟随），内层事件继续按滚动消息处理。
            SessionMessage::ScrollbarViewportChanged {
                region,
                viewport,
                event,
            } => {
                self.scrollbar.viewport_by_region.insert(region, viewport);
                self.handle_scroll_message(*event)
            }
            SessionMessage::ScrollbarEngaged { region } => self.show_scrollbars_temporarily(region),
            SessionMessage::ScrollbarAutoHideElapsed { generation } => {
                self.start_scrollbar_hide(generation);
                Vec::new()
            }
            // 非滚动消息不会进入本入口（is_scroll_message 把关）；仅为穷尽。
            _ => Vec::new(),
        }
    }

    /// 滚轮输入：Lines 走 Mos 惯性累积并暂显滚动条；Pixels（触控板）
    /// 停止惯性直接位移。portal 无密度档位语义，Ctrl+滚轮按普通滚动处理。
    fn handle_wheel_scrolled(
        &mut self,
        region: SessionScrollRegion,
        delta: mouse::ScrollDelta,
    ) -> Vec<Task<SessionMessage>> {
        let Some(wheel) = wheel_delta_for_axis(scroll_axis(region), self.shift_pressed, delta)
        else {
            return Vec::new();
        };
        match wheel.mode {
            WheelScrollMode::MosAnimated => {
                self.smooth_scroll.push_wheel_delta(region, wheel.delta);
                self.show_scrollbars_temporarily(region)
            }
            WheelScrollMode::Direct => {
                self.smooth_scroll.stop();
                let scroll_task = iced::widget::operation::scroll_by(
                    scroll_id(&self.request_path, region),
                    scrollable::AbsoluteOffset {
                        x: wheel.delta.x,
                        y: wheel.delta.y,
                    },
                );
                let mut tasks = self.show_scrollbars_temporarily(region);
                tasks.push(scroll_task);
                tasks
            }
        }
    }

    /// 显示入口保持唯一：先探实时布局再决定是否淡入。iced 在内容塞得下
    /// 时永不发布 on_scroll，缓存可能过期，“能不能滚”以探针读到的当帧
    /// 布局为准；回信核实溢出后才淡入（`handle_scrollbar_layout_verified`）。
    fn show_scrollbars_temporarily(
        &mut self,
        region: SessionScrollRegion,
    ) -> Vec<Task<SessionMessage>> {
        vec![self.scrollbar_layout_probe_task(region)]
    }

    /// 内容骤变后（导航/展开/扫描回填/过滤切换）核实两个区域的溢出：
    /// 只探测不预设显示意图，无溢出仅自愈缓存。main 层翻译
    /// `SessionEffect::VerifyScrollbarLayout` 时调用。
    pub(crate) fn scrollbar_layout_probe_tasks(&self) -> Vec<Task<SessionMessage>> {
        vec![
            self.scrollbar_layout_probe_task(SessionScrollRegion::List),
            self.scrollbar_layout_probe_task(SessionScrollRegion::Breadcrumb),
        ]
    }

    fn scrollbar_layout_probe_task(&self, region: SessionScrollRegion) -> Task<SessionMessage> {
        let request_path = self.request_path.clone();
        advanced_widget::operate(scrollbar_layout_probe(
            scroll_id(&request_path, region),
            move |viewport| SessionMessage::ScrollbarLayoutVerified { region, viewport },
        ))
    }

    /// 探针回信：把当帧布局写回缓存（自愈过期数据），核实溢出后才淡入。
    fn handle_scrollbar_layout_verified(
        &mut self,
        region: SessionScrollRegion,
        viewport: ScrollbarViewport,
    ) -> Vec<Task<SessionMessage>> {
        self.scrollbar.viewport_by_region.insert(region, viewport);
        if !scrollbar_viewport_has_overflow(viewport) {
            return Vec::new();
        }
        self.start_scrollbar_reveal(region)
    }

    fn start_scrollbar_reveal(&mut self, region: SessionScrollRegion) -> Vec<Task<SessionMessage>> {
        let scrollbar = &mut self.scrollbar;
        scrollbar.active_region = Some(region);
        // 代数戳：每次 reveal 重排 auto-hide，旧延迟回信凭不匹配被丢弃。
        scrollbar.auto_hide_generation = scrollbar.auto_hide_generation.wrapping_add(1);

        let current_opacity = scrollbar.visibility.opacity();
        if (1.0 - current_opacity) <= f32::EPSILON {
            scrollbar.visibility = ScrollbarVisibility::Visible;
            scrollbar.animation = None;
        } else if !matches!(
            scrollbar.animation,
            Some(ScrollbarOpacityAnimation::Revealing { .. })
        ) {
            let initial_opacity = current_opacity.max(SCROLLBAR_MIN_REVEAL_OPACITY);
            scrollbar.visibility = ScrollbarVisibility::with_opacity(initial_opacity);
            scrollbar.animation = Some(ScrollbarOpacityAnimation::Revealing {
                started_at: Instant::now(),
                initial_opacity,
            });
        }

        vec![scrollbar_auto_hide_task(scrollbar.auto_hide_generation)]
    }

    fn start_scrollbar_hide(&mut self, generation: u64) {
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

        self.scrollbar.animation = Some(ScrollbarOpacityAnimation::Hiding {
            started_at: Instant::now(),
            initial_opacity,
        });
    }

    fn advance_smooth_scroll(&mut self) -> Task<SessionMessage> {
        // 克隆命名空间：next_frame_delta 独占借用状态机，闭包不能再借 self。
        let request_path = self.request_path.clone();
        let scroll_task = self
            .smooth_scroll
            .next_frame_delta()
            .map(|(region, delta)| {
                iced::widget::operation::scroll_by(
                    scroll_id(&request_path, region),
                    scrollable::AbsoluteOffset {
                        x: delta.x,
                        y: delta.y,
                    },
                )
            });

        if !self.smooth_scroll.is_active() {
            self.smooth_scroll.stop();
        }

        scroll_task.unwrap_or_else(Task::none)
    }

    fn advance_scrollbar_animation(&mut self) -> Task<SessionMessage> {
        let Some(animation) = self.scrollbar.animation else {
            return Task::none();
        };

        let still_active = match animation {
            ScrollbarOpacityAnimation::Revealing {
                started_at,
                initial_opacity,
            } => advance_scrollbar_reveal(&mut self.scrollbar, started_at, initial_opacity),
            ScrollbarOpacityAnimation::Hiding {
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

    /// 帧时钟推进：展开动画 + 惯性滚动位移 + 滚动条透明度；产出的
    /// Task 由 main 层按窗口路由后批量执行。
    pub(crate) fn advance_frame(&mut self) -> Vec<Task<SessionMessage>> {
        // 地址栏渐变过渡不产出 Task，只需在帧尾清理已播完的退出过渡。
        self.advance_address_bar_transition();
        let expansion_frame = self.advance_animations();
        let mut tasks = vec![
            self.advance_smooth_scroll(),
            self.advance_scrollbar_animation(),
        ];
        // 收起播完触发结构性收缩后，行数/内容高度骤变：立刻核实两个
        // 区域的溢出并自愈视口缓存。
        if expansion_frame.collapse_finished {
            tasks.extend(self.scrollbar_layout_probe_tasks());
        }
        tasks
    }
}

/// 650ms 无输入后回信 auto-hide；回信携带代数，重排后的旧回信被丢弃。
fn scrollbar_auto_hide_task(generation: u64) -> Task<SessionMessage> {
    Task::perform(
        async move {
            tokio::time::sleep(SCROLLBAR_AUTO_HIDE_DURATION).await;
            generation
        },
        |generation| SessionMessage::ScrollbarAutoHideElapsed { generation },
    )
}

fn advance_scrollbar_reveal(
    scrollbar: &mut SessionScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = elapsed_fraction(started_at, SCROLLBAR_REVEAL_DURATION);
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
    scrollbar: &mut SessionScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = elapsed_fraction(started_at, SCROLLBAR_HIDE_DURATION);
    if progress >= 1.0 {
        return false;
    }

    let opacity = initial_opacity * (1.0 - smoothstep(progress));
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn elapsed_fraction(started_at: Instant, duration: Duration) -> f32 {
    (started_at.elapsed().as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

// 与主软件动画模块同式的缓动：视觉节奏保持一致，不为单个纯函数
// 把 app-ui 的动画模块拖进依赖。
fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    progress * progress * (3.0 - 2.0 * progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus_file_chooser::PickerResolution;
    use crate::picker_request::{PickerKind, PickerRequestSpec};
    use std::path::PathBuf;

    fn test_session() -> PickerSession {
        let (reply, _receiver) = tokio::sync::oneshot::channel::<PickerResolution>();
        PickerSession::new(
            &PickerRequestSpec {
                kind: PickerKind::OpenFile {
                    multiple: false,
                    directory: true,
                },
                accept_label: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
            },
            "/org/freedesktop/portal/desktop/request/test".to_string(),
            PathBuf::from("/tmp"),
            reply,
        )
    }

    fn overflowing_viewport() -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
            content_width: 800.0,
            content_height: 1500.0,
        }
    }

    fn fitting_viewport() -> ScrollbarViewport {
        ScrollbarViewport {
            content_height: 600.0,
            ..overflowing_viewport()
        }
    }

    #[test]
    fn reveal_and_hide_curves_keep_endpoints() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(smoothstep(0.0), 0.0);
        assert!((smoothstep(0.5) - 0.5).abs() <= f32::EPSILON);
        assert_eq!(smoothstep(1.0), 1.0);
    }

    #[test]
    fn verified_viewport_with_overflow_reveals_scrollbar() {
        let mut session = test_session();

        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: overflowing_viewport(),
        });

        assert!(
            session
                .scrollbar_visibility_for(&SessionScrollRegion::List)
                .opacity()
                > 0.0
        );
        assert!(session.is_animating());
    }

    #[test]
    fn verified_viewport_without_overflow_heals_cache_and_skips_reveal() {
        let mut session = test_session();
        // 预置过期缓存：旧布局曾溢出，真实布局已塞得下。
        session
            .scrollbar
            .viewport_by_region
            .insert(SessionScrollRegion::List, overflowing_viewport());

        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: fitting_viewport(),
        });

        assert_eq!(
            session.scrollbar_visibility_for(&SessionScrollRegion::List),
            ScrollbarVisibility::Hidden
        );
        assert_eq!(
            session.scrollbar_viewport_for(&SessionScrollRegion::List),
            Some(fitting_viewport())
        );
    }

    #[test]
    fn stale_auto_hide_generation_does_not_hide() {
        let mut session = test_session();
        drop(session.start_scrollbar_reveal(SessionScrollRegion::List));
        drop(session.start_scrollbar_reveal(SessionScrollRegion::List));

        // 第一次 reveal 的旧代数回信：必须被丢弃。
        session.start_scrollbar_hide(1);

        assert!(
            session
                .scrollbar_visibility_for(&SessionScrollRegion::List)
                .opacity()
                > 0.0
        );
    }

    #[test]
    fn hide_animation_runs_to_completion_and_clears_state() {
        let mut session = test_session();
        drop(session.start_scrollbar_reveal(SessionScrollRegion::List));
        let generation = session.scrollbar.auto_hide_generation;

        session.start_scrollbar_hide(generation);
        assert!(session.is_animating());
        // 淡出按真实时间推进：等过 300ms 后再推一帧，动画必须收尾。
        std::thread::sleep(SCROLLBAR_HIDE_DURATION + Duration::from_millis(10));
        drop(session.advance_frame());

        assert_eq!(
            session.scrollbar_visibility_for(&SessionScrollRegion::List),
            ScrollbarVisibility::Hidden
        );
        assert!(!session.is_animating());
    }

    #[test]
    fn visibility_moves_with_active_region_switch() {
        let mut session = test_session();
        drop(session.start_scrollbar_reveal(SessionScrollRegion::List));

        assert!(
            session
                .scrollbar_visibility_for(&SessionScrollRegion::List)
                .opacity()
                > 0.0
        );
        assert_eq!(
            session.scrollbar_visibility_for(&SessionScrollRegion::Breadcrumb),
            ScrollbarVisibility::Hidden
        );

        drop(session.start_scrollbar_reveal(SessionScrollRegion::Breadcrumb));
        assert_eq!(
            session.scrollbar_visibility_for(&SessionScrollRegion::List),
            ScrollbarVisibility::Hidden
        );
        assert!(
            session
                .scrollbar_visibility_for(&SessionScrollRegion::Breadcrumb)
                .opacity()
                > 0.0
        );
    }

    #[test]
    fn line_wheel_engages_inertia_and_pixel_wheel_stops_it() {
        let mut session = test_session();

        session.handle_wheel_scrolled(
            SessionScrollRegion::List,
            mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        );
        assert!(session.smooth_scroll.is_active());
        assert!(session.is_animating());

        session.handle_wheel_scrolled(
            SessionScrollRegion::List,
            mouse::ScrollDelta::Pixels { x: 0.0, y: -30.0 },
        );
        assert!(!session.smooth_scroll.is_active());
    }

    #[test]
    fn advance_frame_drains_inertia() {
        let mut session = test_session();
        session.handle_wheel_scrolled(
            SessionScrollRegion::List,
            mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
        );

        for _ in 0..200 {
            if !session.smooth_scroll.is_active() {
                break;
            }
            drop(session.advance_frame());
        }

        assert!(!session.smooth_scroll.is_active());
    }

    #[test]
    fn viewport_changed_updates_cache_and_routes_inner_event() {
        let mut session = test_session();

        session.handle_scroll_message(SessionMessage::ScrollbarViewportChanged {
            region: SessionScrollRegion::Breadcrumb,
            viewport: overflowing_viewport(),
            event: Box::new(SessionMessage::ScrollbarEngaged {
                region: SessionScrollRegion::Breadcrumb,
            }),
        });

        assert_eq!(
            session.scrollbar_viewport_for(&SessionScrollRegion::Breadcrumb),
            Some(overflowing_viewport())
        );
        // 视口回传只刷缓存；淡入必须等探针回信核实溢出。
        assert_eq!(
            session.scrollbar_visibility_for(&SessionScrollRegion::Breadcrumb),
            ScrollbarVisibility::Hidden
        );

        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::Breadcrumb,
            viewport: overflowing_viewport(),
        });
        assert!(
            session
                .scrollbar_visibility_for(&SessionScrollRegion::Breadcrumb)
                .opacity()
                > 0.0
        );
    }

    #[test]
    fn scroll_ids_are_namespaced_per_session_and_region() {
        let first = scroll_id("/req/a", SessionScrollRegion::List);
        let second = scroll_id("/req/b", SessionScrollRegion::List);
        let breadcrumb = scroll_id("/req/a", SessionScrollRegion::Breadcrumb);

        assert_ne!(first, second);
        assert_ne!(first, breadcrumb);
    }
}
