//! 文件区右键菜单的键盘操作：Shift+F10 / Menu 键对选中项打开（没有选中项时是空白处菜单），
//! 打开后 ↑↓ 走行，→ / Enter 进子菜单，← 退出子菜单，Enter 执行；vim 模式下 j k h l 同方向键。
//! 键盘执行的就是鼠标点那一行发的消息。

use iced::keyboard::{self, key};
use iced::{Point, Task};

use super::FileBrowser;
use crate::context_menu_rows::{file_context_menu_rows, MenuRow, MenuRowKind};
use crate::file_drag_hit_test_bounds::{
    file_drag_hit_test_bounds_command, FileDragHitTestBoundsRequest,
};
use crate::model::{
    BrowserLocation, ColumnEntryBounds, ContextMenuState, FileContextMenuExpansion, MenuHighlight,
    Message,
};
use crate::shortcuts::LetterMode;

/// 菜单左上角离选中项左边缘多远（贴着选中项的下沿出来）。
const MENU_ANCHOR_INSET_X: f32 = 24.0;
/// 选中项不在屏幕上时，菜单放在内容区左上角往里这么远。
const MENU_FALLBACK_INSET: f32 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKey {
    Next,
    Previous,
    First,
    Last,
    Enter,
    IntoSubmenu,
    OutOfSubmenu,
    Escape,
    Other,
}

fn menu_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    letter_mode: LetterMode,
) -> MenuKey {
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return MenuKey::Other;
    }
    match key.as_ref() {
        keyboard::Key::Named(key::Named::ArrowDown) => MenuKey::Next,
        keyboard::Key::Named(key::Named::ArrowUp) => MenuKey::Previous,
        keyboard::Key::Named(key::Named::Home) => MenuKey::First,
        keyboard::Key::Named(key::Named::End) => MenuKey::Last,
        keyboard::Key::Named(key::Named::Enter | key::Named::Space) => MenuKey::Enter,
        keyboard::Key::Named(key::Named::ArrowRight) => MenuKey::IntoSubmenu,
        keyboard::Key::Named(key::Named::ArrowLeft) => MenuKey::OutOfSubmenu,
        keyboard::Key::Named(key::Named::Escape) => MenuKey::Escape,
        keyboard::Key::Character(value) if letter_mode == LetterMode::Vim => match value {
            "j" => MenuKey::Next,
            "k" => MenuKey::Previous,
            "l" => MenuKey::IntoSubmenu,
            "h" => MenuKey::OutOfSubmenu,
            _ => MenuKey::Other,
        },
        _ => MenuKey::Other,
    }
}

impl FileBrowser {
    /// Shift+F10 / Menu 键：先量选中项在屏幕上的位置，量回来再打开菜单。
    pub(super) fn open_context_menu_from_keyboard(&mut self) -> Task<Message> {
        file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::KeyboardContextMenu)
    }

    pub(super) fn open_context_menu_at_measured_selection(
        &mut self,
        bounds: &[ColumnEntryBounds],
    ) -> Task<Message> {
        // 量的这段时间里可能已经开了别的东西。
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let pane_id = self.active_pane_id();
        let selected = self.tab().selected.clone();
        let position = selected
            .as_ref()
            .and_then(|path| {
                bounds
                    .iter()
                    .find(|entry| entry.pane_id == pane_id && &entry.path == path)
            })
            .map(|entry| {
                Point::new(
                    entry.bounds.x + MENU_ANCHOR_INSET_X,
                    entry.bounds.y + entry.bounds.height,
                )
            })
            .unwrap_or_else(|| {
                Point::new(
                    self.sidebar_layout_width() + MENU_FALLBACK_INSET,
                    self.main_panes_area_top() + MENU_FALLBACK_INSET,
                )
            });
        let task = match (selected, self.keyboard_target_directory()) {
            (Some(path), _) => self.open_entry_context_menu(path, position),
            (None, Some(directory)) => self.open_blank_context_menu(directory, position),
            (None, None) if self.tab().location == BrowserLocation::Trash => {
                self.open_trash_context_menu(None, position);
                Task::none()
            }
            (None, None) => Task::none(),
        };
        if let Some(ContextMenuState::FileArea(menu)) = &mut self.context_menu {
            menu.keyboard_highlight = Some(MenuHighlight {
                row: 0,
                member: None,
            });
        }
        task
    }

    /// 文件区右键菜单开着时的按键。返回 None 表示不归菜单管（菜单没开，或在一级菜单按了
    /// Esc——交给关闭当前层）；其余键都由菜单吃掉，不穿透到后面的文件区。
    pub(super) fn file_context_menu_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        let Some(ContextMenuState::FileArea(menu)) = &self.context_menu else {
            return None;
        };
        let rows = file_context_menu_rows(
            menu,
            &self.user_config.context_menus,
            self.keyboard_modifiers,
            self.user_config.file_grouping,
        );
        let menu_key = menu_key(key, modifiers, self.user_config.letter_mode);
        let highlight = menu.keyboard_highlight;
        let (next, message) = navigate(&rows, highlight, menu_key)?;
        if let Some(ContextMenuState::FileArea(menu)) = &mut self.context_menu {
            menu.keyboard_highlight = next;
            menu.expansion = next
                .filter(|highlight| highlight.member.is_some())
                .and_then(|highlight| rows.get(highlight.row))
                .and_then(MenuRow::expansion)
                .unwrap_or(FileContextMenuExpansion::None);
        }
        Some(message.map_or_else(Task::none, Task::done))
    }
}

