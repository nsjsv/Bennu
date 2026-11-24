use iced::widget::{button, container, row, scrollable, Column};
use iced::{Alignment, Element, Length};

use super::file_context_menu::CONTEXT_MENU_ITEM_HEIGHT;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use crate::app::right_preview_panel::PANEL_CONTENT_PADDING;
use crate::app::FileBrowser;
use crate::appearance::{context_menu_item_button_style, muted_text_color};
use crate::context_menu_rows::{quick_action_rows, MenuRow, MenuRowKind};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::CAPTION_SIZE;

/// 快捷操作面板：一行一个能点的项，样子跟菜单行一样。菜单里收进子菜单的（动作多了收起来的、
/// 服务菜单自己的子菜单）在面板里摊开，子菜单名写成一行小标题。
pub(super) fn quick_actions_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let rows = browser
        .quick_actions
        .menu()
        .map(|menu| quick_action_rows(menu, &browser.user_config().context_menus))
        .unwrap_or_default();
    let mut list = Column::new().spacing(2).width(Length::Fill);
    if rows.is_empty() {
        list = list.push(muted(tr!("No quick actions for these items")));
    }
    for menu_row in &rows {
        match &menu_row.kind {
            MenuRowKind::Action(_) => list = list.push(action_button(menu_row)),
            MenuRowKind::Submenu { members, .. } => {
                list = list.push(
                    container(muted(menu_row.label.clone()).size(CAPTION_SIZE)).padding([6, 0]),
                );
                for member in members {
                    list = list.push(action_button(member));
                }
            }
        }
    }
    container(scrollable(list))
        .padding(PANEL_CONTENT_PADDING)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn action_button(menu_row: &MenuRow) -> Element<'static, Message> {
    let mut content = row![]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if let Some(icon) = menu_row.icon {
        content = content.push(themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE));
    }
    content = content.push(readable_text(menu_row.label.clone()).width(Length::Fill));
    button(content)
        .on_press(menu_row.press_message())
        .width(Length::Fill)
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .style(context_menu_item_button_style())
        .into()
}

fn muted(label: String) -> iced::widget::Text<'static> {
    readable_text(label).style(|theme| iced::widget::text::Style {
        color: Some(muted_text_color(theme)),
    })
}
