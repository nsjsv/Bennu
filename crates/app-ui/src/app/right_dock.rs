//! 右侧停靠区在主窗口里：开关、拖宽、格间分隔线、拖面板换位置、右键菜单、写盘，以及格子的
//! 几何（预览视口、拖放落点、分隔线都按它算）。布局本身的规则在 [`crate::right_dock`]。

use iced::{Point, Rectangle, Size, Task};

use super::FileBrowser;
use crate::model::{ContextMenuState, DockMenuPanelSection, Message, RightDockMenuState};
use crate::right_dock::{DockPanel, DropTarget, PORTION_SCALE};

/// 左缘拖宽带的宽度。
pub(crate) const RESIZE_HANDLE_WIDTH: f32 = 6.0;
/// 格间分隔线（整条是拖动热区）的高度，跟窗格分隔线一样。
pub(crate) const DIVIDER_HEIGHT: f32 = crate::model::SPLIT_DIVIDER_WIDTH;
/// 标题栏、标签条的高度。
pub(crate) const HEADER_HEIGHT: f32 = 28.0;
/// 每格内容区（标题栏、标签条下面）至少这么高：份额分高度和拖分隔线都不会把一格压得更矮。
const MIN_GROUP_CONTENT_HEIGHT: f32 = 80.0;
/// 停靠区最宽：窗格区至少留一栏宽。
const MIN_BROWSER_AREA_WIDTH: f32 = crate::config::MIN_COLUMN_WIDTH;
/// 按住面板标题移动这么远才算拖（跟标签一样）。
const PANEL_DRAG_ACTIVATION_DISTANCE: f32 = 3.0;
/// 拖到某格的上下这么多（按格高的比例）是在它上面、下面另起一格，中间是叠进去。
const EDGE_DROP_FRACTION: f32 = 0.25;

#[derive(Debug, Clone)]
pub(crate) enum RightDockMessage {
    /// 工具栏按钮、Alt+P：显示或隐藏整个右侧栏。
    VisibilityToggled,
    WidthDragStarted,
    /// 第几格下面那条分隔线。
    DividerDragStarted(usize),
    /// 按下了面板的标签或标题栏：锁定时切过去；解锁时可能是点，也可能开始拖。
    PanelPressed(DockPanel),
    PanelClosed(DockPanel),
    /// 右键菜单里点了某个面板：开着就关，关着就开。
    PanelToggled(DockPanel),
    LockToggled,
    MenuRequested,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RightDockDrag {
    Width {
        cursor_start_x: f32,
        width_start: f32,
    },
    /// 上下两格各自的最小高度之外，还有 `room` 高度能在两格之间挪。
    Divider {
        upper: usize,
        cursor_start_y: f32,
        upper_start: f32,
        upper_min: f32,
        room: f32,
    },
    Panel {
        panel: DockPanel,
        origin: Point,
        dragging: bool,
        target: Option<DropTarget>,
    },
}

/// 一格在窗口里的位置，和它标题栏（或标签条）的高度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GroupFrame {
    pub(crate) bounds: Rectangle,
    pub(crate) header_height: f32,
}

