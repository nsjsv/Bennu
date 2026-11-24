//! 主窗口底部内嵌终端面板:底部窄条 + 可展开的真终端(PTY + 终端模拟)。
//!
//! 会话生命周期与面板可见性解耦:收起不杀 shell,`exit` 后自动收起。
//! 面板内是多个终端标签,同一时刻只渲染激活标签的网格。
//!
//! 输出在各会话自己的读线程里解析（`session.rs`），所有会话的事件经一条通道
//! （[`SessionEvents`]）、一个常驻订阅交给界面。

pub(crate) mod emulator;
mod events;
pub(crate) mod follow;
mod grid;
mod hidden;
pub(crate) mod input;
pub(crate) mod links;
mod palette;
pub(crate) mod search;
pub(crate) mod session;
pub(crate) mod tabs;
pub(crate) mod view;

use std::collections::HashMap;
use std::path::PathBuf;

use iced::{Point, Task};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{KeyArea, KeyPress, ShortcutTarget};

pub(crate) use events::SessionEvents;
use session::{SessionEvent, SessionId};
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
    /// 会话们攒下的一批事（同一会话的屏幕变化已合并）。
    SessionEvents(Vec<(SessionId, SessionEvent)>),
    /// 点了终端网格：键盘归终端（开着的搜索条随之关掉）。选区、鼠标上报、滚动由网格控件自己做。
    GridPressed,
    CopySelectionRequested,
    PasteRequested,
    PasteReceived(Option<String>),
    /// 程序没要鼠标时右键网格：复制、粘贴、全选、清屏、新标签。
    MenuRequested,
    SelectAllRequested,
    ClearRequested,
    /// Ctrl+点击了链接。
    LinkOpened(links::TerminalLink),
    /// Ctrl+点击的路径在后台看过了：在不在、是不是文件夹。
    LinkPathChecked {
        path: PathBuf,
        is_directory: Option<bool>,
    },
    /// 交给系统打开网址的结果；打不开时提示原因。
    LinkOpenFinished(Result<(), String>),
    SearchChanged(String),
    SearchStepped(search::SearchStep),
    /// 搜索框里按了回车：往上找，按着 Shift 往下找。
    SearchSubmitted,
    SearchClosed,
}

