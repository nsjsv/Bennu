//! 终端标签:数据模型、纯逻辑(重排/相邻激活)与 [`FileBrowser`] 上的标签管理。
//!
//! 拖拽复刻主区标签的状态机:按下先激活,移动超过激活距离才算拖拽,
//! 拖到其他标签上立即换位,被挤开的标签播放位移动画。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::{Point, Task};

use super::session::{SessionId, TerminalSession};
use super::{
    FileBrowser, MIN_PANEL_HEIGHT, PANEL_HORIZONTAL_PADDING, TERMINAL_TAB_DRAG_ACTIVATION_DISTANCE,
};
use crate::animation::{ease_out_cubic, elapsed_fraction};
use crate::model::Message;

/// 单个终端标签:一个 PTY 会话 + 标签条展示所需的元数据。
pub(crate) struct TerminalTab {
    pub(crate) session: TerminalSession,
    /// 标签当前目录:启动目录,或最近一次跟随注入的目标;驱动标签标题。
    pub(crate) directory: PathBuf,
    /// 非激活期间收到过输出,标签标题旁显示小圆点。
    pub(crate) has_unread_output: bool,
}

/// 终端标签拖拽状态;phase 语义与主区 [`crate::model::TabDragState`] 一致。
pub(crate) struct TerminalTabDrag {
    pub(crate) session_id: SessionId,
    pub(crate) phase: TerminalTabDragPhase,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TerminalTabDragPhase {
    WaitingForMovement { origin: Point },
    Dragging,
}

impl TerminalTabDrag {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, TerminalTabDragPhase::Dragging)
    }
}

/// 重排位移动画的时长;与主区标签一致。
const TAB_SHIFT_DURATION: Duration = Duration::from_millis(160);
/// 标签槽最小宽度,防止标签过多时位移动画距离失真。
const MIN_TAB_SLOT_WIDTH: f32 = 48.0;

/// 被挤开标签的位移动画;初值可以是负数(累计进行中的动画)。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TabShiftAnimation {
    started_at: Instant,
    initial_offset: f32,
}

impl TabShiftAnimation {
    fn new(initial_offset: f32) -> Self {
        Self {
            started_at: Instant::now(),
            initial_offset,
        }
    }

    pub(crate) fn shift_offset(self) -> f32 {
        self.initial_offset
            * (1.0 - ease_out_cubic(elapsed_fraction(self.started_at, TAB_SHIFT_DURATION)))
    }

    fn is_finished(self) -> bool {
        elapsed_fraction(self.started_at, TAB_SHIFT_DURATION) >= 1.0
    }
}

/// 拖拽换位后的会话顺序;被拖标签不存在或与目标相同时返回 `None`。
pub(crate) fn reordered_session_ids(
    ids: &[SessionId],
    dragged: SessionId,
    entered: SessionId,
) -> Option<Vec<SessionId>> {
    if dragged == entered {
        return None;
    }
    let dragged_index = ids.iter().position(|id| *id == dragged)?;
    let entered_index = ids.iter().position(|id| *id == entered)?;
    let mut reordered = ids.to_vec();
    let dragged_id = reordered.remove(dragged_index);
    reordered.insert(entered_index, dragged_id);
    Some(reordered)
}

/// 重排时需要位移动画的会话:拖动方向上被跨过的那段。
pub(crate) fn shifted_ids_for_reorder(
    ids: &[SessionId],
    dragged_index: usize,
    entered_index: usize,
) -> Vec<SessionId> {
    if dragged_index < entered_index {
        ids[dragged_index + 1..=entered_index].to_vec()
    } else {
        ids[entered_index..dragged_index].to_vec()
    }
}

/// 关闭后应激活的相邻会话:优先左邻,其次右邻(与主区标签一致)。
pub(crate) fn adjacent_session_id(ids: &[SessionId], closing: SessionId) -> Option<SessionId> {
    let closing_index = ids.iter().position(|id| *id == closing)?;
    ids[..closing_index]
        .iter()
        .rev()
        .chain(ids[closing_index + 1..].iter())
        .copied()
        .next()
}

