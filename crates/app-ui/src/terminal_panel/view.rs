use iced::alignment::Vertical;
use iced::mouse::Interaction;
use iced::widget::{button, container, mouse_area, row, space, stack, text};
use iced::{Element, Length, Theme};

use super::grid::terminal_grid;
use super::session::SessionId;
use super::{FileBrowser, TerminalPanelMessage, PANEL_HORIZONTAL_PADDING};
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;

/// 终端字号。
pub(crate) const FONT_SIZE: f32 = 14.0;
/// 单元格宽度(像素):ASCII 按段整段排字,格子必须和所用等宽字体的 advance 一样宽,
/// 字才落在自己的格子里。
pub(crate) fn cell_width() -> f32 {
    FONT_SIZE * bennu_theme::font_families::monospace_advance_em()
}
/// 单元格高度(像素)。
pub(crate) const CELL_HEIGHT: f32 = FONT_SIZE * 1.4;
/// 底部窄条高度(收起时)。
pub(crate) const BOTTOM_BAR_HEIGHT: f32 = 28.0;
const BOTTOM_BAR_ICON_SIZE: f32 = 15.0;
/// 展开面板顶部的分隔线 + 拖拽热区总高度。
const DRAG_HANDLE_HEIGHT: f32 = 6.0;
/// 分隔线可见宽度。
const DIVIDER_LINE_HEIGHT: f32 = 1.0;
/// 终端标签条高度。
const TAB_STRIP_HEIGHT: f32 = 26.0;
/// 标签条上下留白。
const TAB_STRIP_VERTICAL_PADDING: f32 = 3.0;
/// 标签间距;位移动画距离也按它计算。
pub(crate) const TAB_STRIP_SPACING: f32 = 6.0;
/// 标签上关闭按钮的图标大小与槽宽。
const TAB_CLOSE_ICON_SIZE: f32 = 11.0;
const TAB_CLOSE_SLOT_WIDTH: f32 = 18.0;
/// 未读输出小圆点直径。
const TAB_UNREAD_DOT_SIZE: f32 = 5.0;
/// 未读圆点占位宽(含间距);恒定占位保证标题不因圆点出现而横跳。
const TAB_UNREAD_DOT_SLOT_WIDTH: f32 = TAB_UNREAD_DOT_SIZE + 3.0;
/// 标签标题最大字符数。
const TAB_LABEL_MAX_CHARS: usize = 20;
/// 标签条「+」按钮的图标与按钮尺寸。
const TAB_ADD_ICON_SIZE: f32 = 13.0;
pub(crate) const TAB_ADD_BUTTON_SIZE: f32 = 20.0;

/// 标签条可见性只由标签数量决定:≥2 个标签显示标签条,单标签只留右上角加号。
pub(crate) fn tab_strip_visible_for_tab_count(tab_count: usize) -> bool {
    tab_count >= 2
}

/// 标签条当前占据的高度;隐藏时为 0,这块高度让给终端画布。
fn tab_strip_height(tab_strip_visible: bool) -> f32 {
    if tab_strip_visible {
        TAB_STRIP_HEIGHT
    } else {
        0.0
    }
}

/// 面板高度中终端网格可用的部分:去掉拖拽手柄与标签条。
pub(crate) fn canvas_height_for_panel(height: f32, tab_strip_visible: bool) -> f32 {
    (height - DRAG_HANDLE_HEIGHT - tab_strip_height(tab_strip_visible)).max(CELL_HEIGHT)
}

/// 底部抽屉当前占据的高度:展开时拖拽手柄 + 标签条 + 终端画布,
/// 收起时窄条。悬浮面板安全区的底部让位用它计算,与
/// [`terminal_panel_area`] 的分支保持同一判断。
pub(crate) fn terminal_panel_area_height(browser: &FileBrowser) -> f32 {
    if browser.terminal_panel.active_tab().is_some()
        && browser.terminal_panel.height() >= CELL_HEIGHT
    {
        let tab_strip_visible = browser.terminal_panel.tab_strip_visible();
        DRAG_HANDLE_HEIGHT
            + tab_strip_height(tab_strip_visible)
            + canvas_height_for_panel(browser.terminal_panel.height(), tab_strip_visible)
    } else {
        BOTTOM_BAR_HEIGHT
    }
}

