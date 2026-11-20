//! 文件区右键菜单（点在条目上 / 点在空白处）：一级行按用途分段，段与段之间一条细线；
//! 悬停（或键盘走到）组行时子菜单贴在旁边，第一项和触发行对齐。有哪些行由
//! `context_menu_rows` 决定，这里只画；有快捷键的行右边显示它当前的键。

use iced::widget::{button, container, mouse_area, row, Column, Space};
use iced::{Alignment, Background, Element, Length, Theme};

use crate::actions::{ActionId, ActionLibrary};
use crate::appearance::{
    context_menu_highlighted_item_button_style, context_menu_item_button_style, context_menu_style,
    muted_text_color,
};
use crate::config::UserConfig;
use crate::context_menu_rows::{file_context_menu_rows, MenuRow, MenuSection};
use crate::model::{FileContextMenuExpansion, FileContextMenuState, Message};
use crate::shortcuts::LiveKeymap;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

use super::{themed_icon, IconTone, MENU_ICON_SIZE};

pub(super) const CONTEXT_MENU_PADDING: f32 = 8.0;
pub(super) const CONTEXT_MENU_ITEM_SPACING: f32 = 4.0;
pub(super) const CONTEXT_MENU_ITEM_HEIGHT: f32 = 28.0;
/// 主菜单和子菜单同宽：最长的一项（英文“New Folder with Selection”）加上快捷键不折行。
const CONTEXT_MENU_WIDTH: f32 = 280.0;

/// 分隔线上下各留一点空，加上行间距，段与段比行与行明显分得开。
const SECTION_DIVIDER_GAP: f32 = 2.0;
const SECTION_DIVIDER_HEIGHT: f32 = 1.0 + SECTION_DIVIDER_GAP * 2.0;

/// 一级菜单从上往下排：换段时先插分隔线；同时记下每行顶边的纵坐标，
/// 子菜单要贴着触发行出来，分隔线的高度也得算进去。
struct MenuRows<'a> {
    rows: Vec<Element<'a, Message>>,
    next_top: f32,
    section: Option<MenuSection>,
}

impl<'a> MenuRows<'a> {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            next_top: CONTEXT_MENU_PADDING,
            section: None,
        }
    }

    /// 返回这一行顶边离菜单顶边多远。
    fn push(&mut self, section: MenuSection, row: Element<'a, Message>) -> f32 {
        if self.section.is_some_and(|previous| previous != section) {
            self.rows.push(section_divider());
            self.next_top += SECTION_DIVIDER_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        }
        self.section = Some(section);
        let top = self.next_top;
        self.rows.push(row);
        self.next_top += CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        top
    }

    fn into_menu(self) -> Element<'a, Message> {
        menu_frame(Column::with_children(self.rows))
    }
}

fn menu_frame(rows: Column<'_, Message>) -> Element<'_, Message> {
    container(
        rows.spacing(CONTEXT_MENU_ITEM_SPACING)
            .padding(CONTEXT_MENU_PADDING),
    )
    .width(Length::Fixed(CONTEXT_MENU_WIDTH))
    .style(context_menu_style)
    .into()
}

/// 子菜单顶边相对主菜单顶边的偏移：两边内边距一样，往上提一个内边距，
/// 子菜单第一项就和触发行在同一高度。
fn submenu_top(trigger_top: f32) -> f32 {
    trigger_top - CONTEXT_MENU_PADDING
}

fn section_divider() -> Element<'static, Message> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(ui_colors(theme).outline_variant)),
                ..container::Style::default()
            }),
    )
    .padding([SECTION_DIVIDER_GAP, 6.0])
    .width(Length::Fill)
    .into()
}

