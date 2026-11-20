//! 关闭标签页（键盘：关掉窗格或窗口的最后一个）与恢复刚关掉的标签页。

use iced::Task;

use super::super::FileBrowser;
use crate::model::{
    tab_session_from_live, BrowserLocation, BrowserPaneLayout, BrowserTabSession, Message,
    NavigationMode,
};

/// 记住多少个关掉的标签页；只是快照（位置、视图、选择、历史），不带目录内容。
const CLOSED_TAB_MEMORY: usize = 16;

impl FileBrowser {
    pub(super) fn remember_closed_tab(&mut self, snapshot: BrowserTabSession) {
        if self.closed_tabs.len() == CLOSED_TAB_MEMORY {
            self.closed_tabs.remove(0);
        }
        self.closed_tabs.push(snapshot);
    }

    /// Ctrl+W：窗格里还有别的标签页就只关这一个；窗格只剩它时收掉这个窗格；整个窗口只剩它时
    /// 关窗口（标签栏只有一个标签页时不显示，这两种情况只有键盘走得到）。
    pub(in crate::app) fn close_active_tab_from_keyboard(&mut self) -> Task<Message> {
        if self.open_tab_count() > 1 {
            return self.close_tab(self.pane().active_tab_id);
        }
        match self.pane_layout {
            BrowserPaneLayout::Split { .. } => self.close_active_pane(),
            BrowserPaneLayout::Single { .. } => self.close_auxiliary_window(self.main_window),
        }
    }

    fn close_active_pane(&mut self) -> Task<Message> {
        let BrowserPaneLayout::Split {
            first,
            second,
            active,
            ..
        } = self.pane_layout
        else {
            return Task::none();
        };
        let remaining = if active == first { second } else { first };
        let snapshot = tab_session_from_live(self.tab());
        self.remember_closed_tab(snapshot);
        self.invalidate_file_drop_for_tab_close();
        let cancel_address_editing = self.cancel_address_editing();
        self.clear_icon_grid_expansion_for_context_change();
        self.panes.retain(|pane| pane.id != active);
        self.icon_grid_viewports.remove(&active);
        self.pane_layout = BrowserPaneLayout::Single { active: remaining };
        self.reset_active_pane_context();
        self.follow_terminal_panel_directory();
        Task::batch([cancel_address_editing, self.request_browser_session_save()])
    }

    /// Ctrl+Shift+T：在当前窗格末尾重新打开最近关掉的标签页，先关的后开。
    pub(in crate::app) fn restore_closed_tab(&mut self) -> Task<Message> {
        let Some(mut snapshot) = self.closed_tabs.pop() else {
            return Task::none();
        };
        let location = snapshot.location.clone();
        let cancel_address_editing = self.add_tab_and_activate(|id, _| {
            snapshot.id = id;
            snapshot.to_browser_tab()
        });
        let load = match location {
            BrowserLocation::Directory(directory) => {
                self.navigate_to(directory, NavigationMode::KeepHistory)
            }
            BrowserLocation::Trash => self.open_trash_view(NavigationMode::KeepHistory),
            // 快照里的搜索标签已经换成了发起搜索的文件夹。
            BrowserLocation::Search(_) => Task::none(),
        };
        Task::batch([
            cancel_address_editing,
            load,
            self.request_browser_session_save(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};
    use iced::event;
    use iced::keyboard::{Key, Modifiers};

    use super::super::super::FileBrowser;
    use crate::config;
    use crate::model::BrowserLocation;
    use crate::shortcuts::{FileContentKey, ShortcutAction};

    fn browser_in_workspace() -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Ready;
        browser.tab_mut().entries = vec![DirectoryEntry::new(
            Path::new("/workspace/photos").to_path_buf(),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        )]
        .into();
        browser
    }

    fn press(browser: &mut FileBrowser, key: Key, modifiers: Modifiers) {
        drop(browser.handle_keyboard_key_pressed(
            key.clone(),
            key,
            modifiers,
            event::Status::Ignored,
        ));
    }

    fn character(value: &str) -> Key {
        Key::Character(value.into())
    }

    fn routed(browser: &mut FileBrowser, action: ShortcutAction) {
        drop(browser.invoke_routed_file_content_key(FileContentKey::Action(action)));
    }

    fn open_tab_count(browser: &FileBrowser) -> usize {
        browser.open_tab_count()
    }

    #[test]
    fn ctrl_w_closes_a_tab_and_ctrl_shift_t_brings_it_back() {
        let mut browser = browser_in_workspace();
        browser.select_path(PathBuf::from("/workspace/photos"));
        routed(&mut browser, ShortcutAction::OpenInNewTab);
        assert_eq!(open_tab_count(&browser), 2);

        press(&mut browser, character("w"), Modifiers::CTRL);
        assert_eq!(open_tab_count(&browser), 1);
        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(PathBuf::from("/workspace"))
        );

        press(
            &mut browser,
            character("t"),
            Modifiers::CTRL | Modifiers::SHIFT,
        );
        assert_eq!(open_tab_count(&browser), 2);
        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(PathBuf::from("/workspace/photos"))
        );
        // 没有可恢复的了。
        press(
            &mut browser,
            character("t"),
            Modifiers::CTRL | Modifiers::SHIFT,
        );
        assert_eq!(open_tab_count(&browser), 2);
    }

    #[test]
    fn ctrl_w_on_the_last_tab_of_a_split_side_closes_that_side() {
        let mut browser = browser_in_workspace();
        // Ctrl+中键点文件夹 = 在右边分屏打开。
        browser.keyboard_modifiers = Modifiers::CTRL;
        drop(browser.open_directory_from_middle_click(PathBuf::from("/workspace/photos")));
        browser.keyboard_modifiers = Modifiers::default();
        assert!(matches!(
            browser.pane_layout,
            crate::model::BrowserPaneLayout::Split { .. }
        ));
        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(PathBuf::from("/workspace/photos"))
        );

        press(&mut browser, character("w"), Modifiers::CTRL);

        assert!(matches!(
            browser.pane_layout,
            crate::model::BrowserPaneLayout::Single { .. }
        ));
        assert_eq!(browser.panes.len(), 1);
        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(PathBuf::from("/workspace"))
        );
        assert!(browser.application_shutdown_phase.is_running());

        press(
            &mut browser,
            character("t"),
            Modifiers::CTRL | Modifiers::SHIFT,
        );
        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(PathBuf::from("/workspace/photos"))
        );
    }

    #[test]
    fn ctrl_w_on_the_very_last_tab_closes_the_window() {
        let mut browser = browser_in_workspace();
        press(&mut browser, character("w"), Modifiers::CTRL);
        assert!(!browser.application_shutdown_phase.is_running());
    }

    #[test]
    fn only_the_most_recent_closed_tabs_are_kept() {
        let mut browser = browser_in_workspace();
        for index in 0..20 {
            browser.remember_closed_tab(crate::model::tab_session_from_live(&{
                let mut tab = browser.tab().clone();
                tab.location =
                    BrowserLocation::Directory(PathBuf::from(format!("/closed/{index}")));
                tab
            }));
        }
        assert_eq!(browser.closed_tabs.len(), 16);
        assert_eq!(
            browser.closed_tabs.last().map(|tab| tab.location.clone()),
            Some(BrowserLocation::Directory(PathBuf::from("/closed/19")))
        );
    }
}
