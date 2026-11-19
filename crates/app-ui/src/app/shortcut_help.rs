//! F1 快捷键一览的开关。

use iced::keyboard;
use iced::Task;

use super::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{fixed_key, FixedKey, KeyArea, KeyPress, ShortcutAction};

impl FileBrowser {
    pub(super) fn toggle_shortcut_help(&mut self) -> Task<Message> {
        self.shortcut_help_open = !self.shortcut_help_open;
        self.context_menu = None;
        Task::none()
    }

    /// 一览开着时键盘只认关掉它的键（Esc、一览自己的键），别的键都不穿透到后面。
    pub(super) fn shortcut_help_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Task<Message> {
        let Some(press) = KeyPress::from_iced(key, modifiers) else {
            return Task::none();
        };
        let closes = fixed_key(&press) == Some(FixedKey::Escape)
            || self
                .shortcut_config()
                .action_for(KeyArea::FileBrowser, &press)
                == Some(ShortcutAction::ShowShortcuts);
        if closes {
            self.shortcut_help_open = false;
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use iced::event;
    use iced::keyboard::{key, Key, Modifiers};

    use super::*;
    use crate::config;

    fn press(browser: &mut FileBrowser, key: Key, modifiers: Modifiers) {
        drop(browser.handle_keyboard_key_pressed(key, modifiers, event::Status::Ignored));
    }

    #[test]
    fn f1_opens_the_sheet_which_swallows_other_keys_until_closed() {
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        press(
            &mut browser,
            Key::Named(key::Named::F1),
            Modifiers::default(),
        );
        assert!(browser.shortcut_help_open);

        press(&mut browser, Key::Character("t".into()), Modifiers::CTRL);
        assert_eq!(browser.pane().tabs.len(), 1, "Ctrl+T 不穿透");
        press(
            &mut browser,
            Key::Named(key::Named::Escape),
            Modifiers::default(),
        );
        assert!(!browser.shortcut_help_open);

        press(
            &mut browser,
            Key::Character("/".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        );
        assert!(browser.shortcut_help_open);
        press(
            &mut browser,
            Key::Named(key::Named::F1),
            Modifiers::default(),
        );
        assert!(!browser.shortcut_help_open);
    }
}
