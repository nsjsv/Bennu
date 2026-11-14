//! 查看器的指针与键盘交互：选择（含拖出边界自动滚动）、右键菜单、Esc、复制提示、
//! 滚轮缩放、快捷键。update 只负责把结果交给 shell。

use std::time::{Duration, Instant};

use bennu_localization::tr;
use bennu_theme::styles::{context_menu_item_button_style, context_menu_style, muted_text_color};
use bennu_theme::typography::BODY_SIZE;
use bennu_theme::ui_colors;
use iced::advanced::text::Renderer as TextRenderer;
use iced::advanced::{renderer, text, Clipboard};
use iced::widget::{button, text_editor};
use iced::{
    alignment, keyboard, mouse, window, Background, Border, Event, Font, Pixels, Point, Rectangle,
    Size, Theme,
};

use super::geometry::scroll_viewer_by;
use super::render::text_bounds;
use super::selection::{self, apply_key_binding};
use super::{
    refresh_retained_content, scrollbar, view_shortcut, PreviewParagraph, TextPreviewViewerState,
};
use crate::text_document::{TextPreviewDocument, TextViewerEvent};
use crate::text_view::TextViewChange;

/// 拖出边界时每越界 1 px 每秒滚的像素，以及最快速度。
const AUTOSCROLL_GAIN: f32 = 12.0;
const AUTOSCROLL_MAX_SPEED: f32 = 6_000.0;
/// 两帧之间最多按这么久算，切窗口回来不会一下滚很远。
const AUTOSCROLL_MAX_STEP: Duration = Duration::from_millis(50);
const MENU_WIDTH: f32 = 168.0;
const MENU_ITEM_HEIGHT: f32 = 30.0;
const MENU_PADDING: f32 = 4.0;
const NOTICE_DURATION: Duration = Duration::from_millis(2_500);

/// 右键菜单：位置相对查看器左上角。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ContextMenu {
    origin: Point,
    hovered: Option<MenuItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    Copy,
    SelectAll,
}

const MENU_ITEMS: [MenuItem; 2] = [MenuItem::Copy, MenuItem::SelectAll];

/// 查看器底部的短暂提示（复制超限）。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Notice {
    text: String,
    until: Instant,
}

/// 一次事件处理的结果：是否捕获、要不要重绘、有没有滚轮增量要交给宿主的平滑滚动。
#[derive(Debug, Default)]
pub(super) struct EventOutcome {
    pub(super) captured: bool,
    pub(super) redraw: Option<window::RedrawRequest>,
    pub(super) wheel: Option<mouse::ScrollDelta>,
}

impl EventOutcome {
    fn captured() -> Self {
        Self {
            captured: true,
            redraw: Some(window::RedrawRequest::NextFrame),
            wheel: None,
        }
    }

    fn redraw() -> Self {
        Self {
            captured: false,
            redraw: Some(window::RedrawRequest::NextFrame),
            wheel: None,
        }
    }
}

