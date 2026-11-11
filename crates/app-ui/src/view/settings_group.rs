//! 分组卡片：设置页和借用它的弹窗共用。
//!
//! 卡片统一负责行的内边距、分隔线和悬停底色；行只能由 [`rows`] 里的骨架做出来
//! （[`SettingsRow`] 的字段是私有的）。以前卡片收任意 Element、各页自己加内边距，
//! 漏加的行左边就对不齐、分隔线贴字——现在裸 row/column 放不进卡片。

mod rows;

pub(super) use rows::{
    action_row, choice_row, control_row, custom_row, destructive_action_row, entry_row,
    expander_row, label_row, link_row, navigation_row, notice_row, switch_row, value_row, EntryRow,
};

use iced::widget::{button, column, container, Column, Space};
use iced::{Background, Border, Color, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::{base_text_color, muted_text_color, subtle_border_color};
use crate::model::{Message, ScrollbarRegion};
use bennu_preview::panel_text::readable_text;
use bennu_theme::metrics::{CARD_INSET, CARD_RADIUS, PAGE_MAX_WIDTH, ROW_PADDING_X, ROW_RADIUS};
use bennu_theme::typography::{BOLD, CAPTION_SIZE, HEADING_SIZE};
use bennu_theme::ui_colors;

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::option_controls::hover_background_color;

/// 组与组之间的距离（紧凑密度定稿值，见 ui-style-preferences）。
pub(super) const SETTINGS_GROUP_SPACING: f32 = 16.0;
const GROUP_HEADER_SPACING: f32 = 6.0;

/// 卡片里的一行。只能由骨架函数构造。
pub(super) struct SettingsRow<'a>(Element<'a, Message>);

/// 行的附注：标题下面一行小字。提示是灰字，错误是红字；同一时刻只显示一条。
pub(super) enum RowNote {
    Hint(String),
    Error(String),
}

/// 一个设置分类页：内容最宽 [`PAGE_MAX_WIDTH`] 并居中，可滚动。
pub(super) fn settings_page<'a>(
    browser: &FileBrowser,
    sections: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let region = ScrollbarRegion::Settings;
    let sections = Column::with_children(sections)
        .spacing(SETTINGS_GROUP_SPACING)
        .width(Length::Fill)
        .max_width(PAGE_MAX_WIDTH);
    auxiliary_detail_scroller(
        column![container(sections).center_x(Length::Fill)],
        region.clone(),
        browser.scrollbar_visibility_for(&region),
        browser.scrollbar_viewport_for(&region),
        Message::SettingsScrolled,
    )
}

pub(super) fn settings_group<'a>(
    header: String,
    description: Option<String>,
    rows: Vec<SettingsRow<'a>>,
) -> Element<'a, Message> {
    let mut heading = column![readable_text(header).size(HEADING_SIZE).font(BOLD)].spacing(2);
    if let Some(description) = description {
        heading = heading.push(muted_setting_text(description));
    }
    column![
        container(heading)
            .padding([0.0, CARD_INSET + ROW_PADDING_X])
            .width(Length::Fill),
        settings_card(rows),
    ]
    .spacing(GROUP_HEADER_SPACING)
    .width(Length::Fill)
    .into()
}

pub(super) fn settings_card<'a>(rows: Vec<SettingsRow<'a>>) -> Element<'a, Message> {
    let row_count = rows.len();
    let mut content = Column::new().width(Length::Fill);
    for (index, SettingsRow(row)) in rows.into_iter().enumerate() {
        content = content.push(row);
        if index + 1 < row_count {
            content = content.push(row_separator());
        }
    }

    container(content)
        .padding(CARD_INSET)
        .width(Length::Fill)
        .style(settings_card_style)
        .into()
}

/// 灰色说明小字（组说明、弹窗里的状态提示）。
pub(super) fn muted_setting_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

fn row_separator() -> Element<'static, Message> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(separator_line_style),
    )
    .padding([0.0, ROW_PADDING_X])
    .width(Length::Fill)
    .into()
}

fn settings_card_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container)),
        text_color: Some(base_text_color(theme)),
        border: Border {
            radius: CARD_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

fn separator_line_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: 0.55,
            ..subtle_border_color(theme)
        })),
        ..container::Style::default()
    }
}

/// 整行可点的行：平时透明，悬停淡底，圆角与卡片同心。
pub(super) fn card_row_button_style() -> fn(&Theme, button::Status) -> button::Style {
    card_row_button_appearance
}

fn card_row_button_appearance(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(hover_background_color(theme)),
        button::Status::Active | button::Status::Disabled => None,
    };

    button::Style {
        background: background.map(Background::Color),
        text_color: match status {
            button::Status::Disabled => muted_text_color(theme),
            _ => base_text_color(theme),
        },
        border: Border {
            radius: ROW_RADIUS.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
