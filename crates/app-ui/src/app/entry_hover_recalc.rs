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

/// 滚动的是哪种视图:两者只有命中几何不同,门槛、屏障判断和写入是同一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrolledView {
    List,
    IconGrid,
}

/// 内容落点下的条目,以及该视图此刻是否带分组索引栏。
struct ContentHit {
    entry: Option<PathBuf>,
    rail_visible: bool,
}

impl FileBrowser {
    pub(super) fn recalculate_hovered_entry_after_scroll(
        &mut self,
        pane_id: BrowserPaneId,
        view: ScrolledView,
        offset_y: f32,
        viewport: Rectangle,
    ) {
        let Some(cursor) = self.hover_recalc_cursor(pane_id, viewport) else {
            return;
        };
        let Some(hit) = self.content_hit(
            pane_id,
            view,
            cursor_content_point(cursor, viewport, offset_y),
        ) else {
            return;
        };
        // 光标仍压在索引栏上:栏外无条目可指,维持事件通道留下的 None
        // (进入栏时已显式清过)。栏消失时屏障由条目/分组变化的转换清除。
        if hit.rail_visible && self.hovered_grouping_rail_pane == Some(pane_id) {
            return;
        }
        // 等值守卫风格:重算只写"光标正下方条目"这一个事实,值未变化时
        // 不产生任何状态扰动。
        if self.hovered_entry != hit.entry {
            self.hovered_entry = hit.entry;
        }
    }

    fn content_hit(
        &self,
        pane_id: BrowserPaneId,
        view: ScrolledView,
        point: Point,
    ) -> Option<ContentHit> {
        let pane = self.pane_view(pane_id)?;
        Some(match view {
            ScrolledView::List => {
                let geometry =
                    crate::list_view::ListGeometry::for_level(self.user_config().list_view_density);
                // 与列表渲染同一条合并行流:命中高度、表头留量、组头/占位
                // 行的跳过规则全部来自它,不另算一套偏移。
                let rows = crate::transfer_placeholder_view::build_list_transfer_rows(
                    self,
                    pane,
                    geometry.row_height,
                );
                ContentHit {
                    rail_visible: crate::model::file_group_rail_visible(
                        &crate::transfer_placeholder_view::list_file_group_rail_entries(
                            &rows,
                            geometry.row_height,
                        ),
                    ),
                    entry: crate::transfer_placeholder_view::list_entry_path_at_point(
                        &rows,
                        geometry.row_height,
                        crate::list_view::LIST_HEADER_HEIGHT,
                        point,
                    )
                    .map(Path::to_path_buf),
                }
            }
            ScrolledView::IconGrid => {
                let layout = self.icon_grid_layout_for_pane(pane);
                // 网格无表头,flow 段顶点即内容偏移;命中几何与渲染瓦片同源。
                ContentHit {
                    rail_visible: crate::model::file_group_rail_visible(
                        &layout.root().file_group_rail_entries(),
                    ),
                    entry: layout.entry_path_at_point(point).map(Path::to_path_buf),
                }
            }
        })
    }

    /// 补偿路径的统一门槛:只服务活动窗格(hovered_entry 是活动窗格的
    /// 单一状态,非活动窗格滚动不参与);拖拽期间 hover 由拖拽落点通道
    /// 所有,不在此混写;光标未知或没有落在这个视口里则不动作,绝不误清
    /// 事件通道的结果。通过时返回用于换算的光标位置。
    fn hover_recalc_cursor(&self, pane_id: BrowserPaneId, viewport: Rectangle) -> Option<Point> {
        if pane_id != self.active_pane_id()
            || self.file_drag.is_some()
            || self.file_drop_session.is_some()
        {
            return None;
        }
        self.cursor_position
            .filter(|cursor| viewport.contains(*cursor))
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
