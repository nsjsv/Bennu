//! 预览窗滚动管线：七个滚动区域（目录树/归档树/分页文档/纯文本/
//! Markdown/SQLite 表列表/SQLite 数据网格）的滚轮惯性、布局探针与
//! mac 式滚动条显隐。机制与 picker_session/scrollbar.rs 同源（共享
//! scrollbar_state 状态机核），差异只在：区域枚举、widget id 命名空间
//! （portal-preview-*，与选择窗的 portal-file-list#/portal-breadcrumb#
//! 前缀互不串窗）、视口回传的内层事件是 PreviewMessage（经
//! Message::Preview 回流引擎/宿主回退路由）。

use bennu_theme::scrollbar::{
    scrollbar_layout_probe, scrollbar_viewport_has_overflow, ScrollbarViewport,
};
use bennu_theme::smooth_scroll::{
    wheel_delta_for_axis, MosScrollState, SmoothScrollAxis, WheelScrollMode,
};
use bennu_theme::styles::ScrollbarVisibility;
use iced::advanced::widget as advanced_widget;
use iced::widget::scrollable;
use iced::{mouse, Task};

use crate::scrollbar_state::{scrollbar_auto_hide_task, ScrollbarStateMachine};
use crate::Message;

/// 预览窗的滚动区域（主软件 ScrollbarRegion 预览七区的 portal 对应物）。
/// Copy：接线闭包与探针回信按值携带。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PreviewScrollRegion {
    Directory,
    Archive,
    Document,
    Text,
    Markdown,
    SqliteTables,
    SqliteData,
}

/// 滚动容器的 widget Id：widget 操作跨全部窗口执行，portal-preview-*
/// 前缀保证只命中预览窗的滚动容器（选择窗用请求路径命名空间，前缀
/// 不同不会串窗；进程至多一个预览窗，固定 id 即唯一）。
pub(crate) fn preview_scroll_id(region: PreviewScrollRegion) -> iced::widget::Id {
    iced::widget::Id::from(format!(
        "portal-preview-{}",
        match region {
            PreviewScrollRegion::Directory => "directory",
            PreviewScrollRegion::Archive => "archive",
            PreviewScrollRegion::Document => "document",
            PreviewScrollRegion::Text => "text",
            PreviewScrollRegion::Markdown => "markdown",
            PreviewScrollRegion::SqliteTables => "sqlite-tables",
            PreviewScrollRegion::SqliteData => "sqlite-data",
        }
    ))
}

/// 预览窗滚动状态：显隐状态机 + 滚轮惯性，归 PreviewHost 持有。
#[derive(Debug, Default)]
pub(crate) struct PreviewScrollPipeline {
    pub(crate) scrollbar: ScrollbarStateMachine<PreviewScrollRegion>,
    pub(crate) smooth_scroll: MosScrollState<PreviewScrollRegion>,
}

impl PreviewScrollPipeline {
    pub(crate) fn visibility_for(&self, region: PreviewScrollRegion) -> ScrollbarVisibility {
        self.scrollbar.visibility_for(&region)
    }

    pub(crate) fn viewport_for(&self, region: PreviewScrollRegion) -> Option<ScrollbarViewport> {
        self.scrollbar.viewport_for(&region)
    }

    pub(crate) fn remember_viewport(
        &mut self,
        region: PreviewScrollRegion,
        viewport: ScrollbarViewport,
    ) {
        self.scrollbar.remember_viewport(region, viewport);
    }

    /// 帧时钟是否需要挂载（滚动条淡入淡出 ∪ 惯性滚动）。
    pub(crate) fn is_animating(&self) -> bool {
        self.scrollbar.is_animating() || self.smooth_scroll.is_active()
    }

    /// 会话结束整体复位（新预览会话不继承旧视口几何与显隐进度）。
    pub(crate) fn reset(&mut self) {
        self.scrollbar.reset();
        self.smooth_scroll.stop();
    }

    /// 滚轮输入：Lines 走 Mos 惯性累积并暂显滚动条；Pixels（触控板）
    /// 停止惯性直接位移（与选择窗滚轮处理同式）。
    pub(crate) fn handle_wheel_scrolled(
        &mut self,
        region: PreviewScrollRegion,
        shift_pressed: bool,
        delta: mouse::ScrollDelta,
    ) -> Task<Message> {
        // 预览七区全为竖向滚动（主软件 preview 区域同轴）。
        let Some(wheel) = wheel_delta_for_axis(SmoothScrollAxis::Vertical, shift_pressed, delta)
        else {
            return Task::none();
        };
        match wheel.mode {
            WheelScrollMode::MosAnimated => {
                self.smooth_scroll.push_wheel_delta(region, wheel.delta);
                self.show_scrollbars_temporarily(region)
            }
            WheelScrollMode::Direct => {
                self.smooth_scroll.stop();
                let scroll_task = iced::widget::operation::scroll_by(
                    preview_scroll_id(region),
                    scrollable::AbsoluteOffset {
                        x: wheel.delta.x,
                        y: wheel.delta.y,
                    },
                );
                Task::batch([scroll_task, self.show_scrollbars_temporarily(region)])
            }
        }
    }

