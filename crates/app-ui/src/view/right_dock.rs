use iced::mouse::Interaction;
use iced::widget::{button, column, container, mouse_area, row, stack, text, Column, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::right_dock::{
    RightDockMessage, DIVIDER_HEIGHT, HEADER_HEIGHT, RESIZE_HANDLE_WIDTH,
};
use crate::app::FileBrowser;
use crate::appearance::{app_content_style, column_resize_divider_style};
use crate::model::Message;
use crate::right_dock::{DockGroup, DockPanel, DropTarget};
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;

/// 拖面板时边上那条落点线的粗细。
const DROP_LINE_HEIGHT: f32 = 3.0;
const CLOSE_ICON_SIZE: f32 = 11.0;
const CLOSE_SLOT_WIDTH: f32 = 18.0;

pub(crate) fn dock_panel_label(panel: DockPanel) -> String {
    match panel {
        DockPanel::Preview => tr!("Preview"),
        DockPanel::Info => tr!("Info"),
        DockPanel::TaskLog => tr!("Tasks"),
        DockPanel::QuickActions => tr!("Quick Actions"),
        DockPanel::FolderTree => tr!("Folders"),
    }
}

/// 右侧停靠区：左缘拖宽带 + 从上到下几格，格间是可拖的分隔线。只在栏显示时挂进窗格那一行；
/// 宽度用生效值，窗口窄时先让路。
pub(crate) fn right_dock(browser: &FileBrowser) -> Element<'_, Message> {
    let handle: Element<'_, Message> = mouse_area(
        row![
            container(Space::new())
                .width(Length::Fixed(1.0))
                .height(Length::Fill)
                .style(column_resize_divider_style),
            Space::new().width(Length::Fixed(RESIZE_HANDLE_WIDTH - 1.0)),
        ]
        .height(Length::Fill),
    )
    .on_press(Message::RightDock(RightDockMessage::WidthDragStarted))
    .interaction(Interaction::ResizingHorizontally)
    .into();
    let layout = browser.right_dock.layout();
    let drop = browser.right_dock_panel_drop().map(|(_, target)| target);
    let mut groups = Column::new().width(Length::Fill).height(Length::Fill);
    for (index, group) in layout.groups().iter().enumerate() {
        if index > 0 {
            groups = groups.push(divider(index - 1));
        }
        groups = groups.push(group_view(browser, index, group, layout.is_locked(), drop));
    }
    // 手柄必须留在外层 row:Fill 高度与 FillPortion 同列会按 1 份参与分配,整条左缘拖拽带
    // 会被压到 ~1px。
    row![handle, groups]
        .width(Length::Fixed(browser.right_dock_effective_width()))
        .height(Length::Fill)
        .into()
}

/// 一格：标题栏或标签条（锁定且只有一个面板时没有）+ 面板内容，叠一层拖动落点提示。
/// 提示层一直在（不拖时是空的）：按条件包一层会改控件树，预览里的滚动之类的状态会被重建。
fn group_view<'a>(
    browser: &'a FileBrowser,
    index: usize,
    group: &'a DockGroup,
    locked: bool,
    drop: Option<DropTarget>,
) -> Element<'a, Message> {
    let mut body = Column::new().width(Length::Fill).height(Length::Fill);
    match (locked, group.panels()) {
        (true, [_]) => {}
        (false, [panel]) => body = body.push(title_bar(*panel)),
        (_, panels) => body = body.push(tab_strip(panels, group.active_panel(), locked)),
    }
    body = body.push(
        container(panel_content(browser, group.active_panel()))
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(app_content_style),
    );
    let indicator = match drop {
        Some(target) => drop_indicator(target, index),
        None => Space::new().into(),
    };
    // 面板里自己用右键的（文本预览的复制菜单）先吃掉事件，这里收不到。
    let cell = mouse_area(
        stack![body, indicator]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_right_press(Message::RightDock(RightDockMessage::MenuRequested));
    container(cell)
        .width(Length::Fill)
        .height(Length::FillPortion(group.portion()))
        .into()
}

fn panel_content(browser: &FileBrowser, panel: DockPanel) -> Element<'_, Message> {
    match panel {
        DockPanel::Preview => browser.right_preview_panel_content(),
        DockPanel::Info => super::info_panel::info_panel_content(browser),
        DockPanel::TaskLog => super::task_log_panel::task_log_panel(browser),
        DockPanel::QuickActions => super::quick_actions_panel::quick_actions_panel(browser),
        DockPanel::FolderTree => super::folder_tree_panel::folder_tree_panel(browser),
    }
}

