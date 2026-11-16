//! 会话级滚动：业务用的视口缓存（键盘揭示、缩略图可见区间、多栏栏宽）。滚轮惯性在
//! 滚动控件里自己推进（`bennu_theme::smooth_scroll`），不经过会话。滚动条本身由
//! `bennu_theme::scrollbar` 按当帧布局常驻绘制，会话不再持有任何显隐状态。
//!
//! 多窗口约束：iced 0.14 的 widget 操作（operate/scroll_by）作用于进程
//! 内所有窗口的控件树，widget Id 必须带会话命名空间（请求路径），
//! 才能保证只命中本窗口的滚动容器。

use bennu_theme::scroll_report::ScrollPosition;
use bennu_theme::scrollbar::scrollbar_layout_probe;
use bennu_theme::smooth_scroll::SmoothScrollAxis;
use iced::advanced::widget as advanced_widget;
use iced::widget::scrollable;
use iced::{Size, Task, Vector};

use super::{PickerSession, PickerViewMode, SessionMessage};

/// portal 的滚动区域：文件列表（竖向）、地址栏面包屑（横向）、
/// 侧边栏（竖向），以及多栏视图的横向栏容器与逐栏纵向滚动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SessionScrollRegion {
    List,
    Breadcrumb,
    Sidebar,
    /// 多栏视图横向栏容器。
    ColumnsRail,
    /// 多栏第 lane 栏的纵向滚动（region 以栏序号区分，逐栏各自滚动）。
    ColumnsLane(usize),
}

/// 区域 → 滚动轴向：列表/侧栏/多栏栏竖向；面包屑与多栏栏容器是横向
/// 滚动条，主滚轮（竖向滚动）换算为横向增量，与主软件地址栏同语义。
pub(crate) fn scroll_axis(region: SessionScrollRegion) -> SmoothScrollAxis {
    match region {
        SessionScrollRegion::List
        | SessionScrollRegion::Sidebar
        | SessionScrollRegion::ColumnsLane(_) => SmoothScrollAxis::Vertical,
        SessionScrollRegion::Breadcrumb | SessionScrollRegion::ColumnsRail => {
            SmoothScrollAxis::HorizontalFromPrimaryWheel
        }
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
        SessionScrollRegion::ColumnsRail => {
            iced::widget::Id::from(format!("portal-columns-rail#{request_path}"))
        }
        SessionScrollRegion::ColumnsLane(lane) => {
            iced::widget::Id::from(format!("portal-column-{lane}#{request_path}"))
        }
    }
}
// ScrollbarViewport 需经本模块再导出给 mod.rs 的消息定义。
pub(crate) use bennu_theme::scrollbar::ScrollbarViewport;

impl SessionMessage {
    /// 滚动/滚动条类消息由滚动子模块处理并产出 Task，不经
    /// `PickerSession::update` 的会话效果路径；main 层据此拦截路由。
    pub(crate) fn is_scroll_message(&self) -> bool {
        matches!(
            self,
            SessionMessage::ScrollbarLayoutVerified { .. }
                | SessionMessage::ScrollbarViewportChanged { .. }
        )
    }
}

/// `Scrollable::on_scroll` 回调：快照视口并发 `ScrollbarViewportChanged`。
/// 只接在很少滚动的多栏栏容器（横向）上。
pub(crate) fn scrollbar_on_scroll(
    region: SessionScrollRegion,
) -> impl Fn(scrollable::Viewport) -> SessionMessage {
    move |viewport| {
        let absolute_offset = viewport.absolute_offset();
        let bounds = viewport.bounds();
        let content_bounds = viewport.content_bounds();
        viewport_changed(
            region,
            Vector::new(absolute_offset.x, absolute_offset.y),
            bounds.size(),
            content_bounds.size(),
        )
    }
}

/// `report_scroll` 回调：列表、大图和各栏只在需要时（越过缩略图余量、有输入、停下）报告位置。
pub(crate) fn scrollbar_report(
    region: SessionScrollRegion,
) -> impl Fn(ScrollPosition) -> SessionMessage {
    move |position| {
        viewport_changed(
            region,
            position.offset,
            position.bounds.size(),
            position.content_bounds.size(),
        )
    }
}

fn viewport_changed(
    region: SessionScrollRegion,
    offset: Vector,
    viewport: Size,
    content: Size,
) -> SessionMessage {
    SessionMessage::ScrollbarViewportChanged {
        region,
        viewport: ScrollbarViewport {
            offset_x: offset.x,
            offset_y: offset.y,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            content_width: content.width,
            content_height: content.height,
        },
    }
}

impl PickerSession {
    pub(crate) fn scrollbar_viewport_for(
        &self,
        region: &SessionScrollRegion,
    ) -> Option<ScrollbarViewport> {
        self.scroll_viewports.get(region).copied()
    }

    /// 键盘导航滚动跟随的偏移写入收口：程序化 scroll_to 后位置要等
    /// 滚动区停下才报告，视口缓存在此同步（真实几何由随后的布局探针回信
    /// 纠偏）；同时打断滚轮惯性——键盘滚动即时到位，不允许惯性把
    /// 偏移再拉走。
    pub(crate) fn record_keyboard_list_scroll(&mut self, offset_y: f32) {
        self.record_keyboard_scroll(SessionScrollRegion::List, offset_y);
    }