/// 按一个键之后高亮停在哪、要不要执行某一行。外层 None = 这个键不归菜单（一级菜单的 Esc）。
fn navigate(
    rows: &[MenuRow],
    highlight: Option<MenuHighlight>,
    menu_key: MenuKey,
) -> Option<(Option<MenuHighlight>, Option<Message>)> {
    let count = rows.len();
    let row_index = highlight
        .map(|highlight| highlight.row)
        .filter(|row| *row < count);
    let member = highlight.and_then(|highlight| highlight.member);
    let at_row = |row: usize| Some(MenuHighlight { row, member: None });
    let wrap = |index: isize, len: usize| index.rem_euclid(len as isize) as usize;
    if count == 0 {
        return Some((None, None));
    }
    if let (Some(row), Some(member)) = (row_index, member) {
        let members = rows[row].members();
        let in_submenu = |member: usize| {
            Some(MenuHighlight {
                row,
                member: Some(member),
            })
        };
        return Some(match menu_key {
            MenuKey::Next => (in_submenu(wrap(member as isize + 1, members.len())), None),
            MenuKey::Previous => (in_submenu(wrap(member as isize - 1, members.len())), None),
            MenuKey::First => (in_submenu(0), None),
            MenuKey::Last => (in_submenu(members.len().saturating_sub(1)), None),
            MenuKey::Enter | MenuKey::IntoSubmenu => {
                (highlight, members.get(member).map(MenuRow::press_message))
            }
            MenuKey::OutOfSubmenu | MenuKey::Escape => (at_row(row), None),
            MenuKey::Other => (highlight, None),
        });
    }
    let current = row_index;
    Some(match menu_key {
        MenuKey::Next => (
            at_row(current.map_or(0, |row| wrap(row as isize + 1, count))),
            None,
        ),
        MenuKey::Previous => (
            at_row(current.map_or(count - 1, |row| wrap(row as isize - 1, count))),
            None,
        ),
        MenuKey::First => (at_row(0), None),
        MenuKey::Last => (at_row(count - 1), None),
        MenuKey::Escape => return None,
        MenuKey::Enter | MenuKey::IntoSubmenu => {
            let Some(row) = current else {
                return Some((highlight, None));
            };
            match (&rows[row].kind, menu_key) {
                (MenuRowKind::Submenu { members, .. }, _) if !members.is_empty() => (
                    Some(MenuHighlight {
                        row,
                        member: Some(0),
                    }),
                    None,
                ),
                (MenuRowKind::Action(message), MenuKey::Enter) => {
                    (highlight, Some(message.clone()))
                }
                _ => (highlight, None),
            }
        }
        MenuKey::OutOfSubmenu | MenuKey::Other => (highlight, None),
    })
}

#[cfg(test)]
#[path = "context_menu_keys_tests.rs"]
mod tests;
