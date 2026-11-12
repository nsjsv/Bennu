//! 预览窗滚动管线：七个滚动区域（目录树/归档树/分页文档/纯文本/
//! Markdown/SQLite 表列表/SQLite 数据网格）的滚轮惯性。滚动条由
//! `bennu_theme::scrollbar` 按当帧布局常驻绘制，这里不保存视口或显隐
//! 状态。widget id 用 portal-preview-* 命名空间，与选择窗的
//! portal-file-list#/portal-breadcrumb# 前缀互不串窗。

use bennu_theme::smooth_scroll::{
    wheel_delta_for_axis, MosScrollState, SmoothScrollAxis, WheelScrollMode,
};
use iced::widget::scrollable;
use iced::{mouse, Task};

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

/// 预览窗滚轮惯性，归 PreviewHost 持有。
#[derive(Debug, Default)]
pub(crate) struct PreviewScrollPipeline {
    pub(crate) smooth_scroll: MosScrollState<PreviewScrollRegion>,
}

impl PreviewScrollPipeline {
    /// 帧时钟是否需要挂载（惯性滚动进行中）。
    pub(crate) fn is_animating(&self) -> bool {
        self.smooth_scroll.is_active()
    }

    /// 会话结束整体复位（新预览会话不继承旧会话的惯性）。
    pub(crate) fn reset(&mut self) {
        self.smooth_scroll.stop();
    }

    /// 滚轮输入：Lines 走 Mos 惯性累积；Pixels（触控板）停止惯性直接
    /// 位移（与选择窗滚轮处理同式）。
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
                Task::none()
            }
            WheelScrollMode::Direct => {
                self.smooth_scroll.stop();
                iced::widget::operation::scroll_by(
                    preview_scroll_id(region),
                    scrollable::AbsoluteOffset {
                        x: wheel.delta.x,
                        y: wheel.delta.y,
                    },
                )
            }
        }
    }

    /// 帧推进：惯性位移（chrome 淡入淡出由宿主推进）。
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

        scroll_task.unwrap_or_else(Task::none)
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
}