/// 主窗口最底部区域:访达式抽屉。收起时是与内容同背景的窄条(仅顶部分隔线 +
/// 右下角图标);展开时终端取代窄条,图标消失,顶部分隔线即拖拽手柄。
/// 抽屉横贯窗宽,左段由上层贴满侧边的边栏卡片盖住;标签条与终端文字
/// 自行让出边栏宽度,从卡片右缘开始,不依赖遮挡。
pub(crate) fn terminal_panel_area(browser: &FileBrowser) -> Element<'_, Message> {
    if let Some(active_tab) = browser.terminal_panel.active_tab() {
        if browser.terminal_panel.height() >= CELL_HEIGHT {
            let focused = browser.terminal_has_keyboard();
            let session = &active_tab.session;
            let tab_strip_visible = browser.terminal_panel.tab_strip_visible();
            let terminal_canvas: Element<'_, Message> = match session.disconnected() {
                None => terminal_grid(session, focused),
                // 保留已有输出,底部压一条"已断开"提示。
                Some(reason) => stack![
                    terminal_grid(session, focused),
                    container(disconnected_notice(reason))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_y(Vertical::Bottom),
                ]
                .into(),
            };
            // 单标签时标签条隐藏、画布吃满其高度,加号悬浮在画布右上角。
            let canvas_area: Element<'_, Message> = if tab_strip_visible {
                terminal_canvas
            } else {
                stack![
                    terminal_canvas,
                    // 右留白由外层画布容器提供,这里只做垂直对齐,与标签条加号同位。
                    container(add_tab_button())
                        .width(Length::Fill)
                        .height(Length::Fixed(TAB_STRIP_HEIGHT))
                        .padding(iced::Padding {
                            top: TAB_STRIP_VERTICAL_PADDING,
                            ..iced::Padding::default()
                        })
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .into()
            };
            let mut panel = iced::widget::column![resize_handle()];
            if tab_strip_visible {
                // 让位边栏的缩进由 terminal_tab_strip 内部统一处理,这里不再叠加。
                panel = panel.push(terminal_tab_strip(browser));
            }
            panel = panel.push(
                container(canvas_area)
                    .width(Length::Fill)
                    .height(Length::Fixed(canvas_height_for_panel(
                        browser.terminal_panel.height(),
                        tab_strip_visible,
                    )))
                    .padding([0.0, PANEL_HORIZONTAL_PADDING])
                    .style(content_background_style),
            );
            return panel.into();
        }
    }
    collapsed_strip(browser)
}

/// 终端标签条:标签(标题 + 未读圆点 + ×)等分整行宽度,最右端「+」新建按钮。
fn terminal_tab_strip(browser: &FileBrowser) -> Element<'_, Message> {
    let mut strip = row![].spacing(TAB_STRIP_SPACING);
    for tab in &browser.terminal_panel.tabs {
        let is_active = Some(tab.session.id) == browser.terminal_panel.active_session_id;
        strip = strip.push(terminal_tab_button(
            browser,
            tab.session.id,
            &tab.directory,
            tab.has_unread_output,
            is_active,
        ));
    }
    strip = strip.push(add_tab_button());
    container(strip)
        .width(Length::Fill)
        .height(Length::Fixed(TAB_STRIP_HEIGHT))
        .padding([TAB_STRIP_VERTICAL_PADDING, PANEL_HORIZONTAL_PADDING])
        .style(content_background_style)
        .into()
}

fn terminal_tab_button<'a>(
    browser: &'a FileBrowser,
    session_id: SessionId,
    directory: &std::path::Path,
    has_unread_output: bool,
    is_active: bool,
) -> Element<'a, Message> {
    let title = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| directory.to_string_lossy().into_owned());
    let label = row![
        space::Space::new().width(Length::Fixed(TAB_CLOSE_SLOT_WIDTH)),
        container(
            text(format_middle_ellipsized_text(&title, TAB_LABEL_MAX_CHARS)).size(CAPTION_SIZE)
        )
        .center_x(Length::Fill),
        unread_dot_slot(has_unread_output),
        button(
            IconSymbol::Close
                .view(TAB_CLOSE_ICON_SIZE)
                .width(Length::Fixed(TAB_CLOSE_ICON_SIZE))
                .height(Length::Fixed(TAB_CLOSE_ICON_SIZE))
                .style(crate::appearance::icon_svg_style()),
        )
        .on_press(Message::TerminalPanel(
            TerminalPanelMessage::TabCloseRequested(session_id),
        ))
        .padding(2.0)
        .width(Length::Fixed(TAB_CLOSE_SLOT_WIDTH))
        .style(bennu_theme::controls::icon_button_style),
    ]
    .spacing(4.0)
    .align_y(Vertical::Center)
    .width(Length::Fill);

    let tab = container(label)
        .padding([3, 6])
        .width(Length::Fill)
        .clip(true)
        .style(if is_active {
            crate::appearance::selected_tab_item_style
        } else {
            crate::appearance::tab_item_style
        });
    let shift_offset = browser
        .terminal_panel
        .tab_shift_animations
        .get(&session_id)
        .map(|animation| animation.shift_offset())
        .unwrap_or(0.0);
    let tab = super::super::view::tab_motion::translated(tab, shift_offset, 0.0);
    mouse_area(tab)
        .on_press(Message::TerminalPanel(TerminalPanelMessage::TabPressed(
            session_id,
        )))
        .on_middle_press(Message::TerminalPanel(
            TerminalPanelMessage::TabCloseRequested(session_id),
        ))
        .on_enter(Message::TerminalPanel(
            TerminalPanelMessage::TabDragEntered(session_id),
        ))
        .on_release(Message::TerminalPanel(
            TerminalPanelMessage::TabDragFinished,
        ))
        .interaction(Interaction::Pointer)
        .into()
}

