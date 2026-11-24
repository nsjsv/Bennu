//! 终端网格控件：画屏幕（背景色、选区、光标、下划线、字），处理鼠标（选区、PRIMARY、中键
//! 粘贴、上报给程序、滚轮）和输入法。
//!
//! 画的时候锁一次屏幕，先把这一帧按格子算好（[`paint`]，纯函数），再画出来。鼠标、输入法
//! 直接对会话做，不发消息、不重建界面；只有「点了终端要拿键盘」发一条消息。

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse::{self, Interaction, ScrollDelta};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, LineHeight, Shaping, Wrapping};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::Shell;
use iced::keyboard::Modifiers;
use iced::{
    alignment, Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme,
};

mod paint;

use paint::{paint, GridMarks, GridPaint, Highlight, StrokeKind};

use super::emulator::grid_point;
use super::input::{mouse_report, wheel_as_arrows, MouseAction, ReportedButton};
use super::links::{link_at, ScreenSpan, TerminalLink};
use super::palette::Palette;
use super::search::TerminalSearch;
use super::session::TerminalSession;
use super::view::{cell_width, CELL_HEIGHT, FONT_SIZE};
use super::TerminalPanelMessage;
use crate::model::Message;
use bennu_theme::control_pointer::LinkPointer;

/// 下划线、删除线的粗细；光标竖线、下划线形状的粗细。
const STROKE_WIDTH: f32 = 1.0;
const CURSOR_STROKE_WIDTH: f32 = 2.0;
/// 滚轮一格滚几行。
const WHEEL_LINES_PER_NOTCH: f32 = 3.0;

pub(super) fn terminal_grid<'a>(
    session: &'a TerminalSession,
    focused: bool,
    search: Option<&'a TerminalSearch>,
    links: LinkPointer,
) -> Element<'a, Message> {
    Element::new(TerminalGrid {
        session,
        focused,
        search,
        links,
    })
}

struct TerminalGrid<'a> {
    session: &'a TerminalSession,
    focused: bool,
    /// 这个标签开着回看搜索时的匹配。
    search: Option<&'a TerminalSearch>,
    links: LinkPointer,
}

/// 控件自己的状态：鼠标、输入法。
#[derive(Default)]
struct GridState {
    modifiers: Modifiers,
    /// 左键按着在拖选区。
    selecting: bool,
    last_click: Option<mouse::Click>,
    /// 上报给程序时按着的键，和上次报过的格子（移动只在换格时报）。
    reported_button: Option<ReportedButton>,
    last_reported_cell: Option<(usize, usize)>,
    /// 触摸板一次滚不满一行时攒着。
    wheel_lines: f32,
    preedit: Option<input_method::Preedit>,
    /// 按着 Ctrl 时指针下的链接：画下划线，点了打开。
    ctrl_link: Option<(TerminalLink, ScreenSpan)>,
}

