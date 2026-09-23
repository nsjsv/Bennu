//! 会话级滚动状态机：Mos 惯性（复用 `bennu_theme::smooth_scroll`）与
//! mac 式滚动条显隐（主软件 `app/scrollbar.rs` 状态机的小型移植）。
//! 显隐状态机本体在 crate 级 `scrollbar_state`（区域泛化，预览窗滚动
//! 管线共用）；本模块保留会话侧差异——区域枚举、widget id 命名空间与
//! 消息路由。
//!
//! 多窗口约束：iced 0.14 的 widget 操作（operate/scroll_by）作用于进程
//! 内所有窗口的控件树，widget Id 必须带会话命名空间（请求路径），
//! 才能保证只命中本窗口的滚动容器。

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
use crate::scrollbar_state::{scrollbar_auto_hide_task, ScrollbarStateMachine};

/// portal 的滚动区域：文件列表（竖向）、地址栏面包屑（横向）与
/// 侧边栏（竖向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SessionScrollRegion {
    List,
    Breadcrumb,
    Sidebar,
}

/// 会话滚轮惯性状态机：区域枚举由本模块钉死。
pub(crate) type SmoothScrollState = MosScrollState<SessionScrollRegion>;
/// 区域 → 滚动轴向：列表/侧栏竖向；面包屑是横向滚动条，主滚轮（竖向
/// 滚动）换算为横向增量，与主软件地址栏同语义。
pub(crate) fn scroll_axis(region: SessionScrollRegion) -> SmoothScrollAxis {
    match region {
        SessionScrollRegion::List | SessionScrollRegion::Sidebar => SmoothScrollAxis::Vertical,
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
        SessionScrollRegion::Sidebar => {
            iced::widget::Id::from(format!("portal-sidebar#{request_path}"))
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

/// mac 式滚动条显隐状态（主软件 ScrollbarState 的两区域移植）：
/// 状态机本体在 scrollbar_state，这里按会话区域实例化。
pub(crate) type SessionScrollbarState = ScrollbarStateMachine<SessionScrollRegion>;

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
        self.scrollbar.visibility_for(region)
    }

    pub(crate) fn scrollbar_viewport_for(
        &self,
        region: &SessionScrollRegion,
    ) -> Option<ScrollbarViewport> {
        self.scrollbar.viewport_for(region)
    }

    /// 键盘导航滚动跟随的偏移写入收口：程序化 scroll_to 不回发
    /// on_scroll，视口缓存在此同步（真实几何由随后的布局探针回信
    /// 纠偏）；同时打断滚轮惯性——键盘滚动即时到位，不允许惯性把
    /// 偏移再拉走。
    pub(crate) fn record_keyboard_list_scroll(&mut self, offset_y: f32) {
        if let Some(mut viewport) = self.scrollbar.viewport_for(&SessionScrollRegion::List) {
            viewport.offset_y = offset_y;
            self.scrollbar
                .remember_viewport(SessionScrollRegion::List, viewport);
        }
        self.smooth_scroll.stop();
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
                self.scrollbar.remember_viewport(region, viewport);
                // 列表视口更新：可见区间变化，重算缩略图请求（main 层
                // 在本入口返回后统一 drain 发起）。
                if region == SessionScrollRegion::List {
                    self.schedule_visible_thumbnails();
                }
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
            self.scrollbar_layout_probe_task(SessionScrollRegion::Sidebar),
        ]
    }

    pub(crate) fn scrollbar_layout_probe_task(
        &self,
        region: SessionScrollRegion,
    ) -> Task<SessionMessage> {
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
        self.scrollbar.remember_viewport(region, viewport);
        if region == SessionScrollRegion::List {
            self.schedule_visible_thumbnails();
        }
        if !scrollbar_viewport_has_overflow(viewport) {
            return Vec::new();
        }
        self.start_scrollbar_reveal(region)
    }

    fn start_scrollbar_reveal(&mut self, region: SessionScrollRegion) -> Vec<Task<SessionMessage>> {
        let generation = self.scrollbar.start_reveal(region);
        vec![scrollbar_auto_hide_task(generation, |generation| {
            SessionMessage::ScrollbarAutoHideElapsed { generation }
        })]
    }

    fn start_scrollbar_hide(&mut self, generation: u64) {
        self.scrollbar.start_hide(generation);
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
        self.scrollbar.advance_animation();
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
                title: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
                choices: Vec::new(),
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
            .remember_viewport(SessionScrollRegion::List, overflowing_viewport());

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
        let generation = session.scrollbar.auto_hide_generation();

        session.start_scrollbar_hide(generation);
        assert!(session.is_animating());
        // 淡出按真实时间推进：等过 300ms 后再推一帧，动画必须收尾。
        std::thread::sleep(
            crate::scrollbar_state::SCROLLBAR_HIDE_DURATION + std::time::Duration::from_millis(10),
        );
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
