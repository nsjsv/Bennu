//! 主窗口底部内嵌终端面板:底部窄条 + 可展开的真终端(PTY + 终端模拟)。
//!
//! 会话生命周期与面板可见性解耦:收起不杀 shell,`exit` 后自动收起。
//! 面板内是多个终端标签,同一时刻只渲染激活标签的网格。

pub(crate) mod emulator;
pub(crate) mod input;
pub(crate) mod session;
pub(crate) mod tabs;
pub(crate) mod view;

use std::collections::HashMap;
use std::path::PathBuf;

use iced::advanced::subscription::{self, EventStream, Hasher, Recipe};
use iced::futures::stream::BoxStream;
use iced::{Point, Task};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::ShortcutAction;

use session::SessionId;
use tabs::TerminalTab;

/// 展开时的默认面板高度(px)。
pub(crate) const DEFAULT_PANEL_HEIGHT: f32 = 320.0;
/// 拖拽调高的下限。
pub(crate) const MIN_PANEL_HEIGHT: f32 = 120.0;
/// 拖拽调高的上限(窗口高度比例)。
pub(crate) const MAX_PANEL_HEIGHT_RATIO: f32 = 0.8;
/// 面板内容左右留白;列数计算用。
pub(crate) const PANEL_HORIZONTAL_PADDING: f32 = 12.0;
/// 面板高度动画的 tick 步数(60Hz 下约 8 帧)。
const HEIGHT_ANIMATION_STEPS: f32 = 8.0;
/// 面板未展开时高度。
pub(crate) const COLLAPSED_HEIGHT: f32 = 0.0;
/// 终端标签拖拽的移动激活距离(与主区标签一致)。
pub(crate) const TERMINAL_TAB_DRAG_ACTIVATION_DISTANCE: f32 = 3.0;

#[derive(Debug, Clone)]
pub(crate) enum TerminalPanelMessage {
    ToggleRequested,
    PanelResizeDragStarted,
    TabAddRequested,
    TabPressed(SessionId),
    TabCloseRequested(SessionId),
    TabDragEntered(SessionId),
    TabDragFinished,
    OutputReceived { session: SessionId, bytes: Vec<u8> },
    ProcessExited { session: SessionId },
    GridPressed { position: Point },
    GridDragged { position: Point },
    GridWheelScrolled { lines: i32 },
    CopySelectionRequested,
    PasteReceived(Option<String>),
}

/// 高度动画:from → to 的缓动插值。
#[derive(Debug, Clone, Copy)]
struct HeightAnimation {
    from: f32,
    to: f32,
    step: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PanelResizeDrag {
    cursor_start_y: f32,
    height_start: f32,
}

pub(crate) struct TerminalPanelState {
    /// 用户意图:展开(true)/收起(false);height 动画跟随此值。
    expanded: bool,
    /// 当前渲染高度;0 = 完全收起。
    height: f32,
    /// 展开时的目标高度(拖拽记忆值)。
    expanded_height: f32,
    height_animation: Option<HeightAnimation>,
    resize_drag: Option<PanelResizeDrag>,
    pub(crate) tabs: Vec<TerminalTab>,
    pub(crate) active_session_id: Option<SessionId>,
    next_session_id: SessionId,
    /// 终端网格持有键盘焦点(点击面板获得;点击地址栏等输入会话自动让路)。
    pub(crate) focused: bool,
    /// 鼠标左键拖动选区进行中。
    selection_drag_active: bool,
    /// 文件区跟随目标的上次快照;None 表示下次检查必须重新同步(刚展开/收起过)。
    follow_target_snapshot: Option<PathBuf>,
    pub(crate) tab_drag: Option<tabs::TerminalTabDrag>,
    pub(crate) tab_shift_animations: HashMap<SessionId, tabs::TabShiftAnimation>,
}

impl TerminalPanelState {
    pub(crate) fn new() -> Self {
        Self {
            expanded: false,
            height: COLLAPSED_HEIGHT,
            expanded_height: DEFAULT_PANEL_HEIGHT,
            height_animation: None,
            resize_drag: None,
            tabs: Vec::new(),
            active_session_id: None,
            next_session_id: 1,
            focused: false,
            selection_drag_active: false,
            follow_target_snapshot: None,
            tab_drag: None,
            tab_shift_animations: HashMap::new(),
        }
    }

    pub(crate) fn height(&self) -> f32 {
        self.height
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.height_animation.is_some()
            || self.resize_drag.is_some()
            || !self.tab_shift_animations.is_empty()
    }