    /// 多栏键盘揭示的同款收口（逐栏视口缓存）。
    pub(crate) fn record_keyboard_lane_scroll(&mut self, lane: usize, offset_y: f32) {
        self.record_keyboard_scroll(SessionScrollRegion::ColumnsLane(lane), offset_y);
    }

    fn record_keyboard_scroll(&mut self, region: SessionScrollRegion, offset_y: f32) {
        if let Some(viewport) = self.scroll_viewports.get_mut(&region) {
            viewport.offset_y = offset_y;
        }
    }

    /// 滚动类消息入口（main 层以 `is_scroll_message` 把关后路由）：
    /// 处理探针回信与视口回传。
    pub(crate) fn handle_scroll_message(
        &mut self,
        message: SessionMessage,
    ) -> Vec<Task<SessionMessage>> {
        match message {
            SessionMessage::ScrollbarLayoutVerified { region, viewport } => {
                self.handle_scrollbar_layout_verified(region, viewport)
            }
            SessionMessage::ScrollbarViewportChanged { region, viewport } => {
                self.scroll_viewports.insert(region, viewport);
                // 列表/多栏栏视口更新：可见区间变化，重算缩略图请求（main 层
                // 在本入口返回后统一 drain 发起）。
                if region == SessionScrollRegion::List
                    || matches!(region, SessionScrollRegion::ColumnsLane(_))
                {
                    self.schedule_visible_thumbnails();
                }
                Vec::new()
            }
            // 非滚动消息不会进入本入口（is_scroll_message 把关）；仅为穷尽。
            _ => Vec::new(),
        }
    }

    /// 内容骤变后（导航/展开/扫描回填/过滤切换）把当帧布局写回视口缓存：
    /// 位置报告要等下一次重画，栏容器的 on_scroll 在内容塞得下时不发，缓存靠探针立刻对上。多栏视图下
    /// 追加栏容器与各栏（逐栏视口缓存驱动键盘揭示与缩略图可见区间）。
    pub(crate) fn scrollbar_layout_probe_tasks(&self) -> Vec<Task<SessionMessage>> {
        let mut regions = vec![SessionScrollRegion::List];
        if self.view_mode() == PickerViewMode::Columns {
            regions.push(SessionScrollRegion::ColumnsRail);
            regions.extend((0..self.columns_chain().len()).map(SessionScrollRegion::ColumnsLane));
        }
        regions
            .into_iter()
            .map(|region| self.scrollbar_layout_probe_task(region))
            .collect()
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

    /// 探针回信：把当帧布局写回缓存（自愈过期数据）。
    fn handle_scrollbar_layout_verified(
        &mut self,
        region: SessionScrollRegion,
        viewport: ScrollbarViewport,
    ) -> Vec<Task<SessionMessage>> {
        self.scroll_viewports.insert(region, viewport);
        let mut tasks = Vec::new();
        if region == SessionScrollRegion::List {
            self.schedule_visible_thumbnails();
            // 视图切换的主选中项揭示等到这里：缓存刚被新几何覆写。
            if let Some(offset_y) = self.complete_view_switch_reveal() {
                tasks.push(iced::widget::operation::scroll_to(
                    scroll_id(&self.request_path, region),
                    scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: offset_y,
                    },
                ));
            }
        } else if matches!(region, SessionScrollRegion::ColumnsLane(_)) {
            // 栏视口缓存就位：各栏可见区间重算。
            self.schedule_visible_thumbnails();
        }
        tasks
    }

    /// 帧时钟推进：展开动画；产出的 Task 由 main 层按窗口路由后批量执行。
    pub(crate) fn advance_frame(&mut self) -> Vec<Task<SessionMessage>> {
        // 地址栏渐变过渡不产出 Task，只需在帧尾清理已播完的退出过渡。
        self.advance_address_bar_transition();
        let expansion_frame = self.advance_animations();
        let mut tasks = Vec::new();
        // 收起播完触发结构性收缩后，行数/内容高度骤变：立刻自愈视口缓存。
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
            crate::picker_session::PickerViewMode::List,
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
    fn verified_viewport_heals_stale_cache() {
        let mut session = test_session();
        // 预置过期缓存：旧布局曾溢出，真实布局已塞得下。
        session
            .scroll_viewports
            .insert(SessionScrollRegion::List, overflowing_viewport());

        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: fitting_viewport(),
        });

        assert_eq!(
            session.scrollbar_viewport_for(&SessionScrollRegion::List),
            Some(fitting_viewport())
        );
    }

    #[test]
    fn viewport_changed_updates_cache() {
        let mut session = test_session();

        session.handle_scroll_message(SessionMessage::ScrollbarViewportChanged {
            region: SessionScrollRegion::List,
            viewport: overflowing_viewport(),
        });

        assert_eq!(
            session.scrollbar_viewport_for(&SessionScrollRegion::List),
            Some(overflowing_viewport())
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
