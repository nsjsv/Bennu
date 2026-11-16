use std::sync::Arc;

use bennu_theme::font_families::{installed_fonts, InstalledFonts};
use iced::Task;

use super::FileBrowser;
use crate::model::Message;

/// 设置里哪一个字体选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontRole {
    Interface,
    Monospace,
}

/// 系统里装了哪些字体：要读所有字体文件，设置的外观页第一次显示时在后台读一次。
#[derive(Debug, Clone, Default)]
pub(crate) enum InstalledFontList {
    #[default]
    NotRead,
    Reading,
    Read(Arc<InstalledFonts>),
}

impl FileBrowser {
    pub(super) fn read_installed_fonts(&mut self) -> Task<Message> {
        if !matches!(self.installed_fonts, InstalledFontList::NotRead) {
            return Task::none();
        }
        self.installed_fonts = InstalledFontList::Reading;
        Task::perform(
            async {
                tokio::task::spawn_blocking(installed_fonts)
                    .await
                    .unwrap_or_default()
            },
            |fonts| Message::InstalledFontsRead(Arc::new(fonts)),
        )
    }

    pub(super) fn accept_installed_fonts(&mut self, fonts: Arc<InstalledFonts>) -> Task<Message> {
        self.installed_fonts = InstalledFontList::Read(fonts);
        Task::none()
    }

    /// 字体在第一次排版前定下（`font_families::adopt_font_families`），改了要重启才生效。
    pub(super) fn select_font(&mut self, role: FontRole, family: String) -> Task<Message> {
        let chosen = match role {
            FontRole::Interface => &mut self.user_config.interface_font,
            FontRole::Monospace => &mut self.user_config.monospace_font,
        };
        if *chosen == family {
            return Task::none();
        }
        *chosen = family;
        self.restart_notice_visible = true;
        self.persist_user_preferences_command()
    }
}

#[cfg(test)]
#[path = "font_settings_tests.rs"]
mod tests;