    pub(crate) fn is_focused(&self) -> bool {
        self.expanded && self.focused && self.active_session_id.is_some()
    }

    pub(crate) fn active_tab(&self) -> Option<&TerminalTab> {
        self.tabs
            .iter()
            .find(|tab| Some(tab.session.id) == self.active_session_id)
    }

    pub(crate) fn active_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let active_session_id = self.active_session_id;
        self.tabs
            .iter_mut()
            .find(|tab| Some(tab.session.id) == active_session_id)
    }

    /// 标签条可见性:≥2 个标签才显示,单标签只留右上角加号。
    pub(crate) fn tab_strip_visible(&self) -> bool {
        view::tab_strip_visible_for_tab_count(self.tabs.len())
    }

    fn start_height_animation(&mut self, target: f32) {
        self.height_animation = Some(HeightAnimation {
            from: self.height,
            to: target,
            step: 0.0,
        });
    }

    fn clamp_height_for_window(&self, height: f32, window_height: f32) -> f32 {
        height
            .max(MIN_PANEL_HEIGHT)
            .min((window_height * MAX_PANEL_HEIGHT_RATIO).max(MIN_PANEL_HEIGHT))
    }
}

impl FileBrowser {
    /// 快捷键语义:开着就彻底关闭(杀掉全部标签的 shell),关着就新开。
    pub(crate) fn toggle_terminal_via_shortcut(&mut self) -> Task<Message> {
        if self.terminal_panel.expanded {
            self.close_terminal_panel()
        } else {
            self.toggle_terminal_panel()
        }
    }

    /// 彻底关闭终端:终止所有标签的 shell 并收起面板。
    pub(crate) fn close_terminal_panel(&mut self) -> Task<Message> {
        self.terminal_panel.expanded = false;
        self.terminal_panel.focused = false;
        self.terminal_panel.follow_target_snapshot = None;
        self.terminal_panel.tab_drag = None;
        self.terminal_panel.tab_shift_animations.clear();
        for mut tab in self.terminal_panel.tabs.drain(..) {
            tab.session.terminate();
        }
        self.terminal_panel.active_session_id = None;
        self.terminal_panel.start_height_animation(COLLAPSED_HEIGHT);
        Task::none()
    }

    pub(crate) fn toggle_terminal_panel(&mut self) -> Task<Message> {
        if self.terminal_panel.expanded {
            self.terminal_panel.expanded = false;
            self.terminal_panel.focused = false;
            self.terminal_panel.start_height_animation(COLLAPSED_HEIGHT);
        } else {
            let target = self.terminal_panel.clamp_height_for_window(
                self.terminal_panel.expanded_height,
                self.main_window_height,
            );
            self.terminal_panel.expanded_height = target;
            self.terminal_panel.focused = true;
            if self.terminal_panel.tabs.is_empty() {
                let directory = self.terminal_panel_spawn_directory();
                if !self.spawn_terminal_tab_in_directory(directory, target) {
                    self.terminal_panel.expanded = false;
                    return Task::none();
                }
            }
            self.terminal_panel.expanded = true;
            self.terminal_panel.start_height_animation(target);
        }
        Task::none()
    }

    /// 终端跟随的目标目录:多栏视图取鼠标悬停的栏,其余视图取活动窗格目录。
    /// 多栏视图鼠标不在任何栏上时返回 None(保持现状,避免来回跳)。
    fn terminal_panel_follow_target(&self) -> Option<PathBuf> {
        let active_pane_id = self.pane_layout.active();
        let pane = self.panes.iter().find(|pane| pane.id == active_pane_id)?;
        if pane.view_mode != crate::model::BrowserViewMode::Columns {
            return Some(pane.current_dir.clone());
        }
        self.pointer_hovered_column_directory()
    }

    /// 活动窗格目录变化时向**激活标签**的 shell 注入 cd;前进/后退/侧边栏跳转统一跟随。
    /// 切换标签不注入:各标签保住自己的目录,否则每个标签都会被拉回文件区目录。
    /// 快照在面板收起时清空,让下次展开立即同步一次。
    pub(crate) fn follow_terminal_panel_directory(&mut self) {
        let Some(directory) = self.terminal_panel_follow_target() else {
            return;
        };
        if !self.terminal_panel.expanded {
            self.terminal_panel.follow_target_snapshot = None;
            return;
        }
        if self.terminal_panel.follow_target_snapshot.as_ref() == Some(&directory) {
            return;
        }
        self.terminal_panel.follow_target_snapshot = Some(directory.clone());
        let Some(active_tab) = self.terminal_panel.active_tab_mut() else {
            return;
        };
        active_tab.directory = directory.clone();
        let quoted = directory.to_string_lossy().replace('\'', "'\\''");
        active_tab
            .session
            .write_input(format!("cd -- '{quoted}'\r").as_bytes());
    }

