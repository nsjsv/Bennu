use iced::widget::scrollable;
use iced::{Point, Rectangle, Size, Task};

use super::smooth_scroll::smooth_scroll_id;
use super::FileBrowser;
use crate::model::{BrowserPaneLayout, BrowserViewMode, Message, ScrollbarRegion};

// 60Hz 帧循环驱动,光标静止压在边缘也能持续滚动;越贴近边缘越快。
const EDGE_SCROLL_ZONE: f32 = 44.0;
const EDGE_SCROLL_MAX_STEP: f32 = 24.0;

/// 内部文件拖拽期间(应用内拖拽或原生拖放皆可),光标压近浏览区边缘
/// 时每帧推进的滚动计划。应用内拖拽由指针移动重算,原生拖放由目标
/// Moved 事件重算;任何拖拽终态都必须清掉计划,防止无事件后残留滚动。
#[derive(Debug, Clone)]
pub(crate) struct FileDragEdgeScroll {
    region: ScrollbarRegion,
    offset: scrollable::AbsoluteOffset,
}

impl FileBrowser {
    pub(crate) fn file_drag_edge_scroll_is_active(&self) -> bool {
        self.file_drag_edge_scroll.is_some()
    }

    pub(crate) fn stop_file_drag_edge_scroll(&mut self) {
        self.file_drag_edge_scroll = None;
    }

    /// 每次指针移动(应用内拖拽)或原生 Moved 事件重算;拖拽未激活或
    /// 光标不在边缘带内时清除计划,任何拖拽结束路径之后都不会残留滚动。
    pub(crate) fn update_file_drag_edge_scroll(&mut self, position: Point) {
        self.file_drag_edge_scroll = self.file_drag_edge_scroll_at(position);
    }

    pub(crate) fn advance_file_drag_edge_scroll(&mut self) -> Task<Message> {
        let Some(edge_scroll) = self.file_drag_edge_scroll.clone() else {
            return Task::none();
        };
        // 滚动平移内容即落点快照软失效:滚动与后台重测同帧批量下发,
        // 新快照到达后高亮按当前指针位置重算,松手 freeze 始终有快照可用。
        Task::batch([
            iced::widget::operation::scroll_by(
                smooth_scroll_id(&edge_scroll.region),
                edge_scroll.offset,
            ),
            self.remeasure_file_drop_layout_after_scroll(),
        ])
    }

    fn file_drag_edge_scroll_at(&self, position: Point) -> Option<FileDragEdgeScroll> {
        // 外部拖入(悬停本窗口)没有 file_drag 状态,自然不参与;
        // 内部拖拽无论走应用内通道还是原生通道都规划。
        let file_drag = self.file_drag.as_ref()?;
        if !file_drag.is_dragging() {
            return None;
        }
        let pane_id = self.pane_id_at_position(position)?;
        let pane_bounds = self.pane_bounds(pane_id)?;
        let (region, offset) = match self.view_mode {
            BrowserViewMode::Columns => {
                let (direction, step) = edge_approach(
                    position.x,
                    (pane_bounds.x, pane_bounds.x + pane_bounds.width),
                )?;
                (
                    ScrollbarRegion::ColumnBrowser(pane_id),
                    scrollable::AbsoluteOffset {
                        x: direction * step,
                        y: 0.0,
                    },
                )
            }
            BrowserViewMode::List | BrowserViewMode::Icons => {
                let (direction, step) = edge_approach(
                    position.y,
                    (pane_bounds.y, pane_bounds.y + pane_bounds.height),
                )?;
                let region = match self.view_mode {
                    BrowserViewMode::Icons => ScrollbarRegion::PaneIcons(pane_id),
                    _ => ScrollbarRegion::PaneList(pane_id),
                };
                (
                    region,
                    scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: direction * step,
                    },
                )
            }
        };
        Some(FileDragEdgeScroll { region, offset })
    }

    /// 与 `pane_id_at_position` 同源的纯布局推导,给出单个窗格的屏幕矩形。
    pub(crate) fn pane_bounds(&self, pane_id: super::BrowserPaneId) -> Option<Rectangle> {
        let x = self.sidebar_width;
        let width = self.split_content_width();
        let y = self.main_panes_area_top();
        let height = (self.main_window_height - y).max(1.0);
        let full = Rectangle::new(Point::new(x, y), Size::new(width, height));
        match self.pane_layout {
            BrowserPaneLayout::Single { active } => (pane_id == active).then_some(full),
            BrowserPaneLayout::Split {
                axis: crate::model::SplitAxis::Horizontal,
                first,
                second,
                ..
            } => {
                let boundary = self.pane_layout.split_divider_center(width);
                let first_bounds = Rectangle::new(full.position(), Size::new(boundary, height));
                let second_bounds = Rectangle::new(
                    Point::new(x + boundary, y),
                    Size::new((width - boundary).max(1.0), height),
                );
                split_pane_bounds(pane_id, first, second, first_bounds, second_bounds)
            }
            BrowserPaneLayout::Split {
                axis: crate::model::SplitAxis::Vertical,
                first,
                second,
                ..
            } => {
                let boundary = self.pane_layout.split_divider_center(height);
                let first_bounds = Rectangle::new(full.position(), Size::new(width, boundary));
                let second_bounds = Rectangle::new(
                    Point::new(x, y + boundary),
                    Size::new(width, (height - boundary).max(1.0)),
                );
                split_pane_bounds(pane_id, first, second, first_bounds, second_bounds)
            }
        }
    }
}