    /// 显示入口唯一：先探实时布局再决定是否淡入（内容塞得下时 iced
    /// 永不发布 on_scroll，缓存会过期）。
    fn show_scrollbars_temporarily(&mut self, region: PreviewScrollRegion) -> Task<Message> {
        self.layout_probe_task(region)
    }

    pub(crate) fn layout_probe_task(&self, region: PreviewScrollRegion) -> Task<Message> {
        advanced_widget::operate(scrollbar_layout_probe(
            preview_scroll_id(region),
            move |viewport| Message::PreviewScrollbarLayoutVerified { region, viewport },
        ))
    }

    /// 探针回信：当帧布局写回缓存（自愈），核实溢出后才淡入。
    pub(crate) fn handle_layout_verified(
        &mut self,
        region: PreviewScrollRegion,
        viewport: ScrollbarViewport,
    ) -> Task<Message> {
        self.scrollbar.remember_viewport(region, viewport);
        if !scrollbar_viewport_has_overflow(viewport) {
            return Task::none();
        }
        let generation = self.scrollbar.start_reveal(region);
        scrollbar_auto_hide_task(generation, |generation| {
            Message::PreviewScrollbarAutoHideElapsed { generation }
        })
    }

    pub(crate) fn handle_auto_hide_elapsed(&mut self, generation: u64) {
        self.scrollbar.start_hide(generation);
    }

    /// 帧推进：惯性位移 + 滚动条透明度（chrome 淡入淡出由宿主推进）。
    pub(crate) fn advance_frame(&mut self) -> Task<Message> {
        let scroll_task = self
            .smooth_scroll
            .next_frame_delta()
            .map(|(region, delta)| {
                iced::widget::operation::scroll_by(
                    preview_scroll_id(region),
                    scrollable::AbsoluteOffset {
                        x: delta.x,
                        y: delta.y,
                    },
                )
            });

        if !self.smooth_scroll.is_active() {
            self.smooth_scroll.stop();
        }
        self.scrollbar.advance_animation();

        scroll_task.unwrap_or_else(Task::none)
    }
}

/// 视口快照提取（on_scroll 回传闭包共用）。
pub(crate) fn scrollbar_viewport_from(viewport: &scrollable::Viewport) -> ScrollbarViewport {
    let absolute_offset = viewport.absolute_offset();
    let bounds = viewport.bounds();
    let content_bounds = viewport.content_bounds();
    ScrollbarViewport {
        offset_x: absolute_offset.x,
        offset_y: absolute_offset.y,
        viewport_width: bounds.width,
        viewport_height: bounds.height,
        content_width: content_bounds.width,
        content_height: content_bounds.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_scroll_ids_are_namespaced_away_from_picker_ids() {
        // portal spec 红线：iced operate 遍历所有窗口的控件树，预览
        // id 不得与选择窗（请求路径命名空间）或文本查看器固定 id 相撞。
        let picker_list = crate::picker_session::scrollbar::scroll_id(
            "/req/a",
            crate::picker_session::SessionScrollRegion::List,
        );
        let picker_breadcrumb = crate::picker_session::scrollbar::scroll_id(
            "/req/a",
            crate::picker_session::SessionScrollRegion::Breadcrumb,
        );
        for region in [
            PreviewScrollRegion::Directory,
            PreviewScrollRegion::Archive,
            PreviewScrollRegion::Document,
            PreviewScrollRegion::Text,
            PreviewScrollRegion::Markdown,
            PreviewScrollRegion::SqliteTables,
            PreviewScrollRegion::SqliteData,
        ] {
            let id = preview_scroll_id(region);
            assert_ne!(id, picker_list);
            assert_ne!(id, picker_breadcrumb);
            assert_ne!(id, crate::view::address_input_id("/req/a"));
        }
        // 七区域彼此也不相同。
        let all: Vec<_> = [
            PreviewScrollRegion::Directory,
            PreviewScrollRegion::Archive,
            PreviewScrollRegion::Document,
            PreviewScrollRegion::Text,
            PreviewScrollRegion::Markdown,
            PreviewScrollRegion::SqliteTables,
            PreviewScrollRegion::SqliteData,
        ]
        .iter()
        .map(|region| preview_scroll_id(*region))
        .collect();
        for (index, id) in all.iter().enumerate() {
            assert_eq!(
                all.iter().filter(|other| *other == id).count(),
                1,
                "第 {index} 个区域 id 重复"
            );
        }
    }

    #[test]
    fn layout_verified_reveals_only_on_overflow() {
        let mut pipeline = PreviewScrollPipeline::default();
        let fitting = ScrollbarViewport {
            viewport_height: 600.0,
            content_height: 600.0,
            ..overflowing()
        };
        drop(pipeline.handle_layout_verified(PreviewScrollRegion::Directory, fitting));
        assert_eq!(
            pipeline.visibility_for(PreviewScrollRegion::Directory),
            ScrollbarVisibility::Hidden
        );

        drop(pipeline.handle_layout_verified(PreviewScrollRegion::Directory, overflowing()));
        assert!(
            pipeline
                .visibility_for(PreviewScrollRegion::Directory)
                .opacity()
                > 0.0
        );
    }

    fn overflowing() -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 400.0,
            viewport_height: 300.0,
            content_width: 400.0,
            content_height: 900.0,
        }
    }
}