impl FileBrowser {
    pub(super) fn handle_right_dock_message(&mut self, message: RightDockMessage) -> Task<Message> {
        match message {
            RightDockMessage::VisibilityToggled => {
                self.right_dock.toggle_visible();
                return self.right_dock_changed();
            }
            RightDockMessage::WidthDragStarted => {
                self.right_dock_drag = Some(RightDockDrag::Width {
                    cursor_start_x: self.cursor_or_origin().x,
                    width_start: self.right_dock.layout().width(),
                });
            }
            RightDockMessage::DividerDragStarted(upper) => {
                let frames = self.right_dock_group_frames();
                if let (Some(top), Some(bottom)) = (frames.get(upper), frames.get(upper + 1)) {
                    let upper_min = min_group_height(top.header_height);
                    let room = top.bounds.height + bottom.bounds.height
                        - upper_min
                        - min_group_height(bottom.header_height);
                    // 栏矮到两格都只剩平分的高度时，份额不影响画出来的高度，没有可拖的。
                    if room > 0.0 {
                        self.right_dock_drag = Some(RightDockDrag::Divider {
                            upper,
                            cursor_start_y: self.cursor_or_origin().y,
                            upper_start: top.bounds.height,
                            upper_min,
                            room,
                        });
                    }
                }
            }
            RightDockMessage::PanelPressed(panel) => {
                if self.right_dock.layout().is_locked() {
                    self.right_dock.select(panel);
                    return self.right_dock_changed();
                }
                self.right_dock_drag = Some(RightDockDrag::Panel {
                    panel,
                    origin: self.cursor_or_origin(),
                    dragging: false,
                    target: None,
                });
            }
            RightDockMessage::PanelClosed(panel) => {
                self.right_dock.close(panel);
                return self.right_dock_changed();
            }
            // 菜单里的勾是「在栏里」：在就关，不在就开（藏在别的标签后面的也算在）。
            RightDockMessage::PanelToggled(panel) => {
                self.context_menu = None;
                if self.right_dock.layout().contains(panel) {
                    self.right_dock.close(panel);
                } else {
                    self.right_dock.open(panel);
                }
                return self.right_dock_changed();
            }
            RightDockMessage::LockToggled => {
                self.context_menu = None;
                self.right_dock.toggle_locked();
                return self.right_dock_changed();
            }
            RightDockMessage::MenuRequested => {
                self.context_menu = Some(self.right_dock_context_menu(None));
            }
        }
        // 拖动开始时别的指针手势让路，跟侧栏拖宽一样。
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        Task::none()
    }

    /// 右键停靠区弹出的菜单；`panel_section` 是从某个面板里右键时多出的那一段。
    pub(super) fn right_dock_context_menu(
        &self,
        panel_section: Option<DockMenuPanelSection>,
    ) -> ContextMenuState {
        let layout = self.right_dock.layout();
        ContextMenuState::RightDock(RightDockMenuState {
            position: self.cursor_or_origin(),
            open_panels: DockPanel::ALL
                .into_iter()
                .filter(|panel| layout.contains(*panel))
                .collect(),
            locked: layout.is_locked(),
            shows_hovered_item: self.user_config.dock_shows_hovered_item,
            panel_section,
        })
    }

    /// 快捷键、右键菜单：开或关一个面板（栏隐藏时打开它会把栏一起显示）。
    pub(crate) fn toggle_dock_panel(&mut self, panel: DockPanel) -> Task<Message> {
        self.right_dock.toggle_panel(panel);
        self.right_dock_changed()
    }

    pub(crate) fn toggle_right_dock_lock(&mut self) -> Task<Message> {
        self.right_dock.toggle_locked();
        self.right_dock_changed()
    }

    pub(crate) fn toggle_right_dock(&mut self) -> Task<Message> {
        self.handle_right_dock_message(RightDockMessage::VisibilityToggled)
    }

    /// 布局改完：写盘；栏的宽度、显示与否会改窗格区宽度，视口跟着收。
    fn right_dock_changed(&mut self) -> Task<Message> {
        // 任务面板显示出来就收起右下角弹出的任务列表：两处一样的东西不同时开着。
        if self.right_dock.layout().shows(DockPanel::TaskLog) {
            self.operation_queue.close_panel();
        }
        self.user_config.right_dock = self.right_dock.layout().clone();
        Task::batch([
            self.persist_user_preferences_command(),
            self.clamp_visible_pane_viewports(),
        ])
    }

    /// 拖宽改窗格区宽度，视口要逐帧跟着收；分隔线、拖面板不影响窗格。
    pub(super) fn right_dock_width_drag_is_active(&self) -> bool {
        matches!(self.right_dock_drag, Some(RightDockDrag::Width { .. }))
    }

