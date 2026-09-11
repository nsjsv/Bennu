use iced::widget::{
    button, checkbox, column, container, mouse_area, rich_text, row, space::Space, Button,
};
use iced::{Alignment, Background, Color, Element, Length};

use crate::app::FileBrowser;
use crate::appearance::{
    context_menu_item_button_style, context_menu_style, muted_icon_svg_style, subtle_border_color,
};
use crate::icons::IconSymbol;
use crate::model::{
    ContextMenuSettingsPage, ContextMenuSettingsPageStep, ContextMenuSettingsRow, Message,
    CONTEXT_MENU_SETTINGS_PAGES,
};
use crate::typography::readable_text;

use super::option_controls::{destructive_confirmation_button_style, secondary_action_button};
use super::tab_motion::translated;
use super::{themed_icon, IconTone};

const CONTEXT_MENU_ITEM_HEIGHT: f32 = 24.0;
const CONTEXT_MENU_PADDING: f32 = 8.0;
const CONTEXT_MENU_ITEM_SPACING: f32 = 4.0;
const CONTEXT_MENU_WIDTH: f32 = 240.0;
const CONTEXT_MENU_ICON_SIZE: f32 = 14.0;
const CONTEXT_MENU_GRIP_ICON_SIZE: f32 = 14.0;
const CONTEXT_MENU_EYE_ICON_SIZE: f32 = 14.0;
const CONTEXT_MENU_LABEL_SIZE: f32 = 13.0;

/// 「右键菜单」配置区块:直接以右键菜单的样子预览当前菜单,
/// 行内拖拽排序、眼睛开关可见性;下方翻页器与恢复默认。
pub(super) fn context_menu_settings_section(browser: &FileBrowser) -> Element<'_, Message> {
    let page = browser.context_menu_settings_page;
    let position = CONTEXT_MENU_SETTINGS_PAGES
        .iter()
        .position(|candidate| *candidate == page)
        .unwrap_or(0);
    let rows = browser.user_config().context_menus.settings_rows(page);
    let entry_type_checked = entry_types_checked_for(browser, page);
    let dragged_index = browser.context_menu_settings_drag_index();
    let item_rows: Vec<Element<'static, Message>> = rows
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let checked = entry_type_checked
                .as_ref()
                .and_then(|checked| checked.get(index).copied());
            context_menu_item_row(
                page,
                index,
                entry,
                checked,
                browser.context_menu_settings_drag_offset(index),
                dragged_index == Some(index),
            )
        })
        .collect();

    let pager = container(
        row![
            pager_button(
                IconSymbol::ArrowLeft,
                Message::ContextMenuSettingsPageShifted(ContextMenuSettingsPageStep::Previous),
            ),
            readable_text(format!(
                "{}/{}",
                position + 1,
                CONTEXT_MENU_SETTINGS_PAGES.len()
            ))
            .size(14),
            pager_button(
                IconSymbol::ArrowRight,
                Message::ContextMenuSettingsPageShifted(ContextMenuSettingsPageStep::Next),
            ),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .center_x(Length::Fill);

    // 菜单名在预览面板正上方居中。
    let page_label: Element<'static, Message> = container(
        readable_text(page.label())
            .size(13)
            .width(Length::Fixed(CONTEXT_MENU_WIDTH)),
    )
    .width(Length::Fixed(CONTEXT_MENU_WIDTH))
    .center_x(Length::Fixed(CONTEXT_MENU_WIDTH))
    .into();

    column![
        center_horizontally(page_label),
        center_horizontally(menu_preview_panel(item_rows)),
        pager,
        center_horizontally(reset_row(browser, page)),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

fn center_horizontally<'a, Msg: 'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// 搜索类型筛选菜单本体是勾选行:预览需要每项的当前勾选状态。
fn entry_types_checked_for(
    browser: &FileBrowser,
    page: ContextMenuSettingsPage,
) -> Option<Vec<bool>> {
    if page != ContextMenuSettingsPage::SearchEntryTypes {
        return None;
    }
    let selected = browser.search_workspace.as_ref()?;
    let selected = &selected.filters.selected_entry_types;
    Some(
        browser
            .user_config()
            .context_menus
            .search_entry_types
            .entries
            .iter()
            .map(|entry| selected.contains(&entry.item))
            .collect(),
    )
}

/// 右键菜单样式的预览面板:与真实菜单同排版,行间分隔线与草图一致。
fn menu_preview_panel(rows: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    let mut content = column![]
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .width(Length::Fill);
    let row_count = rows.len();
    for (index, menu_row) in rows.into_iter().enumerate() {
        content = content.push(menu_row);
        if index + 1 < row_count {
            content = content.push(menu_row_separator());
        }
    }

    container(container(content).padding(CONTEXT_MENU_PADDING))
        .width(Length::Fixed(CONTEXT_MENU_WIDTH))
        .style(context_menu_style)
        .into()
}

fn menu_row_separator() -> Element<'static, Message> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0))).width(Length::Fill),
    )
    .style(menu_row_separator_style)
    .into()
}