    /// 点击终端抽屉以外的区域时把键盘焦点还给文件区;
    /// 否则面板一旦聚焦就永久吞掉文件列表快捷键。
    pub(crate) fn release_terminal_panel_focus_if_outside(&mut self) {
        if !self.terminal_panel.focused {
            return;
        }
        let drawer_top = if self.terminal_panel.expanded {
            self.main_window_height - self.terminal_panel.height()
        } else {
            self.main_window_height - view::BOTTOM_BAR_HEIGHT
        };
        if self.cursor_position.y < drawer_top {
            self.terminal_panel.focused = false;
            if let Some(active_tab) = self.terminal_panel.active_tab_mut() {
                active_tab.session.emulator.clear_selection();
            }
        }
    }

    /// 用户在设置里选的 shell;未选(空)时跟随系统登录 shell。
    fn terminal_panel_shell(&self) -> String {
        if self.user_config().terminal_shell.is_empty() {
            session::shell_for_user()
        } else {
            self.user_config().terminal_shell.clone()
        }
    }

    fn terminal_panel_spawn_directory(&self) -> PathBuf {
        let active_pane_id = self.pane_layout.active();
        self.panes
            .iter()
            .find(|pane| pane.id == active_pane_id)
            .map(|pane| pane.current_dir.clone())
            .unwrap_or_else(|| self.current_dir.clone())
    }

    /// 内容区宽度能容纳的终端列数(抽屉在侧边栏占位右侧)。
    pub(crate) fn terminal_panel_grid_columns(&self) -> usize {
        let width =
            (self.main_window_width - self.sidebar_width - PANEL_HORIZONTAL_PADDING * 2.0).max(1.0);
        ((width / view::CELL_WIDTH).floor() as usize).max(2)
    }

    /// 面板高度稳定后把新尺寸同步给所有 PTY;动画期间跳过,结束时再收敛。
    /// 后台标签保持同尺寸,避免切回时内容按旧宽度折行。
    pub(crate) fn sync_terminal_panel_size(&mut self) {
        if !self.terminal_panel.expanded {
            return;
        }
        let rows = terminal_panel_canvas_rows(
            self.terminal_panel.height.max(MIN_PANEL_HEIGHT),
            self.terminal_panel.tab_strip_visible(),
        );
        let columns = self.terminal_panel_grid_columns();
        for tab in &mut self.terminal_panel.tabs {
            tab.session.resize(emulator_dimensions(columns, rows));
        }
    }

    /// 高度动画推进;由 [`FileBrowser::advance_window_animation_frame`] 驱动。
    pub(crate) fn advance_terminal_panel_height_animation(&mut self) {
        let Some(animation) = self.terminal_panel.height_animation else {
            return;
        };
        let step = (animation.step + 1.0) / HEIGHT_ANIMATION_STEPS;
        let eased = ease_out_cubic(step.min(1.0));
        self.terminal_panel.height = animation.from + (animation.to - animation.from) * eased;
        if step >= 1.0 {
            self.terminal_panel.height = animation.to;
            self.terminal_panel.height_animation = None;
        } else {
            self.terminal_panel.height_animation = Some(HeightAnimation {
                from: animation.from,
                to: animation.to,
                step,
            });
        }
        self.sync_terminal_panel_size();
    }

