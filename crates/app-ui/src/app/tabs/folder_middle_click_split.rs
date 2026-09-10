use std::path::PathBuf;

use iced::Task;

use super::{apply_tab_to_pane, pane_from_tab};
use crate::app::FileBrowser;
use crate::model::{BrowserPaneId, BrowserPaneLayout, BrowserTab, Message, SplitAxis};

impl FileBrowser {
    pub(in crate::app) fn open_directory_from_middle_click(
        &mut self,
        directory: PathBuf,
    ) -> Task<Message> {
        let Some(axis) = self.folder_middle_click_split_axis() else {
            return self.open_directory_in_new_tab(directory);
        };

        self.open_directory_in_split(directory, axis)
    }

    fn folder_middle_click_split_axis(&self) -> Option<SplitAxis> {
        if self.keyboard_modifiers.control() {
            Some(SplitAxis::Vertical)
        } else if self.keyboard_modifiers.shift() {
            Some(SplitAxis::Horizontal)
        } else {
            None
        }
    }

    fn open_directory_in_split(&mut self, directory: PathBuf, axis: SplitAxis) -> Task<Message> {
        self.sync_active_tab_state();
        self.context_menu = None;
        self.clear_preview();

        let source_id = self.active_pane_id();
        let destination_id = self.middle_click_split_destination_pane_id(source_id);
        let tab = self.next_directory_tab(directory);
        self.put_directory_tab_in_pane(destination_id, tab);

        let first_portion = match self.pane_layout {
            BrowserPaneLayout::Split {
                axis: current_axis,
                first_portion,
                ..
            } if current_axis == axis => first_portion,
            _ => 500,
        };
        self.pane_layout = BrowserPaneLayout::Split {
            axis,
            first: source_id,
            second: destination_id,
            active: destination_id,
            first_portion,
        };

        if let Some(destination) = self.pane_by_id(destination_id).cloned() {
            self.restore_pane_snapshot(destination);
            self.sync_tab_bar_visibility();
        }
        Task::batch([
            self.reload_current(),
            self.request_breadcrumb_drop_target_bounds_measurement(),
            // 中键分屏同样改变 panes 与 pane_layout，归属会话保存不变量。
            self.request_browser_session_save(),
        ])
    }

    fn middle_click_split_destination_pane_id(
        &mut self,
        source_id: BrowserPaneId,
    ) -> BrowserPaneId {
        match self.pane_layout {
            BrowserPaneLayout::Single { .. } => self.new_pane_id(),
            BrowserPaneLayout::Split { first, second, .. } => {
                if source_id == first {
                    second
                } else {
                    first
                }
            }
        }
    }

    fn next_directory_tab(&mut self, directory: PathBuf) -> BrowserTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        BrowserTab::directory(tab_id, directory)
    }

    fn put_directory_tab_in_pane(&mut self, pane_id: BrowserPaneId, tab: BrowserTab) {
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.tabs.push(tab.clone());
            pane.active_tab_id = tab.id;
            apply_tab_to_pane(pane, &tab);
        } else {
            self.panes.push(pane_from_tab(pane_id, tab));
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::keyboard;

    use super::*;
    use crate::config;

    fn browser() -> FileBrowser {
        FileBrowser::new(config::default_user_config()).0
    }

    #[test]
    fn plain_middle_click_keeps_opening_directory_in_new_tab() {
        let mut browser = browser();
        let directory = PathBuf::from("/workspace/project");

        drop(browser.open_directory_from_middle_click(directory.clone()));

        assert!(matches!(
            browser.pane_layout,
            BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY
            }
        ));
        assert_eq!(browser.tabs.len(), 2);
        assert_eq!(browser.current_dir, directory);
    }

    #[test]
    fn shift_middle_click_opens_directory_in_horizontal_split() {
        let mut browser = browser();
        let directory = PathBuf::from("/workspace/project");
        browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;

        drop(browser.open_directory_from_middle_click(directory.clone()));

        assert_split_directory(&browser, directory, SplitAxis::Horizontal);
    }

    #[test]
    fn control_middle_click_opens_directory_in_vertical_split() {
        let mut browser = browser();
        let directory = PathBuf::from("/workspace/project");
        browser.keyboard_modifiers = keyboard::Modifiers::CTRL;

        drop(browser.open_directory_from_middle_click(directory.clone()));

        assert_split_directory(&browser, directory, SplitAxis::Vertical);
    }

    #[test]
    fn middle_click_split_requests_session_save() {
        let mut config = config::ui_thread_startup_config();
        config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
        config.save_view_state = config.startup_location_policy.saves_view_state();
        let mut browser = FileBrowser::new(config).0;
        browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;

        drop(browser.open_directory_from_middle_click(PathBuf::from("/workspace/project")));

        assert!(matches!(browser.pane_layout, BrowserPaneLayout::Split { .. }));
        assert!(browser.pending_browser_session_save);
    }

    fn assert_split_directory(browser: &FileBrowser, directory: PathBuf, expected_axis: SplitAxis) {
        let BrowserPaneLayout::Split {
            axis,
            first,
            second,
            active,
            ..
        } = browser.pane_layout
        else {
            panic!("expected split pane layout");
        };

        assert_eq!(axis, expected_axis);
        assert_eq!(first, BrowserPaneId::PRIMARY);
        assert_eq!(active, second);
        assert_eq!(browser.current_dir, directory);
        assert_eq!(
            browser
                .pane_by_id(second)
                .expect("split destination pane")
                .current_dir,
            directory
        );
    }
}