    /// 全局指针移动推进拖动。
    pub(super) fn update_right_dock_drag(&mut self, position: Point) {
        let Some(drag) = self.right_dock_drag else {
            return;
        };
        match drag {
            RightDockDrag::Width {
                cursor_start_x,
                width_start,
            } => {
                let width =
                    self.right_dock_width_for_window(width_start - (position.x - cursor_start_x));
                self.right_dock.set_width(width);
            }
            RightDockDrag::Divider {
                upper,
                cursor_start_y,
                upper_start,
                upper_min,
                room,
            } => {
                // 份额按「最小高度之外的部分」分，所以上面那格在 room 里占的比例就是它在两格份额
                // 里占的比例；指针走多少像素分隔线就走多少。
                let upper_height =
                    (upper_start + position.y - cursor_start_y).clamp(upper_min, upper_min + room);
                self.right_dock
                    .set_split(upper, (upper_height - upper_min) / room);
            }
            RightDockDrag::Panel {
                panel,
                origin,
                dragging,
                ..
            } => {
                let moved = position.distance(origin) >= PANEL_DRAG_ACTIVATION_DISTANCE;
                let dragging = dragging || moved;
                self.right_dock_drag = Some(RightDockDrag::Panel {
                    panel,
                    origin,
                    dragging,
                    target: dragging
                        .then(|| self.right_dock_drop_target_at(position))
                        .flatten(),
                });
            }
        }
    }

    /// 全局左键松开：拖宽、分隔线写盘；拖面板放下（没拖起来就是点了这个标签）。
    pub(super) fn finish_right_dock_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.right_dock_drag.take() else {
            return Task::none();
        };
        if let RightDockDrag::Panel {
            panel,
            dragging,
            target,
            ..
        } = drag
        {
            match (dragging, target) {
                (false, _) => self.right_dock.select(panel),
                (true, Some(target)) => self.right_dock.move_panel(panel, target),
                // 拖出了停靠区：取消。
                (true, None) => return Task::none(),
            }
        }
        self.right_dock_changed()
    }

    /// 正拖着面板时它和落点：视图画落点提示。
    pub(crate) fn right_dock_panel_drop(&self) -> Option<(DockPanel, DropTarget)> {
        match self.right_dock_drag {
            Some(RightDockDrag::Panel {
                panel,
                dragging: true,
                target: Some(target),
                ..
            }) => Some((panel, target)),
            _ => None,
        }
    }

    /// 运行时生效的宽度：存的宽度再按当前窗口收；栏和窗格同排，窗格区至少留一栏宽。
    pub(crate) fn right_dock_effective_width(&self) -> f32 {
        self.right_dock_width_for_window(self.right_dock.layout().width())
    }

    fn right_dock_width_for_window(&self, width: f32) -> f32 {
        let max_width = (self.main_content_width() - MIN_BROWSER_AREA_WIDTH).max(1.0);
        crate::config::normalize_right_dock_width(width).min(max_width)
    }

    /// 栏显示时占掉的宽度，没显示是 0。
    pub(crate) fn right_dock_layout_width(&self) -> f32 {
        if self.right_dock.layout().is_visible() {
            self.right_dock_effective_width()
        } else {
            0.0
        }
    }

    /// 每格在窗口里的位置，从上到下：渲染、预览视口、拖放落点、分隔线拖动共用的唯一一份。
    /// 栏从窗格区顶上开始，到终端抽屉上沿为止，贴着窗口右边；高度见 [`group_heights`]。
    pub(crate) fn right_dock_group_frames(&self) -> Vec<GroupFrame> {
        let layout = self.right_dock.layout();
        let groups = layout.groups();
        let top = self.main_panes_area_top();
        let bottom =
            self.main_window_height - crate::terminal_panel::view::terminal_panel_area_height(self);
        let dividers = DIVIDER_HEIGHT * groups.len().saturating_sub(1) as f32;
        let available = (bottom - top - dividers).max(0.0);
        let portions_and_headers: Vec<(u16, f32)> = groups
            .iter()
            .map(|group| {
                (
                    group.portion(),
                    header_height(layout.is_locked(), group.panels().len()),
                )
            })
            .collect();
        let heights = group_heights(available, &portions_and_headers);
        let width = self.right_dock_effective_width();
        let left = self.main_window_width - width;
        let mut y = top;
        portions_and_headers
            .iter()
            .zip(heights)
            .map(|(&(_, header), height)| {
                let frame = GroupFrame {
                    bounds: Rectangle::new(Point::new(left, y), Size::new(width, height)),
                    header_height: header,
                };
                y += height + DIVIDER_HEIGHT;
                frame
            })
            .collect()
    }

    /// 某个面板那格的内容区（标题栏下面）；面板不在栏里时是 None。
    pub(crate) fn right_dock_panel_area(&self, panel: DockPanel) -> Option<Rectangle> {
        let index = self
            .right_dock
            .layout()
            .groups()
            .iter()
            .position(|group| group.panels().contains(&panel))?;
        let frame = self.right_dock_group_frames().get(index).copied()?;
        let mut area = frame.bounds;
        area.y += frame.header_height;
        area.height = (area.height - frame.header_height).max(0.0);
        Some(area)
    }

    /// 指针在哪格的上 1/4、下 1/4（另起一格）或中间（叠进去）；出了停靠区是 None。
    fn right_dock_drop_target_at(&self, position: Point) -> Option<DropTarget> {
        let frames = self.right_dock_group_frames();
        let first = frames.first()?;
        if position.x < first.bounds.x {
            return None;
        }
        let index = frames
            .iter()
            .position(|frame| position.y < frame.bounds.y + frame.bounds.height)
            .unwrap_or(frames.len() - 1);
        let bounds = frames[index].bounds;
        let fraction = ((position.y - bounds.y) / bounds.height.max(1.0)).clamp(0.0, 1.0);
        Some(if fraction < EDGE_DROP_FRACTION {
            DropTarget::Above(index)
        } else if fraction > 1.0 - EDGE_DROP_FRACTION {
            DropTarget::Below(index)
        } else {
            DropTarget::Join(index)
        })
    }
}