pub(super) fn file_context_menu_panel(
    menu: &FileContextMenuState,
    keyboard_modifiers: iced::keyboard::Modifiers,
    config: &UserConfig,
    actions: &ActionLibrary,
) -> (
    Element<'static, Message>,
    Option<(Element<'static, Message>, f32)>,
) {
    let keymap = config.shortcuts.live(|id| actions.contains(id));
    let rows = file_context_menu_rows(
        menu,
        &config.context_menus,
        keyboard_modifiers,
        config.file_grouping,
    );
    let highlight = menu.keyboard_highlight;
    let mut layout = MenuRows::new();
    let mut submenu = None;
    for (index, row) in rows.iter().enumerate() {
        let highlighted = highlight.is_some_and(|highlight| highlight.row == index);
        // 普通行悬停时收起别的组的子菜单；组行悬停就展开自己。
        let hover = row.expansion().unwrap_or(FileContextMenuExpansion::None);
        let top = layout.push(row.section, menu_row(row, hover, highlighted, &keymap));
        if let Some(expansion) = row
            .expansion()
            .filter(|expansion| *expansion == menu.expansion)
        {
            let member_highlight = highlight
                .filter(|highlight| highlight.row == index)
                .and_then(|highlight| highlight.member);
            let members = row
                .members()
                .iter()
                .enumerate()
                .map(|(member_index, member)| {
                    menu_row(
                        member,
                        expansion,
                        member_highlight == Some(member_index),
                        &keymap,
                    )
                })
                .collect::<Vec<_>>();
            let panel = mouse_area(menu_frame(Column::with_children(members)))
                .on_enter(Message::FileContextMenuExpansionChanged(expansion))
                .into();
            submenu = Some((panel, submenu_top(top)));
        }
    }
    (layout.into_menu(), submenu)
}

/// 一行：图标、文字、快捷键（次要文字色）、子菜单箭头或单选勾。`hover` 是鼠标移进来时
/// 菜单该展开到的状态。
fn menu_row(
    row: &MenuRow,
    hover: FileContextMenuExpansion,
    highlighted: bool,
    keymap: &LiveKeymap<'_, impl Fn(&ActionId) -> bool>,
) -> Element<'static, Message> {
    let mut content = row![]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if let Some(icon) = row.icon {
        content = content.push(themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE));
    }
    content = content.push(readable_text(row.label.clone()).width(Length::Fill));
    if let Some(key) = row
        .shortcut
        .clone()
        .and_then(|target| keymap.keys(target).first().copied())
    {
        content = content.push(readable_text(key.display_label()).size(CAPTION_SIZE).style(
            |theme: &Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            },
        ));
    }
    if row.expansion().is_some() {
        content = content.push(themed_icon(
            IconSymbol::ChevronRight,
            IconTone::Normal,
            MENU_ICON_SIZE,
        ));
    } else if row.checked {
        content = content.push(themed_icon(
            IconSymbol::Check,
            IconTone::Normal,
            MENU_ICON_SIZE,
        ));
    }
    let style = if highlighted {
        context_menu_highlighted_item_button_style()
    } else {
        context_menu_item_button_style()
    };
    mouse_area(
        button(content)
            .on_press(row.press_message())
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(style),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(hover))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder_row() -> Element<'static, Message> {
        Space::new().into()
    }

    #[test]
    fn a_divider_goes_only_between_sections_and_shifts_the_rows_below_it() {
        let step = CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        let mut rows = MenuRows::new();

        let open = rows.push(MenuSection::Open, placeholder_row());
        let open_with = rows.push(MenuSection::Open, placeholder_row());
        let copy = rows.push(MenuSection::Edit, placeholder_row());
        let move_to = rows.push(MenuSection::Edit, placeholder_row());

        // 第一行上面不画线；同段两行之间不画线；换段画一条。
        assert_eq!(rows.rows.len(), 5);
        assert_eq!(open, CONTEXT_MENU_PADDING);
        assert_eq!(open_with, open + step);
        assert_eq!(
            copy,
            open_with + step + SECTION_DIVIDER_HEIGHT + CONTEXT_MENU_ITEM_SPACING
        );
        assert_eq!(move_to, copy + step);
    }

    #[test]
    fn items_split_apart_by_the_user_form_separate_sections() {
        let mut rows = MenuRows::new();
        rows.push(MenuSection::Edit, placeholder_row());
        rows.push(MenuSection::Delete, placeholder_row());
        rows.push(MenuSection::Edit, placeholder_row());

        assert_eq!(rows.rows.len(), 5);
    }

    #[test]
    fn a_submenu_starts_level_with_the_row_that_opened_it() {
        let mut rows = MenuRows::new();
        rows.push(MenuSection::Open, placeholder_row());
        let tools = rows.push(MenuSection::Edit, placeholder_row());

        // 子菜单和主菜单内边距相同：子菜单顶边 + 内边距 = 它第一项的顶边。
        assert_eq!(submenu_top(tools) + CONTEXT_MENU_PADDING, tools);
    }
}