/// 高度动画:from → to 的缓动插值。
#[derive(Debug, Clone, Copy)]
struct HeightAnimation {
    from: f32,
    to: f32,
    /// 已经走了几帧（不是进度比例：存成比例时动画永远走不完）。
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
    /// 超级输入框执行的命令：有终端、还没显示出来的标签（见 `hidden.rs`）。
    pub(crate) hidden_tabs: Vec<TerminalTab>,
    pub(crate) active_session_id: Option<SessionId>,
    next_session_id: SessionId,
    /// 终端网格持有键盘焦点(点击面板获得;点击地址栏等输入会话自动让路)。
    pub(crate) focused: bool,
    /// 文件区跟随目标的上次快照;None 表示下次检查必须重新同步(刚展开/收起过)。
    follow_target_snapshot: Option<PathBuf>,
    pub(crate) tab_drag: Option<tabs::TerminalTabDrag>,
    pub(crate) tab_shift_animations: HashMap<SessionId, tabs::TabShiftAnimation>,
    pub(crate) events: SessionEvents,
    /// 开着的回看搜索属于哪个标签；切标签、点网格时关掉。
    pub(crate) search: Option<(SessionId, search::TerminalSearch)>,
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
            hidden_tabs: Vec::new(),
            active_session_id: None,
            next_session_id: 1,
            focused: false,
            follow_target_snapshot: None,
            tab_drag: None,
            tab_shift_animations: HashMap::new(),
            events: SessionEvents::new(),
            search: None,
        }
    }

    pub(crate) fn height(&self) -> f32 {
        self.height
    }

    fn take_session_id(&mut self) -> SessionId {
        let session_id = self.next_session_id;
        self.next_session_id += 1;
        session_id
    }

    pub(crate) fn is_resizing(&self) -> bool {
        self.resize_drag.is_some()
    }

    /// 不启动 shell 就展开面板，用来测文件区转换是否更新跟随目标。
    #[cfg(test)]
    pub(crate) fn expand_without_shell(&mut self) {
        self.expanded = true;
    }

    #[cfg(test)]
    pub(crate) fn followed_directory(&self) -> Option<&std::path::Path> {
        self.follow_target_snapshot.as_deref()
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.height_animation.is_some()
            || self.resize_drag.is_some()
            || !self.tab_shift_animations.is_empty()
    }

    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded
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
        // 丢掉会话就是挂断它（`TerminalSession` 的 Drop）。
        self.terminal_panel.tabs.clear();
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
            self.follow_terminal_panel_directory();
        }
        Task::none()
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
        if self.cursor_or_origin().y < drawer_top {
            self.terminal_panel.focused = false;
            if let Some(active_tab) = self.terminal_panel.active_tab_mut() {
                active_tab.session.emulator.clear_selection();
            }
        }
    }

    /// 用户在设置里选的 shell;未选(空)时跟随系统登录 shell。
    pub(crate) fn terminal_panel_shell(&self) -> String {
        if self.user_config().terminal_shell.is_empty() {
            session::shell_for_user()
        } else {
            self.user_config().terminal_shell.clone()
        }
    }

    /// 新终端从当前目录起步；回收站不是目录，从主目录起步。
    fn terminal_panel_spawn_directory(&self) -> PathBuf {
        self.tab()
            .current_directory()
            .unwrap_or(&self.home_dir)
            .to_path_buf()
    }

    /// 抽屉在内容栏里、左右各留 `PANEL_HORIZONTAL_PADDING`：标签条和终端画布都排在这个宽度里。
    pub(crate) fn terminal_panel_inner_width(&self) -> f32 {
        (self.main_content_width() - PANEL_HORIZONTAL_PADDING * 2.0).max(1.0)
    }

    pub(crate) fn terminal_panel_grid_columns(&self) -> usize {
        ((self.terminal_panel_inner_width() / view::cell_width()).floor() as usize).max(2)
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
        let step = animation.step + 1.0;
        let progress = (step / HEIGHT_ANIMATION_STEPS).min(1.0);
        self.terminal_panel.height =
            animation.from + (animation.to - animation.from) * ease_out_cubic(progress);
        if progress >= 1.0 {
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
            cursor_start_y: self.cursor_or_origin().y,
            height_start: self.terminal_panel.height,
        });
    }

    /// 全局左键释放:结束拖拽调高、选区拖动与标签拖拽。
    pub(crate) fn finish_terminal_panel_pointer_interaction(&mut self) {
        self.terminal_panel.resize_drag = None;
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
            TerminalPanelMessage::SessionEvents(events) => {
                return Task::batch(
                    events
                        .into_iter()
                        .map(|(session, event)| self.accept_session_event(session, event)),
                );
            }
            TerminalPanelMessage::GridPressed => {
                self.terminal_panel.focused = true;
                self.terminal_panel.search = None;
            }
            TerminalPanelMessage::PasteRequested => {
                self.context_menu = None;
                return iced::clipboard::read()
                    .map(|text| Message::TerminalPanel(TerminalPanelMessage::PasteReceived(text)));
            }
            TerminalPanelMessage::MenuRequested => {
                let has_selection = self
                    .terminal_panel
                    .active_tab()
                    .is_some_and(|tab| tab.session.emulator.selection_string().is_some());
                self.context_menu = Some(crate::model::ContextMenuState::Terminal(
                    crate::model::TerminalMenuState {
                        position: self.cursor_or_origin(),
                        has_selection,
                    },
                ));
            }
            TerminalPanelMessage::SelectAllRequested => {
                self.context_menu = None;
                if let Some(text) = self
                    .terminal_panel
                    .active_tab()
                    .and_then(|tab| tab.session.emulator.select_all())
                {
                    return iced::clipboard::write_primary(text);
                }
            }
            TerminalPanelMessage::ClearRequested => {
                self.context_menu = None;
                if let Some(tab) = self.terminal_panel.active_tab() {
                    tab.session.emulator.clear_history();
                    // shell 清屏重画提示符，全屏程序重画自己；不打断正在跑的程序。
                    tab.session.write_input(b"\x0c");
                }
            }
            TerminalPanelMessage::LinkOpened(link) => return self.open_terminal_link(link),
            TerminalPanelMessage::LinkPathChecked { path, is_directory } => {
                if let Some(is_directory) = is_directory {
                    return self.show_path_in_file_area(path, is_directory);
                }
            }
            TerminalPanelMessage::LinkOpenFinished(outcome) => {
                if let Err(error) = outcome {
                    self.show_global_error(error);
                }
            }
            TerminalPanelMessage::SearchChanged(query) => self.change_terminal_search(query),
            TerminalPanelMessage::SearchStepped(step) => self.step_terminal_search(step),
            TerminalPanelMessage::SearchSubmitted => {
                let step = if self.keyboard_modifiers.shift() {
                    search::SearchStep::Newer
                } else {
                    search::SearchStep::Older
                };
                self.step_terminal_search(step);
            }
            TerminalPanelMessage::SearchClosed => {
                self.terminal_panel.search = None;
            }
            TerminalPanelMessage::CopySelectionRequested => {
                self.context_menu = None;
                if let Some(active_tab) = self.terminal_panel.active_tab() {
                    if let Some(text) = active_tab.session.emulator.selection_string() {
                        return iced::clipboard::write::<Message>(text);
                    }
                }
            }
            TerminalPanelMessage::PasteReceived(text) => {
                if let (Some(text), Some(active_tab)) = (text, self.terminal_panel.active_tab()) {
                    active_tab.session.paste(&text);
                }
            }
        }
        Task::none()
    }

    /// 终端聚焦时的键盘独占路由;返回 `None` 表示不归终端管,继续原有快捷键路由。
    /// 宿主保留键按 `key` 认,打进终端的是 `modified_key`(按出来的字:Shift+; 是 `:`)。
    pub(crate) fn terminal_panel_keyboard_input(
        &mut self,
        key: &iced::keyboard::Key,
        modified_key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        if !self.terminal_has_keyboard() {
            return None;
        }

        // 宿主保留键:终端作用域的键(开关面板、新建/关闭标签),以及下面的复制、粘贴。
        // 动作只在文件区触发，终端里查到的只会是内置的。
        if let Some(ShortcutTarget::Builtin(action)) = KeyPress::from_iced(key, modifiers)
            .and_then(|press| self.keymap().target_for(KeyArea::Terminal, &press))
        {
            return Some(self.invoke_shortcut(action));
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

        let active_tab = self.terminal_panel.active_tab()?;
        // Shift+翻页、Shift+Home/End 滚回看（终端通行键位）；不带 Shift 的翻页给程序。
        if modifiers == iced::keyboard::Modifiers::SHIFT {
            if let Some(scroll) = input::scrollback_for_key(key) {
                active_tab.session.emulator.scroll(scroll);
                return Some(Task::none());
            }
        }
        let mode = active_tab.session.emulator.mode();
        let bytes = input::input_for_key(modified_key, modifiers, mode).into_bytes();
        if !bytes.is_empty() {
            active_tab.session.type_input(&bytes);
        }
        Some(Task::none())
    }

    /// 键盘（连同输入法）此刻归终端：终端聚焦，而且没有地址栏、改名这类输入会话占着。
    /// 按键路由、网格要不要输入法、终端里 cd 要不要带着文件区走，都按它。
    pub(crate) fn terminal_has_keyboard(&self) -> bool {
        self.terminal_panel.is_focused()
            && self.terminal_panel.search.is_none()
            && !self.keyboard_input_session_is_active()
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
    fn terminal_grid_fits_the_content_column_inside_its_padding() {
        // 抽屉在侧栏右边的内容栏里：只减左右留白，不再另减一次侧栏（2026-11-10 多减过）。
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.main_window_width = 1_000.0;
        browser.sidebar_width_when_shown = 200.0;
        let inner_width = 800.0 - PANEL_HORIZONTAL_PADDING * 2.0;
        assert_eq!(browser.terminal_panel_inner_width(), inner_width);
        assert_eq!(
            browser.terminal_panel_grid_columns(),
            (inner_width / view::cell_width()).floor() as usize
        );
    }

    #[test]
    fn opening_the_panel_reaches_its_height_and_stops_animating() {
        // 2026-11-21 虚拟机里：终端开着空闲时一直每秒 60 次重画，面板停在目标高度的三分之一多。
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.terminal_panel.expanded = true;
        browser
            .terminal_panel
            .start_height_animation(DEFAULT_PANEL_HEIGHT);

        for _ in 0..HEIGHT_ANIMATION_STEPS as usize {
            browser.advance_terminal_panel_height_animation();
        }

        assert_eq!(browser.terminal_panel.height(), DEFAULT_PANEL_HEIGHT);
        assert!(!browser.terminal_panel.is_animating());
    }

    #[test]
    fn canvas_height_reclaims_tab_strip_when_hidden() {
        let panel_height = 320.0;
        // 320 - 6(拖拽手柄) - 26(标签条) 与 320 - 6(标签条隐藏,归画布)。
        assert_eq!(view::canvas_height_for_panel(panel_height, true), 288.0);
        assert_eq!(view::canvas_height_for_panel(panel_height, false), 314.0);
    }
}