pub(super) fn handle_event(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
    clipboard: &mut dyn Clipboard,
) -> EventOutcome {
    if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
        state.modifiers = *modifiers;
    }
    if let Event::Window(window::Event::RedrawRequested(now)) = event {
        return tick(state, document, *now);
    }
    if state.context_menu.is_some() {
        if let Some(outcome) = handle_menu_event(state, document, event, bounds, cursor, clipboard)
        {
            return outcome;
        }
    }
    if scrollbar::handle_event(state, document, event, bounds, cursor) {
        return EventOutcome::captured();
    }
    let text_bounds = text_bounds(bounds, state.gutter_width);
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
            if let Some(position) = cursor.position_in(text_bounds) {
                let position = Point::new(position.x + state.scroll_x, position.y);
                let click = selection::press(state, document, position);
                state.last_click = Some(click);
                // 双击选词、三击选段：选区已定，写主选区。
                if !matches!(click.kind(), iced::advanced::mouse::click::Kind::Single) {
                    selection::publish_primary(state, document, clipboard);
                }
                return EventOutcome::captured();
            }
            if state.is_focused && !cursor.is_over(bounds) {
                state.is_focused = false;
                state.drag_click = None;
            }
            EventOutcome::default()
        }
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
            let Some(position) = cursor.position_in(text_bounds) else {
                return EventOutcome::default();
            };
            open_context_menu(state, document, position, cursor, bounds);
            EventOutcome::captured()
        }
        Event::Mouse(mouse::Event::CursorMoved { position }) => drag_selection(
            state,
            document,
            *position - text_bounds.position(),
            text_bounds,
        ),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            let was_dragging = state.drag_active;
            state.autoscroll_velocity = 0.0;
            if !selection::release(state) {
                return EventOutcome::default();
            }
            if was_dragging {
                selection::publish_primary(state, document, clipboard);
            }
            EventOutcome {
                captured: true,
                ..EventOutcome::default()
            }
        }
        Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
            // Ctrl+滚轮缩放字号，其余交给宿主的平滑滚动管线。
            if state.modifiers.command() {
                let upward = match delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => {
                        *y > 0.0
                    }
                };
                let change = if upward {
                    TextViewChange::ZoomIn
                } else {
                    TextViewChange::ZoomOut
                };
                state
                    .pending_events
                    .push(TextViewerEvent::ChangeView(change));
                return EventOutcome::captured();
            }
            EventOutcome {
                captured: true,
                redraw: None,
                wheel: Some(*delta),
            }
        }
        Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modified_key,
            physical_key,
            modifiers,
            text,
            ..
        }) if state.is_focused => {
            if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                return escape(state);
            }
            if let Some(change) = view_shortcut(key, *modifiers) {
                state
                    .pending_events
                    .push(TextViewerEvent::ChangeView(change));
                return EventOutcome::captured();
            }
            let Some(binding) = text_editor::Binding::<()>::from_key_press(text_editor::KeyPress {
                key: key.clone(),
                modified_key: modified_key.clone(),
                physical_key: *physical_key,
                modifiers: *modifiers,
                text: text.clone(),
                status: text_editor::Status::Focused {
                    is_hovered: cursor.is_over(bounds),
                },
            }) else {
                return EventOutcome::default();
            };
            let changes_selection = changes_selection(&binding);
            apply_key_binding(binding, state, document, clipboard);
            refresh_retained_content(state, document);
            selection::reveal_cursor_horizontally(state, document);
            if changes_selection {
                selection::publish_primary(state, document, clipboard);
            }
            EventOutcome::captured()
        }
        _ => EventOutcome::default(),
    }
}

/// Shift+方向键、全选、选词、选行：选区变了，要写主选区。
fn changes_selection(binding: &text_editor::Binding<()>) -> bool {
    match binding {
        text_editor::Binding::Select(_)
        | text_editor::Binding::SelectAll
        | text_editor::Binding::SelectWord
        | text_editor::Binding::SelectLine => true,
        text_editor::Binding::Sequence(sequence) => sequence.iter().any(changes_selection),
        _ => false,
    }
}

/// Esc：先关菜单，再取消选区；都没有就不捕获，让宿主关窗口。
fn escape(state: &mut TextPreviewViewerState) -> EventOutcome {
    if state.context_menu.take().is_some() {
        return EventOutcome::captured();
    }
    let has_selection = state
        .selection_anchor
        .zip(state.cursor)
        .is_some_and(|(anchor, cursor)| anchor != cursor);
    if has_selection {
        state.selection_anchor = state.cursor;
        return EventOutcome::captured();
    }
    EventOutcome::default()
}

/// 拖选：指针越过文字区上下边缘时开始自动滚动，光标落在边缘处那一行。
fn drag_selection(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    relative: iced::Vector,
    text_bounds: Rectangle,
) -> EventOutcome {
    if state.drag_press.is_none() {
        return EventOutcome::default();
    }
    let below = relative.y - (text_bounds.height - 1.0);
    state.autoscroll_velocity = if relative.y < 0.0 {
        (relative.y * AUTOSCROLL_GAIN).max(-AUTOSCROLL_MAX_SPEED)
    } else if below > 0.0 {
        (below * AUTOSCROLL_GAIN).min(AUTOSCROLL_MAX_SPEED)
    } else {
        0.0
    };
    state.drag_pointer = Point::new(
        relative.x.clamp(0.0, text_bounds.width) + state.scroll_x,
        relative.y.clamp(0.0, (text_bounds.height - 1.0).max(0.0)),
    );
    let moved = selection::drag(state, document, state.drag_pointer);
    if state.autoscroll_velocity != 0.0 && state.drag_active {
        state.autoscroll_tick = None;
        return EventOutcome::captured();
    }
    if moved {
        EventOutcome::captured()
    } else {
        EventOutcome::default()
    }
}

/// 每帧：自动滚动推进一步、提示到点消失。
fn tick(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    now: Instant,
) -> EventOutcome {
    let mut outcome = EventOutcome::default();
    if let Some(notice) = &state.notice {
        if now >= notice.until {
            state.notice = None;
            outcome.redraw = Some(window::RedrawRequest::NextFrame);
        } else {
            outcome.redraw = Some(window::RedrawRequest::At(notice.until));
        }
    }
    if state.autoscroll_velocity == 0.0 || !state.drag_active {
        return outcome;
    }
    let step = state
        .autoscroll_tick
        .map_or(Duration::from_millis(16), |previous| {
            now.saturating_duration_since(previous)
        })
        .min(AUTOSCROLL_MAX_STEP);
    state.autoscroll_tick = Some(now);
    autoscroll_step(state, document, step);
    EventOutcome::redraw()
}