/// 未读输出圆点槽:无未读时是等宽占位,避免标题横跳。
fn unread_dot_slot(has_unread_output: bool) -> Element<'static, Message> {
    if has_unread_output {
        container(space::Space::new())
            .width(Length::Fixed(TAB_UNREAD_DOT_SIZE))
            .height(Length::Fixed(TAB_UNREAD_DOT_SIZE))
            .style(unread_dot_style)
            .into()
    } else {
        space::Space::new()
            .width(Length::Fixed(TAB_UNREAD_DOT_SLOT_WIDTH))
            .into()
    }
}

fn unread_dot_style(theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(
            theme.extended_palette().primary.base.color,
        )),
        border: iced::Border {
            radius: (TAB_UNREAD_DOT_SIZE / 2.0).into(),
            ..iced::Border::default()
        },
        ..iced::widget::container::Style::default()
    }
}

fn add_tab_button() -> Element<'static, Message> {
    button(
        IconSymbol::Plus
            .view(TAB_ADD_ICON_SIZE)
            .width(Length::Fixed(TAB_ADD_ICON_SIZE))
            .height(Length::Fixed(TAB_ADD_ICON_SIZE))
            .style(crate::appearance::icon_svg_style()),
    )
    .on_press(Message::TerminalPanel(
        TerminalPanelMessage::TabAddRequested,
    ))
    .padding(3.0)
    .width(Length::Fixed(TAB_ADD_BUTTON_SIZE))
    .height(Length::Fixed(TAB_ADD_BUTTON_SIZE))
    .style(crate::appearance::transparent_button_style())
    .into()
}

/// 分隔线 + 拖拽热区:1px 可见横线,整个 6px 区域可按住拖动调高。
fn resize_handle() -> Element<'static, Message> {
    mouse_area(
        iced::widget::column![
            container(space::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(DIVIDER_LINE_HEIGHT))
                .style(divider_line_style),
            container(space::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(DRAG_HANDLE_HEIGHT - DIVIDER_LINE_HEIGHT),),
        ]
        .width(Length::Fill),
    )
    .on_press(Message::TerminalPanel(
        TerminalPanelMessage::PanelResizeDragStarted,
    ))
    .interaction(Interaction::ResizingVertically)
    .into()
}

/// 收起态窄条(底部工具栏):顶部分隔线 + 左端窗格状态统计 + 右下角终端图标。
fn collapsed_strip(browser: &FileBrowser) -> Element<'_, Message> {
    let icon = button(
        IconSymbol::Terminal
            .view(BOTTOM_BAR_ICON_SIZE)
            .width(Length::Fixed(BOTTOM_BAR_ICON_SIZE))
            .height(Length::Fixed(BOTTOM_BAR_ICON_SIZE))
            .style(crate::appearance::icon_svg_style()),
    )
    .on_press(Message::TerminalPanel(
        TerminalPanelMessage::ToggleRequested,
    ))
    .padding(4.0)
    .style(crate::appearance::transparent_button_style());
    let mut content = row![pane_status_summary_strip(
        browser.pane_status_strip_entries(),
        browser.options.include_hidden,
    )]
    .spacing(18)
    .align_y(Vertical::Center);
    if let Some(status) =
        crate::view::keyboard_status::keyboard_status_label(browser, SUMMARY_TEXT_SIZE)
    {
        content = content.push(status);
    }
    let content = content.push(space::Space::new().width(Length::Fill)).push(
        icon.width(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT))
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT)),
    );
    iced::widget::column![
        container(space::Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(DIVIDER_LINE_HEIGHT))
            .style(divider_line_style),
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT))
            // 左端统计与终端标签条、终端文字左对齐。
            .padding(iced::Padding {
                left: PANEL_HORIZONTAL_PADDING,
                right: 6.0,
                ..iced::Padding::default()
            })
            .align_y(Vertical::Center)
            .style(content_background_style),
    ]
    .into()
}