    /// 拖拽面板上边缘调高;鼠标位置由全局 CursorMoved 提供。
    pub(crate) fn update_terminal_panel_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.terminal_panel.resize_drag else {
            return;
        };
        let dragged = drag.height_start + (drag.cursor_start_y - position.y);
        let clamped = self
            .terminal_panel
            .clamp_height_for_window(dragged, self.main_window_height);
        self.terminal_panel.expanded_height = clamped;
        self.terminal_panel.height = clamped;
        self.terminal_panel.height_animation = None;
        self.sync_terminal_panel_size();
    }

    pub(crate) fn start_terminal_panel_resize_drag(&mut self) {
        if !self.terminal_panel.expanded {
            return;
        }
        self.terminal_panel.resize_drag = Some(PanelResizeDrag {
            cursor_start_y: self.cursor_position.y,
            height_start: self.terminal_panel.height,
        });
    }

    /// 全局左键释放:结束拖拽调高、选区拖动与标签拖拽。
    pub(crate) fn finish_terminal_panel_pointer_interaction(&mut self) {
        self.terminal_panel.resize_drag = None;
        self.terminal_panel.selection_drag_active = false;
        self.finish_terminal_tab_drag();
    }

    pub(crate) fn handle_terminal_panel_message(
        &mut self,
        message: TerminalPanelMessage,
    ) -> Task<Message> {
        match message {
            TerminalPanelMessage::ToggleRequested => return self.toggle_terminal_panel(),
            TerminalPanelMessage::PanelResizeDragStarted => {
                self.start_terminal_panel_resize_drag();
            }
            TerminalPanelMessage::TabAddRequested => return self.add_terminal_tab(),
            TerminalPanelMessage::TabPressed(session_id) => {
                self.start_terminal_tab_drag(session_id);
            }
            TerminalPanelMessage::TabCloseRequested(session_id) => {
                return self.close_terminal_tab(session_id);
            }
            TerminalPanelMessage::TabDragEntered(session_id) => {
                self.reorder_terminal_tab_dragged(session_id);
            }
            TerminalPanelMessage::TabDragFinished => {
                self.finish_terminal_tab_drag();
            }
            TerminalPanelMessage::OutputReceived { session, bytes } => {
                let active_session_id = self.terminal_panel.active_session_id;
                if let Some(tab) = self
                    .terminal_panel
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.session.id == session)
                {
                    tab.session.emulator.feed(&bytes);
                    if active_session_id != Some(session) {
                        tab.has_unread_output = true;
                    }
                }
            }
            TerminalPanelMessage::ProcessExited { session } => {
                return self.close_terminal_tab(session);
            }
            TerminalPanelMessage::GridPressed { position } => {
                self.terminal_panel.focused = true;
                self.terminal_panel.selection_drag_active = true;
                if let Some(active_tab) = self.terminal_panel.active_tab_mut() {
                    let (row, column) = view::grid_cell_for_position(position);
                    active_tab.session.emulator.clear_selection();
                    active_tab.session.emulator.begin_selection(row, column);
                }
            }
            TerminalPanelMessage::GridDragged { position } => {
                if self.terminal_panel.selection_drag_active {
                    if let Some(active_tab) = self.terminal_panel.active_tab_mut() {
                        let (row, column) = view::grid_cell_for_position(position);
                        active_tab.session.emulator.extend_selection(row, column);
                    }
                }
            }
            TerminalPanelMessage::GridWheelScrolled { lines } => {
                if let Some(active_tab) = self.terminal_panel.active_tab_mut() {
                    active_tab.session.emulator.scroll_display(lines);
                }
            }
            TerminalPanelMessage::CopySelectionRequested => {
                if let Some(active_tab) = self.terminal_panel.active_tab() {
                    if let Some(text) = active_tab.session.emulator.selection_string() {
                        return iced::clipboard::write::<Message>(text);
                    }
                }
            }
            TerminalPanelMessage::PasteReceived(text) => {
                if let Some(text) = text {
                    if let Some(active_tab) = self.terminal_panel.active_tab() {
                        active_tab
                            .session
                            .write_input(text.replace("\r\n", "\r").as_bytes());
                    }
                }
            }
        }
        Task::none()
    }

    /// 终端聚焦时的键盘独占路由;返回 `None` 表示不归终端管,继续原有快捷键路由。
    pub(crate) fn terminal_panel_keyboard_input(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        if !self.terminal_panel.is_focused() {
            return None;
        }
        if self.keyboard_input_session_is_active() {
            return None;
        }

        // 宿主保留键:开关面板、新建/关闭标签、复制、粘贴。
        match self.shortcut_config().matching_action(key, modifiers) {
            Some(ShortcutAction::ToggleTerminal) => {
                return Some(self.toggle_terminal_via_shortcut());
            }
            Some(ShortcutAction::TerminalNewTab) => return Some(self.add_terminal_tab()),
            Some(ShortcutAction::TerminalCloseTab) => {
                return Some(self.close_active_terminal_tab_via_shortcut());
            }
            _ => {}
        }
        if modifiers.control() && modifiers.shift() {
            match key.as_ref() {
                iced::keyboard::Key::Character("c") => {
                    return Some(self.handle_terminal_panel_message(
                        TerminalPanelMessage::CopySelectionRequested,
                    ));
                }
                iced::keyboard::Key::Character("v") => {
                    return Some(iced::clipboard::read().map(|text| {
                        Message::TerminalPanel(TerminalPanelMessage::PasteReceived(text))
                    }));
                }
                _ => {}
            }
        }

        let Some(active_tab) = self.terminal_panel.active_tab_mut() else {
            return None;
        };
        let input = input::input_for_key(key, modifiers);
        match input {
            input::TerminalInput::None => {
                // 无修饰翻页键属于本地回看。
                let rows = active_tab.session.emulator.dimensions().screen_lines as i32;
                match key.as_ref() {
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::PageUp) => {
                        active_tab.session.emulator.scroll_display(rows / 2);
                    }
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::PageDown) => {
                        active_tab.session.emulator.scroll_display(-(rows / 2));
                    }
                    _ => {}
                }
            }
            other => {
                let bytes = other.into_bytes();
                if !bytes.is_empty() {
                    active_tab.session.write_input(&bytes);
                }
            }
        }
        Some(Task::none())
    }

    /// 终端面板的输出订阅;每个标签一份,后台标签持续积累输出。
    /// 同一会话重订阅时克隆读端继续读;运行时按 session_id 去重,
    /// 重复构造的 recipe(连同其读端)会被丢弃。
    pub(crate) fn terminal_output_subscription(&self) -> iced::Subscription<Message> {
        if self.terminal_panel.tabs.is_empty() {
            return iced::Subscription::none();
        }
        let mut recipes = Vec::with_capacity(self.terminal_panel.tabs.len());
        for tab in &self.terminal_panel.tabs {
            let Ok(reader) = tab.session.clone_reader() else {
                tracing::error!("内嵌终端输出订阅失败: 读端不可用");
                continue;
            };
            recipes.push(subscription::from_recipe(TerminalOutputRecipe {
                session_id: tab.session.id,
                reader,
            }));
        }
        iced::Subscription::batch(recipes)
    }
}