/// 按当前速度滚一步，再把光标放到边缘处的那一行。
pub(super) fn autoscroll_step(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    step: Duration,
) {
    if scroll_viewer_by(
        state,
        document,
        state.autoscroll_velocity * step.as_secs_f32(),
    ) {
        refresh_retained_content(state, document);
    }
    if let Some(position) = selection::hit_test(state, document, state.drag_pointer) {
        state.cursor = Some(position);
    }
}

fn open_context_menu(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    position_in_text: Point,
    cursor: mouse::Cursor,
    bounds: Rectangle,
) {
    let point = Point::new(position_in_text.x + state.scroll_x, position_in_text.y);
    // 点在选区外：光标移到点击处（清掉选区），菜单里「复制」随之置灰。
    if let Some(position) = selection::hit_test(state, document, point) {
        if !selection::selection_contains(state, position) {
            state.cursor = Some(position);
            state.selection_anchor = Some(position);
        }
    }
    state.is_focused = true;
    let origin = cursor.position().map_or(Point::ORIGIN, |pointer| {
        Point::new(pointer.x - bounds.x, pointer.y - bounds.y)
    });
    state.context_menu = Some(ContextMenu {
        origin,
        hovered: None,
    });
}

fn handle_menu_event(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
    clipboard: &mut dyn Clipboard,
) -> Option<EventOutcome> {
    let menu = state.context_menu?;
    let menu_bounds = menu_bounds(menu, bounds);
    let item_under_pointer = cursor
        .position()
        .and_then(|position| item_at(menu_bounds, position));
    match event {
        Event::Mouse(mouse::Event::CursorMoved { .. }) => {
            let hovered = item_under_pointer.filter(|item| item_enabled(state, *item));
            if hovered != menu.hovered {
                state.context_menu = Some(ContextMenu { hovered, ..menu });
                return Some(EventOutcome::redraw());
            }
            Some(EventOutcome::default())
        }
        Event::Mouse(mouse::Event::ButtonPressed(_)) => {
            state.context_menu = None;
            match item_under_pointer {
                Some(item) => {
                    run_menu_item(state, document, item, clipboard);
                    Some(EventOutcome::captured())
                }
                // 点在菜单里的空白处：只关菜单；点在菜单外：关菜单，这次按下照常处理。
                None if cursor.is_over(menu_bounds) => Some(EventOutcome::captured()),
                None => None,
            }
        }
        Event::Mouse(mouse::Event::WheelScrolled { .. }) => {
            state.context_menu = None;
            None
        }
        _ => None,
    }
}

fn run_menu_item(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    item: MenuItem,
    clipboard: &mut dyn Clipboard,
) {
    if !item_enabled(state, item) {
        return;
    }
    let binding = match item {
        MenuItem::Copy => text_editor::Binding::<()>::Copy,
        MenuItem::SelectAll => text_editor::Binding::SelectAll,
    };
    apply_key_binding(binding, state, document, clipboard);
    if item == MenuItem::SelectAll {
        selection::publish_primary(state, document, clipboard);
    }
}

fn item_enabled(state: &TextPreviewViewerState, item: MenuItem) -> bool {
    match item {
        MenuItem::Copy => state
            .selection_anchor
            .zip(state.cursor)
            .is_some_and(|(anchor, cursor)| anchor != cursor),
        MenuItem::SelectAll => true,
    }
}

/// 菜单矩形：左上角在指针处，放不下时往左/往上挪，始终在查看器范围内。
fn menu_bounds(menu: ContextMenu, bounds: Rectangle) -> Rectangle {
    let size = Size::new(
        MENU_WIDTH,
        MENU_ITEMS.len() as f32 * MENU_ITEM_HEIGHT + MENU_PADDING * 2.0,
    );
    let x = (bounds.x + menu.origin.x)
        .min(bounds.x + bounds.width - size.width)
        .max(bounds.x);
    let y = (bounds.y + menu.origin.y)
        .min(bounds.y + bounds.height - size.height)
        .max(bounds.y);
    Rectangle::new(Point::new(x, y), size)
}

fn item_bounds(menu_bounds: Rectangle, index: usize) -> Rectangle {
    Rectangle {
        x: menu_bounds.x + MENU_PADDING,
        y: menu_bounds.y + MENU_PADDING + index as f32 * MENU_ITEM_HEIGHT,
        width: menu_bounds.width - MENU_PADDING * 2.0,
        height: MENU_ITEM_HEIGHT,
    }
}

