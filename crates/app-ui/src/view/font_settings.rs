use std::fmt;

use iced::{Element, Length};

use super::settings_group::{control_row, settings_group, RowNote};
use super::settings_window::SETTINGS_DROPDOWN_WIDTH;
use crate::app::font_settings::{FontRole, InstalledFontList};
use crate::app::FileBrowser;
use crate::model::Message;
use bennu_theme::controls::dropdown;

/// 下拉里的一项：字体家族名，空串是「系统默认」（跟随 fontconfig）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FontPickOption<'a>(&'a str);

impl fmt::Display for FontPickOption<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str(&tr!("System default"));
        }
        formatter.write_str(self.0)
    }
}

pub(super) fn font_settings_group(browser: &FileBrowser) -> Element<'_, Message> {
    settings_group(
        tr!("Fonts"),
        None,
        vec![
            control_row(
                tr!("Interface font"),
                None,
                font_dropdown(browser, FontRole::Interface),
            ),
            control_row(
                tr!("Monospace font"),
                Some(RowNote::Hint(tr!("Code previews and the terminal"))),
                font_dropdown(browser, FontRole::Monospace),
            ),
        ],
    )
}

fn font_dropdown(browser: &FileBrowser, role: FontRole) -> Element<'_, Message> {
    let config = browser.user_config();
    let chosen = match role {
        FontRole::Interface => config.interface_font.as_str(),
        FontRole::Monospace => config.monospace_font.as_str(),
    };
    let mut options = vec![FontPickOption("")];
    if let InstalledFontList::Read(fonts) = &browser.installed_fonts {
        let families = match role {
            FontRole::Interface => &fonts.families,
            FontRole::Monospace => &fonts.monospace,
        };
        options.extend(families.iter().map(|family| FontPickOption(family)));
    }
    // 列表还没读完、或选的字体已经卸掉：选中项照样要显示出来。
    if !options.contains(&FontPickOption(chosen)) {
        options.push(FontPickOption(chosen));
    }

    dropdown(options, Some(FontPickOption(chosen)), move |selected| {
        Message::FontSelected(role, selected.0.to_owned())
    })
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}
