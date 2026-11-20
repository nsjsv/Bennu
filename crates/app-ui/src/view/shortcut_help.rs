//! F1 快捷键一览：居中的对话框，分组分两栏排，内容多时自己滚动。

use iced::widget::{column, row, scrollable, Column};
use iced::{Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, muted_text_color,
};
use crate::model::Message;
use crate::shortcuts::{shortcut_help_sections, HelpSection};
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};

use super::dialog::Dialog;

const SHORTCUT_HELP_WIDTH: f32 = 820.0;
const SHORTCUT_HELP_MAX_HEIGHT: f32 = 680.0;
const KEYS_COLUMN_WIDTH: f32 = 180.0;

pub(super) fn shortcut_help_panel(browser: &FileBrowser) -> Element<'static, Message> {
    let config = browser.user_config();
    let sections = shortcut_help_sections(
        &browser.keymap(),
        browser.action_library.shortcut_targets(),
        config.letter_mode,
    );
    // 两栏按行数大致对半分：每一组放进当前较短的那一栏。
    let mut columns = [Column::new().spacing(18), Column::new().spacing(18)];
    let mut heights = [0usize; 2];
    for section in sections {
        let shorter = usize::from(heights[1] < heights[0]);
        heights[shorter] += section.rows.len() + 2;
        let [left, right] = columns;
        columns = if shorter == 0 {
            [left.push(section_view(section)), right]
        } else {
            [left, right.push(section_view(section))]
        };
    }
    let [left, right] = columns;
    let body = scrollable(
        row![left.width(Length::Fill), right.width(Length::Fill)]
            .spacing(28)
            .padding(iced::Padding::default().right(12.0)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .direction(enhanced_vertical_scrollbar_direction())
    .style(enhanced_scrollbar_style);
    Dialog::new(tr!("Keyboard Shortcuts"), body)
        .width(SHORTCUT_HELP_WIDTH)
        .fill_height(SHORTCUT_HELP_MAX_HEIGHT)
        .into()
}

fn section_view(section: HelpSection) -> Element<'static, Message> {
    let mut rows = column![readable_text(section.title).font(BOLD)].spacing(6);
    for help_row in section.rows {
        rows = rows.push(
            row![
                readable_text(help_row.keys)
                    .size(CAPTION_SIZE)
                    .width(Length::Fixed(KEYS_COLUMN_WIDTH)),
                readable_text(help_row.label)
                    .size(CAPTION_SIZE)
                    .width(Length::Fill)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(muted_text_color(theme)),
                    }),
            ]
            .spacing(8),
        );
    }
    rows.into()
}
