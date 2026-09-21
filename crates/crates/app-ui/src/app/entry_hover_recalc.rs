//! hover 所有权补偿:hovered_entry 恒等于"光标正下方那个条目",但
//! 滚动/虚拟窗口切换只是平移行的 bounds,iced 的 mouse_area 按 bounds
//! 门控重算,不会为静止的光标补发 enter/exit——高亮会残留在旧行。
//! 列表/大图的滚动帧(平滑滚动惯性逐帧触发 Scrolled)在这里用 iced
//! 实测的可视区矩形把存量光标位置(`cursor_position`,拖拽位图与框选
//! 共用的同一事实源)换算成内容落点,按渲染同源的几何重算 hover。
//! 指针移动产生的 enter/exit 仍是主通道,本模块只是逐帧补偿路径。

use std::path::{Path, PathBuf};

use iced::{Point, Rectangle};

use super::FileBrowser;
use crate::model::BrowserPaneId;

impl FileBrowser {
    pub(super) fn recalculate_hovered_entry_after_list_scroll(
        &mut self,
        pane_id: BrowserPaneId,
        offset_y: f32,
        viewport: Rectangle,
    ) {
        if !self.hover_recalc_gate(pane_id, viewport) {
            return;
        }
        let point = cursor_content_point(self.cursor_position, viewport, offset_y);
        let (next, rail_visible) = {
            let geometry =
                crate::list_view::ListGeometry::for_level(self.user_config().list_view_density);
            let Some(pane) = self.pane_view(pane_id) else {
                return;
            };
            // 与列表渲染同一条合并行流:命中高度、表头留量、组头/占位
            // 行的跳过规则全部来自它,不另算一套偏移。
            let rows = crate::transfer_placeholder_view::build_list_transfer_rows(
                self,
                pane,
                geometry.row_height,
            );
            let rail_visible = crate::model::file_group_rail_visible(
                &crate::transfer_placeholder_view::list_file_group_rail_entries(
                    &rows,
                    geometry.row_height,
                ),
            );
            let next = crate::transfer_placeholder_view::list_entry_path_at_point(
                &rows,
                geometry.row_height,
                crate::list_view::LIST_HEADER_HEIGHT,
                point,
            )
            .map(Path::to_path_buf);
            (next, rail_visible)
        };
        if self.rail_barrier_covers_cursor(pane_id, rail_visible) {
            // 光标仍压在索引栏上:栏外无条目可指,维持事件通道留下的
            // None(进入栏时已显式清过)。
            return;
        }
        self.apply_recalculated_hover(next);
    }

    pub(super) fn recalculate_hovered_entry_after_icon_grid_scroll(
        &mut self,
        pane_id: BrowserPaneId,
        offset_y: f32,
        viewport: Rectangle,
    ) {
        if !self.hover_recalc_gate(pane_id, viewport) {
            return;
        }
        let point = cursor_content_point(self.cursor_position, viewport, offset_y);
        let (next, rail_visible) = {
            let Some(pane) = self.pane_view(pane_id) else {
                return;
            };
            let layout = self.icon_grid_layout_for_pane(pane);
            // 网格无表头,flow 段顶点即内容偏移;命中几何与渲染瓦片同源。
            let rail_visible =
                crate::model::file_group_rail_visible(&layout.root().file_group_rail_entries());
            let next = layout.entry_path_at_point(point).map(Path::to_path_buf);
            (next, rail_visible)
        };
        if self.rail_barrier_covers_cursor(pane_id, rail_visible) {
            return;
        }
        self.apply_recalculated_hover(next);
    }

    /// 索引栏事件屏障的门控:栏已随分组关闭/目录切换消失时,屏障事实
    /// 过期、就地自愈清除;栏仍在且光标在栏上时,滚动补偿不得越过屏障
    /// 去命中下层条目。
    fn rail_barrier_covers_cursor(&mut self, pane_id: BrowserPaneId, rail_visible: bool) -> bool {
        if !rail_visible {
            self.hovered_grouping_rail_pane = None;
            return false;
        }
        self.hovered_grouping_rail_pane == Some(pane_id)
    }

    /// 补偿路径的统一门槛:只服务活动窗格(hovered_entry 是活动窗格的
    /// 单一状态,非活动窗格滚动不参与);拖拽期间 hover 由拖拽落点通道
    /// 所有,不在此混写;光标没有落在这个视口里 = 该 pane 无光标记录
    /// (含光标初值 (0,0) 的陈旧记录),不动作,绝不误清事件通道的结果。
    fn hover_recalc_gate(&self, pane_id: BrowserPaneId, viewport: Rectangle) -> bool {
        pane_id == self.active_pane_id()
            && self.file_drag.is_none()
            && self.file_drop_session.is_none()
            && viewport.contains(self.cursor_position)
    }

    /// 等值守卫风格:重算只写"光标正下方条目"这一个事实,值未变化时
    /// 不产生任何状态扰动。
    fn apply_recalculated_hover(&mut self, next: Option<PathBuf>) {
        if self.hovered_entry != next {
            self.hovered_entry = next;
        }
    }
}

/// 窗口坐标光标 → 滚动内容坐标:viewport 是 iced on_scroll 实测的
/// 可视区窗口矩形,内容偏移加上视口内偏移即落点,滚动帧间无陈旧量。
fn cursor_content_point(cursor: Point, viewport: Rectangle, offset_y: f32) -> Point {
    Point::new(cursor.x - viewport.x, cursor.y - viewport.y + offset_y)
}

#[cfg(test)]
#[path = "entry_hover_recalc_tests.rs"]
mod tests;