/// 输出订阅 recipe:稳定键为会话 id。
struct TerminalOutputRecipe {
    session_id: SessionId,
    reader: Box<dyn std::io::Read + Send>,
}

impl Recipe for TerminalOutputRecipe {
    type Output = Message;

    fn hash(&self, state: &mut Hasher) {
        use std::hash::Hasher as _;
        state.write_u64(self.session_id);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Message> {
        use iced::futures::StreamExt;
        Box::pin(
            session::terminal_output_stream(self.session_id, self.reader)
                .map(Message::TerminalPanel),
        )
    }
}

pub(crate) fn terminal_panel_grid_rows(height: f32) -> usize {
    ((height / view::CELL_HEIGHT).floor() as usize).max(2)
}

/// 面板高度 → PTY 行数:按去掉手柄与标签条后的画布高度换算。
pub(crate) fn terminal_panel_canvas_rows(height: f32, tab_strip_visible: bool) -> usize {
    terminal_panel_grid_rows(view::canvas_height_for_panel(height, tab_strip_visible))
}

pub(crate) fn emulator_dimensions(columns: usize, rows: usize) -> emulator::TerminalDimensions {
    emulator::TerminalDimensions {
        columns,
        screen_lines: rows,
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let clamped = t.clamp(0.0, 1.0);
    1.0 - (1.0 - clamped).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> TerminalPanelState {
        TerminalPanelState::new()
    }

    #[test]
    fn new_panel_starts_collapsed() {
        let panel = state();
        assert_eq!(panel.height(), 0.0);
        assert!(!panel.is_focused());
        assert!(panel.tabs.is_empty());
        assert!(panel.active_tab().is_none());
    }

    #[test]
    fn height_clamp_respects_window_ratio() {
        let panel = state();
        assert_eq!(panel.clamp_height_for_window(2000.0, 800.0), 640.0);
        assert_eq!(panel.clamp_height_for_window(10.0, 800.0), MIN_PANEL_HEIGHT);
        assert_eq!(panel.clamp_height_for_window(300.0, 800.0), 300.0);
    }

    #[test]
    fn rows_follow_cell_height() {
        assert_eq!(terminal_panel_grid_rows(view::CELL_HEIGHT * 5.0), 5);
    }

    #[test]
    fn tab_strip_shows_only_with_multiple_tabs() {
        assert!(!view::tab_strip_visible_for_tab_count(0));
        assert!(!view::tab_strip_visible_for_tab_count(1));
        assert!(view::tab_strip_visible_for_tab_count(2));
    }

    #[test]
    fn canvas_height_reclaims_tab_strip_when_hidden() {
        let panel_height = 320.0;
        // 320 - 6(拖拽手柄) - 26(标签条) 与 320 - 6(标签条隐藏,归画布)。
        assert_eq!(view::canvas_height_for_panel(panel_height, true), 288.0);
        assert_eq!(view::canvas_height_for_panel(panel_height, false), 314.0);
    }
}