fn menu_row_separator_style(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(Color {
            a: 0.55,
            ..subtle_border_color(theme)
        })),
        ..iced::widget::container::Style::default()
    }
}

/// 行 = 拖拽手柄 + 菜单项原样预览(图标+名称,隐藏时删除线淡化) + 眼睛开关。
/// 手柄与眼睛都是无框透明按钮,只显示图标。
fn context_menu_item_row(
    page: ContextMenuSettingsPage,
    index: usize,
    entry: &ContextMenuSettingsRow,
    entry_type_checked: Option<bool>,
    drag_offset: Option<f32>,
    is_being_dragged: bool,
) -> Element<'static, Message> {
    // 手柄光标:悬停为 Grab,拖动本行时为 Grabbing。
    let grip = mouse_area(
        container(
            themed_icon(
                IconSymbol::GripVertical,
                IconTone::Normal,
                CONTEXT_MENU_GRIP_ICON_SIZE,
            )
            .width(Length::Fixed(CONTEXT_MENU_GRIP_ICON_SIZE))
            .height(Length::Fixed(CONTEXT_MENU_GRIP_ICON_SIZE)),
        )
        .width(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center),
    )
    .interaction(if is_being_dragged {
        iced::mouse::Interaction::Grabbing
    } else {
        iced::mouse::Interaction::Grab
    })
    .on_press(Message::ContextMenuSettingsDragStarted { page, index });

    let menu_icon = if entry.visible {
        themed_icon(entry.icon, IconTone::Normal, CONTEXT_MENU_ICON_SIZE)
    } else {
        themed_icon(entry.icon, IconTone::Normal, CONTEXT_MENU_ICON_SIZE)
            .style(muted_icon_svg_style())
    };

    let item_content: Element<'static, Message> = if let Some(checked) = entry_type_checked {
        row![checkbox(checked).size(14).spacing(8), item_label(entry)]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    } else {
        row![menu_icon, item_label(entry)]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    };

    let eye_icon = if entry.visible {
        IconSymbol::Eye
    } else {
        IconSymbol::EyeOff
    };
    let eye_icon_view = themed_icon(eye_icon, IconTone::Normal, CONTEXT_MENU_EYE_ICON_SIZE);
    let eye_icon_view = if entry.visible {
        eye_icon_view
    } else {
        eye_icon_view.style(muted_icon_svg_style())
    };
    let mut eye = mouse_area(
        container(
            eye_icon_view
                .width(Length::Fixed(CONTEXT_MENU_EYE_ICON_SIZE))
                .height(Length::Fixed(CONTEXT_MENU_EYE_ICON_SIZE)),
        )
        .width(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center),
    );
    if !entry.locked {
        eye = eye.on_press(Message::ContextMenuSettingsItemToggled { page, index });
    }

    let content = row![grip, item_content, eye]
        .spacing(4)
        .align_y(Alignment::Center)
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .width(Length::Fill);

    match drag_offset {
        Some(offset) => translated(content, 0.0, offset),
        None => content.into(),
    }
}

/// 菜单项名:与真实菜单一样走本地化;隐藏时删除线并沿用容器淡化色。
fn item_label(entry: &ContextMenuSettingsRow) -> Element<'static, Message> {
    let translated_label = crate::localization::translate_current(entry.label);
    if entry.visible {
        readable_text(translated_label)
            .size(CONTEXT_MENU_LABEL_SIZE)
            .into()
    } else {
        rich_text![settings_span(translated_label).strikethrough(true)]
            .size(CONTEXT_MENU_LABEL_SIZE)
            .into()
    }
}

fn pager_button(icon: IconSymbol, message: Message) -> Button<'static, Message> {
    button(themed_icon(icon, IconTone::Normal, 14.0))
        .on_press(message)
        .padding(4)
        .width(Length::Fixed(26.0))
        .height(Length::Fixed(26.0))
        .style(context_menu_item_button_style())
}

/// 恢复默认:同一颗按钮两段式——第一次点进入红色待确认态,再点一次真正恢复;
/// 翻页或改动排序即撤销待确认态。
fn reset_row(browser: &FileBrowser, page: ContextMenuSettingsPage) -> Element<'_, Message> {
    let reset: Element<'static, Message> = if browser.context_menu_reset_confirmation == Some(page)
    {
        button(container(readable_text("Click Again to Reset").size(12)).padding([6, 10]))
            .on_press(Message::ContextMenuSettingsResetConfirmed(page))
            .style(destructive_confirmation_button_style())
            .into()
    } else {
        secondary_action_button(
            "Restore defaults",
            Message::ContextMenuSettingsResetRequested(page),
        )
        .into()
    };
    center_horizontally(reset)
}

fn settings_span(label: String) -> iced::widget::text::Span<'static, (), iced::Font> {
    iced::widget::text::Span::new(label)
}