impl FileBrowser {
    /// 新建终端标签:始终拉起新 shell;面板收起时先展开。
    pub(crate) fn add_terminal_tab(&mut self) -> Task<Message> {
        let directory = self.terminal_panel_spawn_directory();
        if !self.terminal_panel.expanded {
            let target = self.terminal_panel.clamp_height_for_window(
                self.terminal_panel.expanded_height,
                self.main_window_height,
            );
            self.terminal_panel.expanded_height = target;
            self.terminal_panel.focused = true;
            let spawned = self.spawn_terminal_tab_in_directory(directory, target);
            if spawned {
                self.terminal_panel.expanded = true;
                self.terminal_panel.start_height_animation(target);
                self.sync_terminal_panel_size();
            }
            return Task::none();
        }
        self.terminal_panel.focused = true;
        let height = self.terminal_panel.height.max(MIN_PANEL_HEIGHT);
        self.spawn_terminal_tab_in_directory(directory, height);
        // 标签数跨过 1↔2 边界时标签条出现/消失,画布高度变化,其余标签随之重算。
        self.sync_terminal_panel_size();
        Task::none()
    }

    /// 在指定目录拉起新标签并激活;失败且没有其他标签时返回 `false`。
    pub(super) fn spawn_terminal_tab_in_directory(
        &mut self,
        directory: PathBuf,
        rows_height: f32,
    ) -> bool {
        let columns = self.terminal_panel_grid_columns();
        // 行数按落位后的标签数取:第二个标签会让标签条出现,画布变矮。
        let tab_strip_visible =
            super::view::tab_strip_visible_for_tab_count(self.terminal_panel.tabs.len() + 1);
        let rows = super::terminal_panel_canvas_rows(rows_height, tab_strip_visible);
        let shell = self.terminal_panel_shell();
        let session_id = self.terminal_panel.next_session_id;
        self.terminal_panel.next_session_id += 1;

        match super::session::spawn_terminal_session(
            session_id,
            &shell,
            &directory,
            super::emulator_dimensions(columns, rows),
        ) {
            Ok(spawned) => {
                self.terminal_panel.tabs.push(TerminalTab {
                    session: spawned,
                    directory: directory.clone(),
                    has_unread_output: false,
                });
                self.terminal_panel.active_session_id = Some(session_id);
                // 新 shell 已在目标目录,快照同步,避免紧接着注入一次 cd。
                self.terminal_panel.follow_target_snapshot = Some(directory);
                true
            }
            Err(error) => {
                tracing::error!("内嵌终端启动失败: {error}");
                self.terminal_panel.tabs.is_empty()
            }
        }
    }

    /// 关闭一个终端标签并终止其 shell;关掉激活标签后激活相邻标签,
    /// 关掉最后一个标签则收起面板(等同 shell 自己 exit 的语义)。
    pub(crate) fn close_terminal_tab(&mut self, session_id: SessionId) -> Task<Message> {
        let Some(index) = self
            .terminal_panel
            .tabs
            .iter()
            .position(|tab| tab.session.id == session_id)
        else {
            return Task::none();
        };
        let ids: Vec<SessionId> = self
            .terminal_panel
            .tabs
            .iter()
            .map(|tab| tab.session.id)
            .collect();

        let mut tab = self.terminal_panel.tabs.remove(index);
        tab.session.terminate();
        self.terminal_panel.tab_shift_animations.remove(&session_id);
        if self
            .terminal_panel
            .tab_drag
            .as_ref()
            .is_some_and(|drag| drag.session_id == session_id)
        {
            self.terminal_panel.tab_drag = None;
        }

        if self.terminal_panel.active_session_id == Some(session_id) {
            self.terminal_panel.active_session_id = None;
            self.terminal_panel.focused = false;
            if let Some(adjacent) = adjacent_session_id(&ids, session_id) {
                self.select_terminal_tab(adjacent);
            } else if self.terminal_panel.expanded {
                self.terminal_panel.expanded = false;
                self.terminal_panel.follow_target_snapshot = None;
                self.terminal_panel
                    .start_height_animation(super::COLLAPSED_HEIGHT);
            }
        }
        // 关到 1 个标签时标签条隐藏、画布变高,存留标签随之重算。
        self.sync_terminal_panel_size();
        Task::none()
    }

