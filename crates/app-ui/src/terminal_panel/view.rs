use iced::mouse::{self, Interaction, ScrollDelta};
use iced::widget::canvas::{Frame, Geometry, Text as CanvasText};
use iced::widget::{button, canvas, container, mouse_area, row, space, text};
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme};
use iced::alignment::Vertical;

use super::emulator::TerminalCell;
use super::session::{SessionId, TerminalSession};
use super::{FileBrowser, TerminalPanelMessage, PANEL_HORIZONTAL_PADDING};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::Message;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};

/// 终端字号;等宽字体在 cosmic-text 下的 advance 约 0.6em。
pub(crate) const FONT_SIZE: f32 = 14.0;
/// 单元格宽度(像素):按等宽字体 advance 近似,run 内累计误差在 2px 内。
pub(crate) const CELL_WIDTH: f32 = FONT_SIZE * 0.602;
/// 单元格高度(像素)。
pub(crate) const CELL_HEIGHT: f32 = FONT_SIZE * 1.4;
/// 底部窄条高度(收起时)。
pub(crate) const BOTTOM_BAR_HEIGHT: f32 = 28.0;
const BOTTOM_BAR_ICON_SIZE: f32 = 15.0;
/// 左端选中统计相对内容区起点的缩进(侧边栏让位宽度之外的间距)。
const SUMMARY_SIDEBAR_GAP: f32 = 10.0;
/// 展开面板顶部的分隔线 + 拖拽热区总高度。
const DRAG_HANDLE_HEIGHT: f32 = 6.0;
/// 分隔线可见宽度。
const DIVIDER_LINE_HEIGHT: f32 = 1.0;
/// 每格滚动的行数。
const WHEEL_LINES_PER_NOTCH: i32 = 3;
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
const TAB_ADD_BUTTON_SIZE: f32 = 20.0;

/// 面板高度中终端网格可用的部分:去掉拖拽手柄与标签条。
pub(crate) fn canvas_height_for_panel(height: f32) -> f32 {
    (height - DRAG_HANDLE_HEIGHT - TAB_STRIP_HEIGHT).max(CELL_HEIGHT)
}

/// 主窗口最底部区域:访达式抽屉。收起时是与内容同背景的窄条(仅顶部分隔线 +
/// 右下角图标);展开时终端取代窄条,图标消失,顶部分隔线即拖拽手柄。
/// 抽屉横贯窗宽,左段由上层贴满侧边的边栏卡片盖住;标签条与终端文字
/// 自行让出边栏宽度,从卡片右缘开始,不依赖遮挡。
pub(crate) fn terminal_panel_area(browser: &FileBrowser) -> Element<'_, Message> {
    if let Some(active_tab) = browser.terminal_panel.active_tab() {
        if browser.terminal_panel.height() >= CELL_HEIGHT {
            let focused = browser.terminal_panel.is_focused();
            let session = &active_tab.session;
            return iced::widget::column![
                resize_handle(),
                // 标签条与下方终端文字对齐,同样让出边栏宽度。
                container(terminal_tab_strip(browser))
                    .width(Length::Fill)
                    .padding(iced::Padding {
                        left: browser.sidebar_width,
                        ..iced::Padding::default()
                    }),
                container(
                    canvas(TerminalGrid { session, focused })
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fixed(canvas_height_for_panel(
                    browser.terminal_panel.height(),
                )))
                // 左侧让位给边栏列,文字从内容区起点开始。
                .padding(iced::Padding {
                    left: browser.sidebar_width,
                    right: PANEL_HORIZONTAL_PADDING,
                    ..iced::Padding::default()
                })
                .style(content_background_style),
            ]
            .into();
        }
    }
    collapsed_strip(browser)
}