/// 解锁时单个面板的标题栏：名字 + 关闭；按住能拖。
fn title_bar(panel: DockPanel) -> Element<'static, Message> {
    let bar = row![
        text(dock_panel_label(panel)).size(CAPTION_SIZE),
        Space::new().width(Length::Fill),
        close_button(panel),
    ]
    .align_y(Alignment::Center)
    .padding([0, 8])
    .height(Length::Fixed(HEADER_HEIGHT));
    mouse_area(container(bar).width(Length::Fill).style(app_content_style))
        .on_press(Message::RightDock(RightDockMessage::PanelPressed(panel)))
        .interaction(Interaction::Grab)
        .into()
}

/// 叠了几个面板时的标签条：标签等宽，当前的那个高亮；解锁时每个标签能关、能拖。
fn tab_strip(panels: &[DockPanel], active: DockPanel, locked: bool) -> Element<'static, Message> {
    let mut strip = row![]
        .spacing(4)
        .padding([3, 6])
        .height(Length::Fixed(HEADER_HEIGHT));
    for &panel in panels {
        let mut label = row![
            container(text(dock_panel_label(panel)).size(CAPTION_SIZE)).center_x(Length::Fill)
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill);
        if !locked {
            label = label.push(close_button(panel));
        }
        let tab = container(label)
            .padding([2, 6])
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(if panel == active {
                crate::appearance::selected_tab_item_style
            } else {
                crate::appearance::tab_item_style
            });
        strip = strip.push(
            mouse_area(tab)
                .on_press(Message::RightDock(RightDockMessage::PanelPressed(panel)))
                .interaction(if locked {
                    Interaction::Pointer
                } else {
                    Interaction::Grab
                }),
        );
    }
    container(strip)
        .width(Length::Fill)
        .style(app_content_style)
        .into()
}

fn close_button(panel: DockPanel) -> Element<'static, Message> {
    button(
        IconSymbol::Close
            .view(CLOSE_ICON_SIZE)
            .width(Length::Fixed(CLOSE_ICON_SIZE))
            .height(Length::Fixed(CLOSE_ICON_SIZE))
            .style(crate::appearance::icon_svg_style()),
    )
    .on_press(Message::RightDock(RightDockMessage::PanelClosed(panel)))
    .padding(2.0)
    .width(Length::Fixed(CLOSE_SLOT_WIDTH))
    .style(bennu_theme::controls::icon_button_style)
    .into()
}

/// 格间分隔线:整条都是拖拽热区,中缝画 1px 线。
fn divider(upper: usize) -> Element<'static, Message> {
    let top_padding = (DIVIDER_HEIGHT - 1.0) / 2.0;
    mouse_area(
        column![
            Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(top_padding)),
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(column_resize_divider_style),
            Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(DIVIDER_HEIGHT - 1.0 - top_padding)),
        ]
        .width(Length::Fill),
    )
    .on_press(Message::RightDock(RightDockMessage::DividerDragStarted(
        upper,
    )))
    .interaction(Interaction::ResizingVertically)
    .into()
}

/// 这格是落点时的提示：叠进来是整格淡色，上下另起一格是那条边上一道粗线。
fn drop_indicator(target: DropTarget, index: usize) -> Element<'static, Message> {
    let line = || {
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(DROP_LINE_HEIGHT))
            .style(drop_line_style)
    };
    match target {
        DropTarget::Join(group) if group == index => container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(drop_area_style)
            .into(),
        DropTarget::Above(group) if group == index => {
            column![line(), Space::new().height(Length::Fill)].into()
        }
        DropTarget::Below(group) if group == index => {
            column![Space::new().height(Length::Fill), line()].into()
        }
        _ => Space::new().into(),
    }
}

fn drop_line_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(
            theme.extended_palette().primary.base.color,
        )),
        ..container::Style::default()
    }
}

fn drop_area_style(theme: &Theme) -> container::Style {
    let mut color = theme.extended_palette().primary.base.color;
    color.a = 0.12;
    container::Style {
        background: Some(iced::Background::Color(color)),
        border: iced::Border {
            color: theme.extended_palette().primary.base.color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}