fn item_at(menu_bounds: Rectangle, position: Point) -> Option<MenuItem> {
    MENU_ITEMS
        .into_iter()
        .enumerate()
        .find(|(index, _)| item_bounds(menu_bounds, *index).contains(position))
        .map(|(_, item)| item)
}

fn item_label(item: MenuItem) -> String {
    match item {
        MenuItem::Copy => tr!("Copy"),
        MenuItem::SelectAll => tr!("Select All"),
    }
}

/// 复制超限等短暂提示：在查看器底部居中显示一会儿。
pub(super) fn show_notice(state: &mut TextPreviewViewerState, text: String) {
    state.notice = Some(Notice {
        text,
        until: Instant::now() + NOTICE_DURATION,
    });
}

/// 菜单和提示画在新的一层，盖住正文与滑块。
pub(super) fn draw_overlays(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    state: &TextPreviewViewerState,
    bounds: Rectangle,
    theme: &Theme,
) {
    if state.context_menu.is_none() && state.notice.is_none() {
        return;
    }
    renderer.with_layer(bounds, |renderer| {
        if let Some(menu) = state.context_menu {
            draw_menu(renderer, state, menu, bounds, theme);
        }
        if let Some(notice) = &state.notice {
            draw_notice(renderer, notice, bounds, theme);
        }
    });
}

fn draw_menu(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    state: &TextPreviewViewerState,
    menu: ContextMenu,
    bounds: Rectangle,
    theme: &Theme,
) {
    let menu_bounds = menu_bounds(menu, bounds);
    let style = context_menu_style(theme);
    renderer.fill_quad(
        renderer::Quad {
            bounds: menu_bounds,
            border: style.border,
            shadow: style.shadow,
            ..renderer::Quad::default()
        },
        style
            .background
            .unwrap_or(Background::Color(ui_colors(theme).surface_container_low)),
    );
    let hover = context_menu_item_button_style()(theme, button::Status::Hovered);
    for (index, item) in MENU_ITEMS.into_iter().enumerate() {
        let item_bounds = item_bounds(menu_bounds, index);
        if menu.hovered == Some(item) {
            if let Some(background) = hover.background {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: item_bounds,
                        border: Border {
                            radius: 6.0.into(),
                            ..Border::default()
                        },
                        ..renderer::Quad::default()
                    },
                    background,
                );
            }
        }
        let color = if item_enabled(state, item) {
            ui_colors(theme).on_surface
        } else {
            muted_text_color(theme)
        };
        fill_label(
            renderer,
            item_label(item),
            Rectangle {
                x: item_bounds.x + 10.0,
                width: item_bounds.width - 20.0,
                ..item_bounds
            },
            color,
            alignment::Horizontal::Left,
        );
    }
}

fn draw_notice(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    notice: &Notice,
    bounds: Rectangle,
    theme: &Theme,
) {
    let width = (notice.text.chars().count() as f32 * BODY_SIZE + 32.0).min(bounds.width - 16.0);
    let notice_bounds = Rectangle {
        x: bounds.x + (bounds.width - width) / 2.0,
        y: bounds.y + bounds.height - 56.0,
        width,
        height: 36.0,
    };
    let style = context_menu_style(theme);
    renderer.fill_quad(
        renderer::Quad {
            bounds: notice_bounds,
            border: style.border,
            shadow: style.shadow,
            ..renderer::Quad::default()
        },
        Background::Color(ui_colors(theme).surface_container_highest),
    );
    fill_label(
        renderer,
        notice.text.clone(),
        notice_bounds,
        ui_colors(theme).on_surface,
        alignment::Horizontal::Center,
    );
}

fn fill_label(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    label: String,
    bounds: Rectangle,
    color: iced::Color,
    align: alignment::Horizontal,
) {
    let position = match align {
        alignment::Horizontal::Center => Point::new(bounds.center_x(), bounds.center_y()),
        _ => Point::new(bounds.x, bounds.center_y()),
    };
    renderer.fill_text(
        text::Text {
            content: label,
            bounds: bounds.size(),
            size: Pixels(BODY_SIZE),
            line_height: text::LineHeight::default(),
            font: Font::DEFAULT,
            align_x: match align {
                alignment::Horizontal::Center => text::Alignment::Center,
                _ => text::Alignment::Left,
            },
            align_y: alignment::Vertical::Center,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        },
        position,
        color,
        bounds,
    );
}

#[cfg(test)]
pub(super) fn menu_item_center_for_tests(
    state: &TextPreviewViewerState,
    bounds: Rectangle,
    select_all: bool,
) -> Option<Point> {
    let menu = state.context_menu?;
    let index = usize::from(select_all);
    Some(item_bounds(menu_bounds(menu, bounds), index).center())
}