/// 底部工具栏左端常显统计:每窗格 = 选中统计(📁 数量、📄 数量 · 选中大小)
/// 加 " / 目录文件总大小";无选中只显总大小;大小未加载完显 "-"。
/// 组尾为幽灵开关:当前目录隐藏条目数,点击切换"显示隐藏文件"。
/// 多窗格并排,竖线分隔。
const SUMMARY_ICON_SIZE: f32 = 13.0;
const SUMMARY_TEXT_SIZE: f32 = CAPTION_SIZE;
const GROUP_SPACING: f32 = 6.0;
fn pane_status_summary_strip(
    entries: Vec<crate::selection_summary::PaneStatusStripEntry>,
    hidden_files_visible: bool,
) -> Element<'static, Message> {
    let mut strip = row![].spacing(18).align_y(Vertical::Center);
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            strip = strip.push(
                container(space::Space::new())
                    .width(Length::Fixed(DIVIDER_LINE_HEIGHT))
                    .height(Length::Fixed(14.0))
                    .style(divider_line_style),
            );
        }
        let mut group = row![].spacing(GROUP_SPACING).align_y(Vertical::Center);
        if let Some(selection) = entry.selection {
            if selection.directory_count > 0 {
                group = group
                    .push(
                        IconSymbol::Folder
                            .view(SUMMARY_ICON_SIZE)
                            .style(crate::appearance::icon_svg_style()),
                    )
                    .push(text(selection.directory_count.to_string()).size(SUMMARY_TEXT_SIZE));
            }
            if selection.file_count > 0 {
                group = group.push(
                    IconSymbol::File
                        .view(SUMMARY_ICON_SIZE)
                        .style(crate::appearance::icon_svg_style()),
                );
            }
        }
        let total_text = entry
            .visible_files_total_size_bytes
            .map(crate::formatting::format_file_size)
            .unwrap_or_else(|| "-".to_owned());
        // 大小段呈"选中大小 / 总大小";无选中只显总大小。
        let size_text = match entry.selection {
            Some(selection) if selection.file_count > 0 => format!(
                "{} · {} / {}",
                selection.file_count,
                crate::formatting::format_file_size(selection.file_total_bytes),
                total_text
            ),
            Some(selection) => format!(
                "{} / {}",
                crate::formatting::format_file_size(selection.file_total_bytes),
                total_text
            ),
            None => total_text,
        };
        group = group.push(text(size_text).size(SUMMARY_TEXT_SIZE));
        if let Some(hidden_count) = entry.hidden_entry_count {
            group = group.push(hidden_ghost_toggle(hidden_count, hidden_files_visible));
        }
        strip = strip.push(group);
    }
    strip.into()
}

/// 幽灵开关:幽灵图标 + 当前目录直属隐藏条目数(文件+文件夹)。
/// 隐藏内容显示中时幽灵被打叉;点击发与设置页相同的全局开关消息。
fn hidden_ghost_toggle(
    hidden_count: usize,
    hidden_files_visible: bool,
) -> Element<'static, Message> {
    let symbol = if hidden_files_visible {
        IconSymbol::GhostX
    } else {
        IconSymbol::Ghost
    };
    button(
        row![
            symbol
                .view(SUMMARY_ICON_SIZE)
                .style(crate::appearance::icon_svg_style()),
            text(hidden_count.to_string()).size(SUMMARY_TEXT_SIZE),
        ]
        .spacing(GROUP_SPACING)
        .align_y(Vertical::Center),
    )
    .on_press(Message::ShowHiddenFilesToggled)
    .padding(2.0)
    .style(crate::appearance::transparent_button_style())
    .into()
}

fn divider_line_style(theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(
            crate::appearance::subtle_border_color(theme),
        )),
        ..iced::widget::container::Style::default()
    }
}

fn disconnected_notice_text(reason: &str) -> String {
    tr!("Disconnected: %1", reason)
}

fn disconnected_notice(reason: &str) -> Element<'static, Message> {
    container(text(disconnected_notice_text(reason)))
        .width(Length::Fill)
        .padding([4, 10])
        .style(disconnected_notice_style)
        .into()
}

fn disconnected_notice_style(theme: &Theme) -> iced::widget::container::Style {
    let palette = theme.extended_palette();
    iced::widget::container::Style {
        background: Some(iced::Background::Color(palette.danger.weak.color)),
        text_color: Some(palette.danger.weak.text),
        ..iced::widget::container::Style::default()
    }
}

/// 与内容区(app_content_style)完全一致的背景。
fn content_background_style(theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.palette().background)),
        ..iced::widget::container::Style::default()
    }
}