/// 终端标签条:标签(标题 + 未读圆点 + ×)+ 右端「+」新建按钮。
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
    strip = strip.push(space::Space::new().width(Length::Fill));
    container(strip)
        .width(Length::Fill)
        .height(Length::Fixed(TAB_STRIP_HEIGHT))
        .padding(iced::Padding {
            left: browser.sidebar_width,
            right: PANEL_HORIZONTAL_PADDING,
            top: TAB_STRIP_VERTICAL_PADDING,
            bottom: TAB_STRIP_VERTICAL_PADDING,
        })
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
        container(text(format_middle_ellipsized_text(&title, TAB_LABEL_MAX_CHARS)).size(11))
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
        .style(crate::appearance::navigation_icon_button_style()),
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
        .on_press(Message::TerminalPanel(
            TerminalPanelMessage::TabPressed(session_id),
        ))
        .on_middle_press(Message::TerminalPanel(
            TerminalPanelMessage::TabCloseRequested(session_id),
        ))
        .on_enter(Message::TerminalPanel(
            TerminalPanelMessage::TabDragEntered(session_id),
        ))
        .on_release(Message::TerminalPanel(TerminalPanelMessage::TabDragFinished))
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
            container(space::Space::new()).width(Length::Fill).height(
                Length::Fixed(DRAG_HANDLE_HEIGHT - DIVIDER_LINE_HEIGHT),
            ),
        ]
        .width(Length::Fill),
    )
    .on_press(Message::TerminalPanel(
        TerminalPanelMessage::PanelResizeDragStarted,
    ))
    .interaction(Interaction::ResizingVertically)
    .into()
}

/// 收起态窄条(底部工具栏):顶部分隔线 + 左端窗格选中统计 + 右下角终端图标。
fn collapsed_strip(browser: &FileBrowser) -> Element<'_, Message> {
    let icon = button(
        IconSymbol::Terminal
            .view(BOTTOM_BAR_ICON_SIZE)
            .width(Length::Fixed(BOTTOM_BAR_ICON_SIZE))
            .height(Length::Fixed(BOTTOM_BAR_ICON_SIZE))
            .style(crate::appearance::icon_svg_style()),
    )
    .on_press(Message::TerminalPanel(TerminalPanelMessage::ToggleRequested))
    .padding(4.0)
    .style(crate::appearance::transparent_button_style());
    let summaries = browser.pane_selection_summaries();
    iced::widget::column![
        container(space::Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(DIVIDER_LINE_HEIGHT))
            .style(divider_line_style),
        container(
            row![
                pane_selection_summary_strip(summaries),
                space::Space::new().width(Length::Fill),
                icon.width(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT))
                    .height(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT)),
            ]
            .align_y(Vertical::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(BOTTOM_BAR_HEIGHT - DIVIDER_LINE_HEIGHT))
        // 左端统计与终端标签条同源让位侧边栏浮层,随 sidebar_width 变化收放。
        .padding(iced::Padding {
            left: browser.sidebar_width + SUMMARY_SIDEBAR_GAP,
            right: 6.0,
            ..iced::Padding::default()
        })
        .align_y(Vertical::Center)
        .style(content_background_style),
    ]
    .into()
}

/// 底部工具栏左端的窗格选中统计:文件夹图标+数量、文件图标+数量+总大小,
/// 大小只累加文件;多窗格并排,竖线分隔;没有窗格选中时整段不出现。
fn pane_selection_summary_strip(
    summaries: Vec<crate::selection_summary::PaneSelectionSummary>,
) -> Element<'static, Message> {
    const SUMMARY_ICON_SIZE: f32 = 13.0;
    const SUMMARY_TEXT_SIZE: f32 = 12.0;
    const GROUP_SPACING: f32 = 6.0;
    let mut strip = row![].spacing(18).align_y(Vertical::Center);
    for (index, summary) in summaries.into_iter().enumerate() {
        if index > 0 {
            strip = strip.push(
                container(space::Space::new())
                    .width(Length::Fixed(DIVIDER_LINE_HEIGHT))
                    .height(Length::Fixed(14.0))
                    .style(divider_line_style),
            );
        }
        let mut group = row![].spacing(GROUP_SPACING).align_y(Vertical::Center);
        if summary.directory_count > 0 {
            group = group
                .push(
                    IconSymbol::Folder
                        .view(SUMMARY_ICON_SIZE)
                        .style(crate::appearance::icon_svg_style()),
                )
                .push(text(summary.directory_count.to_string()).size(SUMMARY_TEXT_SIZE));
        }
        if summary.file_count > 0 {
            group = group
                .push(
                    IconSymbol::File
                        .view(SUMMARY_ICON_SIZE)
                        .style(crate::appearance::icon_svg_style()),
                )
                .push(
                    text(format!(
                        "{} · {}",
                        summary.file_count,
                        crate::formatting::format_file_size(summary.file_total_bytes)
                    ))
                    .size(SUMMARY_TEXT_SIZE),
                );
        }
        strip = strip.push(group);
    }
    strip.into()
}