fn split_pane_bounds(
    pane_id: super::BrowserPaneId,
    first: super::BrowserPaneId,
    second: super::BrowserPaneId,
    first_bounds: Rectangle,
    second_bounds: Rectangle,
) -> Option<Rectangle> {
    if pane_id == first {
        Some(first_bounds)
    } else if pane_id == second {
        Some(second_bounds)
    } else {
        None
    }
}

/// 光标靠近 min 端返回 -1、靠近 max 端返回 +1,步长按贴近程度线性渐强;
/// 不在边缘带内返回 None。
fn edge_approach(position: f32, (min, max): (f32, f32)) -> Option<(f32, f32)> {
    let from_min = position - min;
    let from_max = max - position;
    let (direction, distance) = if from_min <= from_max {
        (-1.0, from_min)
    } else {
        (1.0, from_max)
    };
    (distance < EDGE_SCROLL_ZONE).then(|| {
        let strength = 1.0 - (distance / EDGE_SCROLL_ZONE).clamp(0.0, 1.0);
        (direction, strength * EDGE_SCROLL_MAX_STEP)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileDragPhase, FileDragState, FileDragStationaryAction};

    #[test]
    fn edge_approach_scales_step_with_proximity() {
        // 带外不滚。
        assert_eq!(edge_approach(200.0, (0.0, 400.0)), None);
        // 贴 min 端向负方向,越贴越快。
        let (direction, step) = edge_approach(5.0, (0.0, 400.0)).unwrap();
        assert_eq!(direction, -1.0);
        assert!(step > 0.0 && step < EDGE_SCROLL_MAX_STEP);
        let (_, pinned_step) = edge_approach(0.0, (0.0, 400.0)).unwrap();
        assert!((pinned_step - EDGE_SCROLL_MAX_STEP).abs() < f32::EPSILON);
        // 贴 max 端向正方向。
        let (direction, _) = edge_approach(398.0, (0.0, 400.0)).unwrap();
        assert_eq!(direction, 1.0);
    }

    #[test]
    fn edge_plan_points_up_near_top_in_list_view() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.file_drag = Some(FileDragState {
            gesture_id: crate::model::FileDragGestureId(1),
            source_pane_id: browser.active_pane_id(),
            source_tab_id: browser.active_tab_id,
            sources: vec![std::path::PathBuf::from("/tmp/a.txt")],
            pressed_path: std::path::PathBuf::from("/tmp/a.txt"),
            bookmark_source: None,
            stationary_action: FileDragStationaryAction::SelectionOnly,
            phase: FileDragPhase::Dragging,
            native_dnd: crate::model::FileDragNativeDndState::NotRequested,
            column_directories_snapshot: Vec::new(),
            press_origin: iced::Point::ORIGIN,
            preview_entries: Vec::new(),
            wayland_drag_icon: None,
        });

        browser.update_file_drag_edge_scroll(Point::new(
            browser.sidebar_width + 20.0,
            browser.main_panes_area_top() + 5.0,
        ));

        let plan = browser
            .file_drag_edge_scroll
            .clone()
            .expect("plan expected");
        assert!(matches!(plan.region, ScrollbarRegion::PaneList(_)));
        assert!(plan.offset.y < 0.0 && plan.offset.x == 0.0);

        // 拖拽结束后不允许残留滚动计划。
        browser.stop_file_drag_edge_scroll();
        assert!(browser.file_drag_edge_scroll.is_none());
    }

    #[test]
    fn edge_plan_survives_native_drag_session() {
        // 原生拖放的 Moved 事件负责重算计划,原生会话本身不再阻断规划;
        // 应用内拖拽与原生拖放共用同一套边缘自动滚。
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        let session_id = desktop_linux::WaylandDndController::new()
            .start_file_drag(
                vec![std::path::PathBuf::from("/tmp/a.txt")],
                desktop_linux::WaylandFileDragIcon::new(1, 1, vec![0, 0, 0, 255])
                    .expect("test icon"),
            )
            .expect("source session");
        browser.file_drag = Some(FileDragState {
            gesture_id: crate::model::FileDragGestureId(1),
            source_pane_id: browser.active_pane_id(),
            source_tab_id: browser.active_tab_id,
            sources: vec![std::path::PathBuf::from("/tmp/a.txt")],
            pressed_path: std::path::PathBuf::from("/tmp/a.txt"),
            bookmark_source: None,
            stationary_action: FileDragStationaryAction::SelectionOnly,
            phase: FileDragPhase::Dragging,
            native_dnd: crate::model::FileDragNativeDndState::Started(session_id),
            column_directories_snapshot: Vec::new(),
            press_origin: iced::Point::ORIGIN,
            preview_entries: Vec::new(),
            wayland_drag_icon: None,
        });

        browser.update_file_drag_edge_scroll(Point::new(
            browser.sidebar_width + 20.0,
            browser.main_panes_area_top() + 5.0,
        ));

        assert!(browser.file_drag_edge_scroll.is_some());
    }
}