    /// 快捷键关闭当前激活标签;没有标签时是空操作。
    pub(crate) fn close_active_terminal_tab_via_shortcut(&mut self) -> Task<Message> {
        match self.terminal_panel.active_session_id {
            Some(session_id) => self.close_terminal_tab(session_id),
            None => Task::none(),
        }
    }

    /// 激活标签:清未读圆点、同步当前面板尺寸、接管键盘焦点。
    pub(crate) fn select_terminal_tab(&mut self, session_id: SessionId) {
        let columns = self.terminal_panel_grid_columns();
        let rows = super::terminal_panel_canvas_rows(
            self.terminal_panel.height.max(MIN_PANEL_HEIGHT),
            self.terminal_panel.tab_strip_visible(),
        );
        let Some(tab) = self
            .terminal_panel
            .tabs
            .iter_mut()
            .find(|tab| tab.session.id == session_id)
        else {
            return;
        };
        tab.has_unread_output = false;
        tab.session
            .resize(super::emulator_dimensions(columns, rows));
        self.terminal_panel.active_session_id = Some(session_id);
        self.terminal_panel.focused = true;
    }

    /// 按下标签:先激活,再进入"等待移动"阶段;移动阈值达成前不算拖拽。
    pub(crate) fn start_terminal_tab_drag(&mut self, session_id: SessionId) {
        self.select_terminal_tab(session_id);
        self.terminal_panel.tab_drag = Some(TerminalTabDrag {
            session_id,
            phase: TerminalTabDragPhase::WaitingForMovement {
                origin: self.cursor_position,
            },
        });
    }

    /// 全局光标移动:推进拖拽相位(未过阈值前保持等待)。
    pub(crate) fn update_terminal_tab_drag(&mut self, position: Point) {
        let Some(drag) = &mut self.terminal_panel.tab_drag else {
            return;
        };
        if let TerminalTabDragPhase::WaitingForMovement { origin } = drag.phase {
            let delta_x = position.x - origin.x;
            let delta_y = position.y - origin.y;
            let distance_squared = delta_x * delta_x + delta_y * delta_y;
            if distance_squared
                >= TERMINAL_TAB_DRAG_ACTIVATION_DISTANCE * TERMINAL_TAB_DRAG_ACTIVATION_DISTANCE
            {
                drag.phase = TerminalTabDragPhase::Dragging;
            }
        }
    }

    /// 拖拽悬停到其他标签上:立即换位并给被挤开的标签启动位移动画。
    pub(crate) fn reorder_terminal_tab_dragged(&mut self, entered: SessionId) {
        let Some(drag) = self.terminal_panel.tab_drag.as_ref() else {
            return;
        };
        if !drag.is_dragging() {
            return;
        }
        let dragged = drag.session_id;
        if dragged == entered {
            return;
        }
        let ids: Vec<SessionId> = self
            .terminal_panel
            .tabs
            .iter()
            .map(|tab| tab.session.id)
            .collect();
        let Some(new_ids) = reordered_session_ids(&ids, dragged, entered) else {
            return;
        };
        let dragged_index = ids
            .iter()
            .position(|id| *id == dragged)
            .expect("checked above");
        let entered_index = ids
            .iter()
            .position(|id| *id == entered)
            .expect("entered id comes from the live tab list");
        let shifted = shifted_ids_for_reorder(&ids, dragged_index, entered_index);

        // new_ids 是现有 id 的一个排列,按它重建 tabs;swap_remove 配合
        // 每次重新 position,避免维护删除后的索引偏移。
        let mut remaining = std::mem::take(&mut self.terminal_panel.tabs);
        let mut reordered_tabs = Vec::with_capacity(remaining.len());
        for id in &new_ids {
            let position = remaining
                .iter()
                .position(|tab| tab.session.id == *id)
                .expect("reordered ids come from the live tab list");
            reordered_tabs.push(remaining.swap_remove(position));
        }
        self.terminal_panel.tabs = reordered_tabs;

        let shift_offset = if dragged_index < entered_index {
            self.terminal_tab_slot_width()
        } else {
            -self.terminal_tab_slot_width()
        };
        for session_id in shifted {
            self.start_terminal_tab_shift(session_id, shift_offset);
        }
    }

