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

    /// 正文字号换算出的界面缩放，所有窗口一样；iced 每帧按它排版，所以选了马上生效。
    pub(crate) fn interface_scale(&self) -> f32 {
        bennu_theme::typography::interface_scale(self.user_config.interface_text_size)
    }

    pub(super) fn select_text_size(&mut self, text_size: u16) -> Task<Message> {
        if self.user_config.interface_text_size == text_size {
            return Task::none();
        }
        self.user_config.interface_text_size = text_size;
        Task::batch([
            self.persist_user_preferences_command(),
            self.reread_preview_window_size(),
        ])
    }

    /// 界面一放大缩小，预览窗口的布局尺寸就变了，可窗口实际没变大小，系统不会发 Resized：按新
    /// 比例取回它的大小，走窗口变大小的同一处理。主窗口记的是不随缩放变的大小，不用取。
    pub(super) fn reread_preview_window_size(&self) -> Task<Message> {
        let Some(window) = self.preview_engine.preview_window else {
            return Task::none();
        };
        iced::window::size(window)
            .map(move |size| Message::AuxiliaryWindowResized(window, size.width, size.height))
    }
}

#[cfg(test)]
#[path = "font_settings_tests.rs"]
mod tests;
