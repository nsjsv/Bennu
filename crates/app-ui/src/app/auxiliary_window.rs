//! 辅助窗口登记："这个窗口 id 是哪种窗口"只在这里判定。
//!
//! 窗口 id 散在各自的状态里（设置、属性、预览引擎、传输）。显示、标题、Esc、
//! 关闭请求、最大化记账、退出都按种类穷举：新增一种窗口时编译器逐处提醒，
//! 不会再出现"退出时漏关某个窗口"这种问题。

use iced::{window, Task};

use super::FileBrowser;
use crate::model::Message;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuxiliaryWindow {
    Settings,
    Properties,
    Preview,
    Transfer,
}

impl AuxiliaryWindow {
    /// 按 id 反查与退出时逐个关闭都靠它；新增种类必须加进来。
    pub(crate) const ALL: [Self; 4] = [
        Self::Settings,
        Self::Properties,
        Self::Preview,
        Self::Transfer,
    ];
}

impl FileBrowser {
    /// 各种类的窗口 id 存在哪：唯一一处读这些字段。
    fn auxiliary_window_id(&self, kind: AuxiliaryWindow) -> Option<window::Id> {
        match kind {
            AuxiliaryWindow::Settings => self.settings_window,
            AuxiliaryWindow::Properties => self.properties_window,
            AuxiliaryWindow::Preview => self.preview_engine.preview_window,
            AuxiliaryWindow::Transfer => self.transfer_window,
        }
    }

    pub(crate) fn auxiliary_window(&self, window: window::Id) -> Option<AuxiliaryWindow> {
        AuxiliaryWindow::ALL
            .into_iter()
            .find(|kind| self.auxiliary_window_id(*kind) == Some(window))
    }

    /// 当前打开的辅助窗口。
    pub(super) fn open_auxiliary_windows(&self) -> Vec<(AuxiliaryWindow, window::Id)> {
        AuxiliaryWindow::ALL
            .into_iter()
            .filter_map(|kind| self.auxiliary_window_id(kind).map(|id| (kind, id)))
            .collect()
    }

    /// 按种类关闭：各自的收尾（二维码会话、属性加载、预览内容……）都在 close_* 里。
    pub(super) fn close_auxiliary_window_of(&mut self, kind: AuxiliaryWindow) -> Task<Message> {
        match kind {
            AuxiliaryWindow::Settings => self.close_settings_window(),
            AuxiliaryWindow::Properties => self.close_properties_window(),
            AuxiliaryWindow::Preview => self.close_preview_window(),
            AuxiliaryWindow::Transfer => self.close_transfer_window(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowFrameState;

    fn browser_with_every_auxiliary_window() -> (FileBrowser, Vec<(AuxiliaryWindow, window::Id)>) {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let opened = AuxiliaryWindow::ALL
            .into_iter()
            .map(|kind| (kind, window::Id::unique()))
            .collect::<Vec<_>>();
        for (kind, id) in &opened {
            let slot = match kind {
                AuxiliaryWindow::Settings => &mut browser.settings_window,
                AuxiliaryWindow::Properties => &mut browser.properties_window,
                AuxiliaryWindow::Preview => &mut browser.preview_engine.preview_window,
                AuxiliaryWindow::Transfer => &mut browser.transfer_window,
            };
            *slot = Some(*id);
        }
        (browser, opened)
    }

    #[test]
    fn every_auxiliary_window_is_found_by_its_id() {
        let (browser, opened) = browser_with_every_auxiliary_window();

        for (kind, id) in &opened {
            assert_eq!(browser.auxiliary_window(*id), Some(*kind));
        }
        assert_eq!(browser.open_auxiliary_windows(), opened);
        assert_eq!(browser.auxiliary_window(browser.main_window), None);
    }

    #[test]
    fn maximizing_any_auxiliary_window_is_tracked() {
        let (mut browser, opened) = browser_with_every_auxiliary_window();

        for (_, id) in &opened {
            drop(browser.accept_window_maximized_observation(*id, WindowFrameState::Maximized));
            assert!(browser.maximized_windows.contains(id));
        }
    }
}