    pub(crate) fn finish_terminal_tab_drag(&mut self) {
        self.terminal_panel.tab_drag = None;
    }

    /// 拖拽中的标签目录;视图层用它渲染浮动预览。
    pub(crate) fn terminal_tab_drag_preview(&self) -> Option<&Path> {
        let drag = self.terminal_panel.tab_drag.as_ref()?;
        if !drag.is_dragging() {
            return None;
        }
        self.terminal_panel
            .tabs
            .iter()
            .find(|tab| tab.session.id == drag.session_id)
            .map(|tab| tab.directory.as_path())
    }

    fn start_terminal_tab_shift(&mut self, session_id: SessionId, offset: f32) {
        let current_offset = self
            .terminal_panel
            .tab_shift_animations
            .get(&session_id)
            .map(|animation| animation.shift_offset())
            .unwrap_or(0.0);
        self.terminal_panel
            .tab_shift_animations
            .insert(session_id, TabShiftAnimation::new(current_offset + offset));
    }

    /// 标签条内单个标签的槽宽;位移动画距离用,与视图等分布局保持一致:
    /// N 个标签和最右 1 个加号排成 N+1 项、N 个间距,标签均分剩余宽度。
    fn terminal_tab_slot_width(&self) -> f32 {
        let count = self.terminal_panel.tabs.len().max(1) as f32;
        let strip_width =
            (self.main_window_width - self.sidebar_width - PANEL_HORIZONTAL_PADDING).max(1.0);
        let shareable_width =
            strip_width - super::view::TAB_ADD_BUTTON_SIZE - super::view::TAB_STRIP_SPACING * count;
        (shareable_width / count).max(MIN_TAB_SLOT_WIDTH)
    }

    /// 每帧推进位移动画;由 [`FileBrowser::advance_window_animation_frame`] 驱动。
    pub(crate) fn advance_terminal_tab_shift_animations(&mut self) {
        self.terminal_panel
            .tab_shift_animations
            .retain(|session_id, animation| {
                !animation.is_finished()
                    && self
                        .terminal_panel
                        .tabs
                        .iter()
                        .any(|tab| tab.session.id == *session_id)
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: u64) -> Vec<SessionId> {
        (1..=count).collect()
    }

    #[test]
    fn reorder_moves_tab_in_both_directions() {
        let list = ids(4);
        // 向左拖(3 → 1 的位置):与主区标签同语义,先移除再按原 index 插入
        assert_eq!(reordered_session_ids(&list, 3, 1), Some(vec![3, 1, 2, 4]));
        // 向右拖(1 → 4 的位置):落在目标槽位,中间标签整体左移
        assert_eq!(reordered_session_ids(&list, 1, 4), Some(vec![2, 3, 4, 1]));
    }

    #[test]
    fn reorder_rejects_same_tab_and_unknown_ids() {
        let list = ids(3);
        assert_eq!(reordered_session_ids(&list, 2, 2), None);
        assert_eq!(reordered_session_ids(&list, 9, 1), None);
        assert_eq!(reordered_session_ids(&list, 1, 9), None);
    }

    #[test]
    fn shifted_ids_cover_crossed_range_only() {
        let list = ids(5);
        // 向右拖:跨过 (dragged, entered] 区间
        assert_eq!(shifted_ids_for_reorder(&list, 0, 3), vec![2, 3, 4]);
        // 向左拖:跨过 [entered, dragged) 区间
        assert_eq!(shifted_ids_for_reorder(&list, 3, 0), vec![1, 2, 3]);
        assert!(shifted_ids_for_reorder(&list, 2, 2).is_empty());
    }

    #[test]
    fn adjacent_prefers_left_neighbor_then_right() {
        let list = ids(4);
        assert_eq!(adjacent_session_id(&list, 2), Some(1));
        assert_eq!(adjacent_session_id(&list, 4), Some(3));
        assert_eq!(adjacent_session_id(&list, 1), Some(2));
        assert_eq!(adjacent_session_id(&[42], 42), None);
    }
}