fn divider_line_style(theme: &Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(
            crate::appearance::subtle_border_color(theme),
        )),
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

/// 把面板内坐标换算成 (row, column) 网格单元;选区消息使用。
pub(crate) fn grid_cell_for_position(position: Point) -> (usize, usize) {
    let column = (position.x / CELL_WIDTH).floor().max(0.0) as usize;
    let row = (position.y / CELL_HEIGHT).floor().max(0.0) as usize;
    (row, column)
}

/// 终端网格画布:渲染 + 鼠标选区/滚动。
struct TerminalGrid<'a> {
    session: &'a TerminalSession,
    focused: bool,
}

impl canvas::Program<Message> for TerminalGrid<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let iced::Event::Mouse(mouse_event) = event else {
            return None;
        };
        let message = match mouse_event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                TerminalPanelMessage::GridPressed {
                    position: cursor.position_over(bounds)?,
                }
            }
            mouse::Event::CursorMoved { position } => TerminalPanelMessage::GridDragged {
                position: Point::new(position.x - bounds.x, position.y - bounds.y),
            },
            mouse::Event::WheelScrolled { delta } => {
                cursor.position_over(bounds)?;
                TerminalPanelMessage::GridWheelScrolled {
                    lines: scroll_lines(delta),
                }
            }
            _ => return None,
        };
        Some(canvas::Action::publish(Message::TerminalPanel(message)).and_capture())
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let palette = theme.palette();
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), palette.background);

        let grid = self.session.emulator.renderable_frame();
        let highlight = selection_highlight_color(theme);

        for (row, line) in grid.lines.iter().enumerate() {
            let y = row as f32 * CELL_HEIGHT;
            let mut run = String::new();
            let mut run_start = 0usize;
            let mut run_style: Option<(Color, iced::Font)> = None;
            for (column, cell) in line.iter().enumerate() {
                let style = cell_text_style(cell, theme, highlight);
                match run_style {
                    Some(current) if current != style => {
                        flush_text_run(&mut frame, &mut run, run_start, y, Some(current));
                        run_start = column;
                        run_style = Some(style);
                    }
                    None => {
                        run_start = column;
                        run_style = Some(style);
                    }
                    _ => {}
                }
                run.push(cell.character);
            }
            flush_text_run(&mut frame, &mut run, run_start, y, run_style);
        }

        if self.focused {
            if let Some((row, column)) = grid.cursor {
                if grid.cursor_shape != CursorShape::Hidden {
                    draw_cursor(&mut frame, row, column, grid.cursor_shape, palette.text);
                }
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Interaction {
        // canvas 的 interaction 全局生效,必须按悬停位置门控,
        // 否则整个应用的光标都会变成输入条。
        if cursor.is_over(bounds) {
            Interaction::Text
        } else {
            Interaction::default()
        }
    }
}

fn selection_highlight_color(theme: &Theme) -> Color {
    let mut color = theme.extended_palette().primary.base.color;
    color.a = 0.35;
    color
}

fn cell_text_style(
    cell: &TerminalCell,
    theme: &Theme,
    highlight: Color,
) -> (Color, iced::Font) {
    let palette = theme.palette();
    let mut fg = terminal_color(cell.fg, palette);
    if cell.flags.contains(Flags::INVERSE) {
        fg = terminal_color(cell.bg, palette);
    }
    if cell.flags.contains(Flags::DIM) {
        fg.a *= 0.55;
    }
    if cell.selected {
        fg = highlight;
    }
    let weight = if cell.flags.contains(Flags::BOLD) {
        iced::font::Weight::Bold
    } else {
        iced::font::Weight::Normal
    };
    let style = if cell.flags.contains(Flags::ITALIC) {
        iced::font::Style::Italic
    } else {
        iced::font::Style::Normal
    };
    (
        fg,
        iced::Font {
            weight,
            style,
            ..iced::Font::MONOSPACE
        },
    )
}

fn draw_cursor(
    frame: &mut Frame,
    row: usize,
    column: usize,
    shape: CursorShape,
    color: Color,
) {
    let x = column as f32 * CELL_WIDTH;
    let y = row as f32 * CELL_HEIGHT;
    match shape {
        CursorShape::Block => frame.fill_rectangle(Point::new(x, y), Size::new(CELL_WIDTH, CELL_HEIGHT), {
            let mut background = color;
            background.a = 0.35;
            background
        }),
        CursorShape::Underline => frame.fill_rectangle(
            Point::new(x, y + CELL_HEIGHT - 2.0),
            Size::new(CELL_WIDTH, 2.0),
            color,
        ),
        _ => frame.fill_rectangle(Point::new(x, y), Size::new(2.0, CELL_HEIGHT), color),
    }
}

fn flush_text_run(
    frame: &mut Frame,
    run: &mut String,
    run_start: usize,
    y: f32,
    style: Option<(Color, iced::Font)>,
) {
    let Some((color, font)) = style else {
        return;
    };
    if run.is_empty() {
        return;
    }
    let content = std::mem::take(run);
    frame.fill_text(CanvasText {
        content,
        position: Point::new(run_start as f32 * CELL_WIDTH, y),
        max_width: f32::INFINITY,
        color,
        size: iced::Pixels(FONT_SIZE),
        line_height: iced::advanced::text::LineHeight::Absolute(iced::Pixels(CELL_HEIGHT)),
        font,
        align_x: iced::advanced::text::Alignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::advanced::text::Shaping::Advanced,
    });
}

/// ANSI 16 色(xterm 经典),其余索引色按 256 色公式;默认前景/背景取主题。
fn terminal_color(color: TermColor, palette: iced::theme::Palette) -> Color {
    match color {
        TermColor::Named(name) => named_color(name, palette),
        TermColor::Indexed(index) if index < 16 => ansi_16(index as usize),
        TermColor::Indexed(index) if index < 232 => cube_color(index),
        TermColor::Indexed(index) => grayscale_color(index),
        TermColor::Spec(rgb) => Color::from_rgb8(rgb.r, rgb.g, rgb.b),
    }
}

fn named_color(name: NamedColor, palette: iced::theme::Palette) -> Color {
    match name {
        NamedColor::Background => palette.background,
        NamedColor::Black
        | NamedColor::Red
        | NamedColor::Green
        | NamedColor::Yellow
        | NamedColor::Blue
        | NamedColor::Magenta
        | NamedColor::Cyan
        | NamedColor::White
        | NamedColor::BrightBlack
        | NamedColor::BrightRed
        | NamedColor::BrightGreen
        | NamedColor::BrightYellow
        | NamedColor::BrightBlue
        | NamedColor::BrightMagenta
        | NamedColor::BrightCyan
        | NamedColor::BrightWhite => ansi_16(plain_index(name)),
        _ => palette.text,
    }
}

fn plain_index(name: NamedColor) -> usize {
    match name {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        _ => 15,
    }
}

fn ansi_16(index: usize) -> Color {
    // VS Code 风格 16 色:在深浅两种背景下都有足够对比度;
    // 经典 xterm 色的暗红/暗蓝在深色背景上几乎不可见。
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = ANSI[index.min(15)];
    Color::from_rgb8(r, g, b)
}

fn cube_color(index: u8) -> Color {
    let levels = [0u8, 95, 135, 175, 215, 255];
    let value = index - 16;
    Color::from_rgb8(
        levels[(value / 36) as usize],
        levels[((value % 36) / 6) as usize],
        levels[(value % 6) as usize],
    )
}

fn grayscale_color(index: u8) -> Color {
    let value = 8 + (index - 232) * 10;
    Color::from_rgb8(value, value, value)
}

fn scroll_lines(delta: &ScrollDelta) -> i32 {
    match delta {
        ScrollDelta::Lines { y, .. } => {
            let magnitude = (y.abs().ceil() as i32).clamp(1, 10);
            y.signum() as i32 * WHEEL_LINES_PER_NOTCH * magnitude
        }
        ScrollDelta::Pixels { y, .. } => (y / CELL_HEIGHT).round() as i32,
    }
}