impl<Renderer> Widget<Message, Theme, Renderer> for TerminalGrid<'_>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GridState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GridState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fill, Length::Fill)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let palette = Palette::for_theme(theme);
        let state = tree.state.downcast_ref::<GridState>();
        let marks = GridMarks {
            link: state.ctrl_link.as_ref().map(|(_, span)| *span),
            matches: self.search.map(TerminalSearch::matches).unwrap_or(&[]),
            current_match: self.search.and_then(TerminalSearch::current),
        };
        let painted = {
            let term = self.session.emulator.lock();
            paint(
                term.renderable_content(),
                &palette,
                Highlight::for_theme(theme),
                self.focused,
                marks,
            )
        };
        renderer.with_layer(bounds, |renderer| {
            draw_paint(renderer, &painted, bounds);
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<GridState>();
        let bounds = layout.bounds();
        match event {
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                self.refresh_ctrl_link(state, cursor.position_over(bounds), bounds, shell);
            }
            Event::Mouse(mouse_event) => {
                self.mouse_event(state, mouse_event, bounds, cursor, clipboard, shell);
            }
            Event::InputMethod(ime_event) if self.focused => {
                self.input_method_event(state, ime_event, shell);
            }
            Event::Window(iced::window::Event::RedrawRequested(_)) if self.focused => {
                let term = self.session.emulator.lock();
                let point = term.grid().cursor.point;
                let line = point.line.0 + term.grid().display_offset() as i32;
                let cursor_area = Rectangle::new(
                    Point::new(
                        bounds.x + point.column.0 as f32 * cell_width(),
                        bounds.y + line.max(0) as f32 * CELL_HEIGHT,
                    ),
                    Size::new(cell_width(), CELL_HEIGHT),
                );
                shell.request_input_method(&InputMethod::Enabled {
                    cursor: cursor_area,
                    purpose: input_method::Purpose::Terminal,
                    preedit: state.preedit.as_ref().map(input_method::Preedit::as_ref),
                });
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> Interaction {
        // 光标形状全局生效，只在悬停在网格上时给（iced 0.14 陷阱）。
        if !cursor.is_over(layout.bounds()) {
            return Interaction::None;
        }
        if tree.state.downcast_ref::<GridState>().ctrl_link.is_some() {
            self.links.mark_hovered();
            return Interaction::Pointer;
        }
        if self
            .session
            .emulator
            .mode()
            .intersects(TermMode::MOUSE_MODE)
        {
            Interaction::Idle
        } else {
            Interaction::Text
        }
    }
}

impl TerminalGrid<'_> {
    fn mouse_event(
        &self,
        state: &mut GridState,
        event: &mouse::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let mode = self.session.emulator.mode();
        // 程序要鼠标（vim、htop、mc）时报给它；按住 Shift 照常选区、粘贴（终端通行做法）。
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !state.modifiers.shift();
        match event {
            mouse::Event::ButtonPressed(button) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                shell.publish(Message::TerminalPanel(TerminalPanelMessage::GridPressed));
                shell.capture_event();
                // Ctrl+点击打开链接，程序要鼠标也一样（终端里 Ctrl+点击通常就是打开）。
                if let (mouse::Button::Left, Some((link, _))) = (button, &state.ctrl_link) {
                    shell.publish(Message::TerminalPanel(TerminalPanelMessage::LinkOpened(
                        link.clone(),
                    )));
                    return;
                }
                let cell = self.cell_at(position, bounds);
                let reported = match button {
                    mouse::Button::Left => Some(ReportedButton::Left),
                    mouse::Button::Middle => Some(ReportedButton::Middle),
                    mouse::Button::Right => Some(ReportedButton::Right),
                    _ => None,
                };
                if let (true, Some(reported)) = (reporting, reported) {
                    state.reported_button = Some(reported);
                    state.last_reported_cell = Some((cell.line, cell.column));
                    self.report(MouseAction::Press(reported), cell, state.modifiers, mode);
                    return;
                }
                match button {
                    mouse::Button::Left => self.start_selection(state, position, cell),
                    mouse::Button::Middle => {
                        if let Some(text) = clipboard.read(clipboard::Kind::Primary) {
                            self.session.paste(text.as_bytes());
                        }
                    }
                    mouse::Button::Right => {
                        shell.publish(Message::TerminalPanel(TerminalPanelMessage::MenuRequested))
                    }
                    _ => {}
                }
                shell.request_redraw();
            }
            mouse::Event::ButtonReleased(button) => {
                if let Some(reported) = state.reported_button.take() {
                    let position = cursor.position().unwrap_or(Point::new(bounds.x, bounds.y));
                    let cell = self.cell_at(position, bounds);
                    state.last_reported_cell = None;
                    self.report(MouseAction::Release(reported), cell, state.modifiers, mode);
                    return;
                }
                if *button == mouse::Button::Left && state.selecting {
                    state.selecting = false;
                    // 选中即进 PRIMARY：在别的程序里中键就能粘出来。
                    match self.session.emulator.selection_string() {
                        Some(text) if !text.is_empty() => {
                            clipboard.write(clipboard::Kind::Primary, text);
                        }
                        _ => self.session.emulator.clear_selection(),
                    }
                    shell.request_redraw();
                }
            }
            mouse::Event::CursorMoved { position } => {
                let over = cursor.is_over(bounds).then_some(*position);
                self.refresh_ctrl_link(state, over, bounds, shell);
                if state.selecting {
                    let cell = self.cell_at(*position, bounds);
                    let mut term = self.session.emulator.lock();
                    let point = grid_point(&term, cell.line, cell.column);
                    if let Some(selection) = term.selection.as_mut() {
                        selection.update(point, cell.side);
                    }
                    shell.request_redraw();
                    return;
                }
                if !reporting || (state.reported_button.is_none() && !cursor.is_over(bounds)) {
                    return;
                }
                let cell = self.cell_at(*position, bounds);
                if state.last_reported_cell == Some((cell.line, cell.column)) {
                    return;
                }
                state.last_reported_cell = Some((cell.line, cell.column));
                let action = MouseAction::Move {
                    held: state.reported_button,
                };
                self.report(action, cell, state.modifiers, mode);
            }
            mouse::Event::WheelScrolled { delta } => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                shell.capture_event();
                state.wheel_lines += match delta {
                    ScrollDelta::Lines { y, .. } => y * WHEEL_LINES_PER_NOTCH,
                    ScrollDelta::Pixels { y, .. } => y / CELL_HEIGHT,
                };
                let lines = state.wheel_lines.trunc() as i32;
                if lines == 0 {
                    return;
                }
                state.wheel_lines -= lines as f32;
                if reporting {
                    let cell = self.cell_at(position, bounds);
                    let action = if lines > 0 {
                        MouseAction::WheelUp
                    } else {
                        MouseAction::WheelDown
                    };
                    for _ in 0..lines.unsigned_abs() {
                        self.report(action, cell, state.modifiers, mode);
                    }
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                    && !state.modifiers.shift()
                {
                    // less、man 这类全屏程序：滚轮滚的是内容，不是回看。
                    self.session.write_input(&wheel_as_arrows(lines));
                } else {
                    self.session.emulator.scroll(Scroll::Delta(lines));
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }

    /// 按着 Ctrl 时认指针下的链接；变了才重画。
    fn refresh_ctrl_link(
        &self,
        state: &mut GridState,
        position: Option<Point>,
        bounds: Rectangle,
        shell: &mut Shell<'_, Message>,
    ) {
        let link = match position {
            Some(position) if state.modifiers.control() => {
                let cell = self.cell_at(position, bounds);
                link_at(&self.session.emulator.lock(), cell.line, cell.column)
            }
            _ => None,
        };
        if link != state.ctrl_link {
            state.ctrl_link = link;
            shell.request_redraw();
        }
    }

    /// 一击、双击、三击分别按字、按词、按行选；Shift+点击把已有的选区扩到这里。
    fn start_selection(&self, state: &mut GridState, position: Point, cell: GridCell) {
        let click = mouse::Click::new(position, mouse::Button::Left, state.last_click);
        state.last_click = Some(click);
        state.selecting = true;
        let mut term = self.session.emulator.lock();
        let point = grid_point(&term, cell.line, cell.column);
        if state.modifiers.shift() {
            if let Some(selection) = term.selection.as_mut() {
                selection.update(point, cell.side);
                return;
            }
        }
        let kind = match click.kind() {
            mouse::click::Kind::Single => SelectionType::Simple,
            mouse::click::Kind::Double => SelectionType::Semantic,
            mouse::click::Kind::Triple => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(kind, point, cell.side));
    }

    fn report(&self, action: MouseAction, cell: GridCell, modifiers: Modifiers, mode: TermMode) {
        if let Some(bytes) = mouse_report(action, cell.line, cell.column, modifiers, mode) {
            self.session.write_input(&bytes);
        }
    }

    /// 指针所在的格子（出了网格就贴到边上的格子），以及指在格子的左半还是右半。
    fn cell_at(&self, position: Point, bounds: Rectangle) -> GridCell {
        let dimensions = self.session.emulator.dimensions();
        let columns = (position.x - bounds.x).max(0.0) / cell_width();
        let line = ((position.y - bounds.y).max(0.0) / CELL_HEIGHT) as usize;
        let column = columns as usize;
        let side = if column >= dimensions.columns || columns.fract() >= 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        GridCell {
            line: line.min(dimensions.screen_lines.saturating_sub(1)),
            column: column.min(dimensions.columns.saturating_sub(1)),
            side,
        }
    }

    fn input_method_event(
        &self,
        state: &mut GridState,
        event: &input_method::Event,
        shell: &mut Shell<'_, Message>,
    ) {
        match event {
            input_method::Event::Opened => state.preedit = Some(input_method::Preedit::new()),
            input_method::Event::Closed => state.preedit = None,
            input_method::Event::Preedit(content, selection) => {
                state.preedit = Some(input_method::Preedit {
                    content: content.clone(),
                    selection: selection.clone(),
                    text_size: Some(Pixels(FONT_SIZE)),
                });
            }
            input_method::Event::Commit(text) => {
                state.preedit = None;
                self.session.type_input(text.as_bytes());
            }
        }
        shell.capture_event();
        shell.request_redraw();
    }
}

#[derive(Debug, Clone, Copy)]
struct GridCell {
    line: usize,
    column: usize,
    side: Side,
}

fn draw_paint<Renderer>(renderer: &mut Renderer, painted: &GridPaint, bounds: Rectangle)
where
    Renderer: text::Renderer<Font = Font>,
{
    let cell = cell_width();
    let origin = |line: usize, column: usize| {
        Point::new(
            bounds.x + column as f32 * cell,
            bounds.y + line as f32 * CELL_HEIGHT,
        )
    };
    // 对齐物理像素：上下两行同色背景（mc 的整屏蓝底）接缝处不透出底色。
    let fill = |renderer: &mut Renderer, area: Rectangle, color: Color| {
        renderer.fill_quad(
            Quad {
                bounds: area,
                snap: true,
                ..Quad::default()
            },
            color,
        );
    };
    for span in &painted.backgrounds {
        let area = Rectangle::new(
            origin(span.line, span.column),
            Size::new(span.width as f32 * cell, CELL_HEIGHT),
        );
        fill(renderer, area, span.color);
    }
    for stroke in &painted.strokes {
        let start = origin(stroke.line, stroke.column);
        let width = stroke.width as f32 * cell;
        let ys: &[f32] = match stroke.kind {
            StrokeKind::Underline => &[CELL_HEIGHT - 2.0 * STROKE_WIDTH],
            StrokeKind::DoubleUnderline => &[
                CELL_HEIGHT - 4.0 * STROKE_WIDTH,
                CELL_HEIGHT - 2.0 * STROKE_WIDTH,
            ],
            StrokeKind::Strikeout => &[CELL_HEIGHT / 2.0],
        };
        for y in ys {
            let area = Rectangle::new(
                Point::new(start.x, start.y + y),
                Size::new(width, STROKE_WIDTH),
            );
            fill(renderer, area, stroke.color);
        }
    }
    if let Some(cursor) = painted.cursor {
        let start = origin(cursor.line, cursor.column);
        let width = cursor.width as f32 * cell;
        match cursor.shape {
            CursorShape::Underline => fill(
                renderer,
                Rectangle::new(
                    Point::new(start.x, start.y + CELL_HEIGHT - CURSOR_STROKE_WIDTH),
                    Size::new(width, CURSOR_STROKE_WIDTH),
                ),
                cursor.color,
            ),
            CursorShape::Beam => fill(
                renderer,
                Rectangle::new(start, Size::new(CURSOR_STROKE_WIDTH, CELL_HEIGHT)),
                cursor.color,
            ),
            _ => renderer.fill_quad(
                Quad {
                    bounds: Rectangle::new(start, Size::new(width, CELL_HEIGHT)),
                    border: Border {
                        color: cursor.color,
                        width: STROKE_WIDTH,
                        radius: 0.0.into(),
                    },
                    snap: true,
                    ..Quad::default()
                },
                Color::TRANSPARENT,
            ),
        }
    }
    for run in &painted.texts {
        let font = Font {
            weight: if run.bold {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Normal
            },
            style: if run.italic {
                iced::font::Style::Italic
            } else {
                iced::font::Style::Normal
            },
            ..Font::MONOSPACE
        };
        renderer.fill_text(
            text::Text {
                content: run.content.clone(),
                bounds: Size::new(f32::INFINITY, CELL_HEIGHT),
                size: Pixels(FONT_SIZE),
                line_height: LineHeight::Absolute(Pixels(CELL_HEIGHT)),
                font,
                align_x: text::Alignment::Left,
                align_y: alignment::Vertical::Top,
                shaping: if run.ascii {
                    Shaping::Basic
                } else {
                    Shaping::Advanced
                },
                wrapping: Wrapping::None,
            },
            origin(run.line, run.column),
            run.color,
            bounds,
        );
    }
}