/// 一格至少这么高：标题栏（或标签条）加上内容区的最小高度。
fn min_group_height(header_height: f32) -> f32 {
    header_height + MIN_GROUP_CONTENT_HEIGHT
}

/// 各格的像素高度（`available` 已去掉分隔线），元素是（份额，标题栏高度）。每格先拿最小高度，
/// 剩下的按份额分；剩的不够（栏太矮）时不看份额，各格平分，不让哪一格被挤没。份额合计是
/// [`PORTION_SCALE`]，所以高度合计正好是 `available`。
fn group_heights(available: f32, portions_and_headers: &[(u16, f32)]) -> Vec<f32> {
    let minimums: Vec<f32> = portions_and_headers
        .iter()
        .map(|&(_, header)| min_group_height(header))
        .collect();
    let spare = available - minimums.iter().sum::<f32>();
    if spare <= 0.0 {
        return vec![available / portions_and_headers.len() as f32; portions_and_headers.len()];
    }
    portions_and_headers
        .iter()
        .zip(minimums)
        .map(|(&(portion, _), minimum)| {
            minimum + spare * f32::from(portion) / f32::from(PORTION_SCALE)
        })
        .collect()
}

/// 锁定时只有一个面板的格子没有标题栏；叠了几个时总有标签条；解锁时总有标题栏。
pub(crate) fn header_height(locked: bool, panel_count: usize) -> f32 {
    if locked && panel_count == 1 {
        0.0
    } else {
        HEADER_HEIGHT
    }
}

#[cfg(test)]
#[path = "right_dock_tests.rs"]
mod tests;
