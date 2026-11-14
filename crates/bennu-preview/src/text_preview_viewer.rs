//! 文本查看器：显示文档窗口里的显示行，自己就是滚动区。
//!
//! 位置 = 视口顶部那一行的起点（字节偏移）+ 该行内已滚过的像素。滚轮的平滑滚动直接
//! `scroll_by` 到这里（相对量，精度与文件大小无关）；滑块按字节比例画、拖动由这里处理；
//! 可见范围变了就告诉引擎（引擎据此往前往后补读）。字号与是否换行来自视图设置。

use std::collections::HashMap;
use std::time::Instant;

use iced::advanced::mouse::{click, Click};
use iced::advanced::text::{Paragraph, Renderer as TextRenderer};
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::advanced::{graphics, layout, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::keyboard;
use iced::{
    alignment, mouse, Element, Event, Font, Length, Padding, Point, Rectangle, Size, Theme, Vector,
};

mod geometry;
mod interaction;
mod render;
mod scrollbar;
mod selection;
mod style;

use crate::text_document::{
    JumpPlace, TextPosition, TextPreviewDocument, TextViewerEvent, TextViewport,
};
use crate::text_highlight::HighlightPalette;
use crate::text_view::{TextViewChange, TextViewSettings};
use geometry::{
    anchor_index, is_at_bottom, normalize_anchor, note_measured_height, row_height,
    scroll_viewer_by, scroll_viewer_horizontally, visible_rows, window_span,
};
use render::{
    draw_empty_placeholder, draw_selection_ranges, draw_visible_rows, gutter_width,
    line_number_bounds, line_number_digit_count, line_number_paragraph, text_bounds,
    text_line_paragraph,
};
use style::{
    current_line_number_color, gutter_background_color, line_number_color, viewer_background_color,
};

const TEXT_PREVIEW_VIEWER_PADDING: f32 = 8.0;
const TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_PREVIEW_GUTTER_MIN_DIGITS: usize = 3;
const TEXT_PREVIEW_GUTTER_SPACING: f32 = 4.0;
pub(crate) const TEXT_PREVIEW_DRAG_THRESHOLD_PX: f32 = 3.0;

type PreviewParagraph = graphics::text::Paragraph;

pub fn text_preview_viewer<'a, Message, Renderer>(
    document: &'a TextPreviewDocument,
    view: TextViewSettings,
    palette: HighlightPalette,
    id: widget::Id,
    on_wheel: impl Fn(mouse::ScrollDelta) -> Message + 'a,
    on_event: impl Fn(TextViewerEvent) -> Message + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a + 'static,
    // 渲染器泛型（而非写死 iced::Renderer）：本 crate 不带渲染器
    // feature，无渲染器时 iced::Renderer 是 () 且不实现 advanced
    // Renderer trait；宿主传入的 wgpu/tiny-skia 渲染器的 Paragraph
    // 关联类型均为 iced_graphics 的具体段落类型，与两宿主兼容。
    Renderer: TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
{
    Element::new(TextPreviewViewer {
        id,
        document,
        view,
        palette,
        on_wheel: Box::new(on_wheel),
        on_event: Box::new(on_event),
    })
}

struct TextPreviewViewer<'a, Message> {
    id: widget::Id,
    document: &'a TextPreviewDocument,
    view: TextViewSettings,
    palette: HighlightPalette,
    on_wheel: Box<dyn Fn(mouse::ScrollDelta) -> Message + 'a>,
    on_event: Box<dyn Fn(TextViewerEvent) -> Message + 'a>,
}

#[derive(Default)]
struct TextPreviewViewerState {
    document_id: Option<u64>,
    revision: Option<u64>,
    jump_revision: Option<u64>,
    view: Option<TextViewSettings>,
    /// 段落按这套配色上的色；应用切换明暗后全部重排。
    palette: HighlightPalette,
    /// 当前字号下一行（不换行时）的高度；未排版的行按它估算。
    line_height: f32,
    digit_count: usize,
    gutter_width: f32,
    content_width: f32,
    // 文字视口高度 = 布局高度 − 上下内边距；只在布局时换算一次。
    viewport_height: f32,
    /// 视口顶部那一行的起点。
    anchor: u64,
    /// 该行内已经滚过的像素。
    pixel: f32,
    /// 不换行时横向滚过的像素。
    scroll_x: f32,
    /// 不换行时已排版行的最大宽度（只增不减），决定横向能滚多远。
    max_row_width: f32,
    /// 已排版行的真实高度（换行后多行），按行起点记，只增不减。
    heights: HashMap<u64, f32>,
    retained_rows: Vec<RetainedRow>,
    retained_text_width: f32,
    retained_numbers: Vec<RetainedNumber>,
    // 光标 = 选择焦点；selection_anchor 与 cursor 相异时存在选区。
    cursor: Option<TextPosition>,
    selection_anchor: Option<TextPosition>,
    last_click: Option<Click>,
    drag_click: Option<click::Kind>,
    // 拖动选择阈值：按下后移动超过阈值才算拖动，防止点击微动带出选区；
    // 一旦激活持续生效直到释放，避免阈值边缘抖动。
    drag_press: Option<Point>,
    drag_active: bool,
    is_focused: bool,
    modifiers: keyboard::Modifiers,
    /// 拖选时指针越过上下边缘的自动滚动速度（像素/秒，0 = 不滚）与上一帧时刻。
    autoscroll_velocity: f32,
    autoscroll_tick: Option<Instant>,
    /// 拖选时指针夹到文字区内的位置（段落坐标），自动滚动每帧按它重新定位光标。
    drag_pointer: Point,
    context_menu: Option<interaction::ContextMenu>,
    notice: Option<interaction::Notice>,
    thumb_drag: Option<scrollbar::ThumbDrag>,
    /// 拖滑块时滑块跟手画在这个比例上，直到落点的内容到了。
    drag_fraction: Option<f32>,
    reported_viewport: Option<TextViewport>,
    pending_events: Vec<TextViewerEvent>,
}

impl widget::operation::Focusable for TextPreviewViewerState {
    fn is_focused(&self) -> bool {
        self.is_focused
    }

    fn focus(&mut self) {
        self.is_focused = true;
    }

    fn unfocus(&mut self) {
        self.is_focused = false;
        self.drag_click = None;
    }
}

impl TextPreviewViewerState {
    fn view(&self) -> TextViewSettings {
        self.view.unwrap_or_default()
    }

    /// 正文可用宽度（行号栏右边、去掉左右内边距）。
    fn text_width(&self) -> f32 {
        (self.content_width - Padding::new(TEXT_PREVIEW_VIEWER_PADDING).x()).max(0.0)
    }

    fn max_scroll_x(&self) -> f32 {
        if self.view().wrap {
            0.0
        } else {
            (self.max_row_width - self.text_width()).max(0.0)
        }
    }
}

/// 段落按行的字节范围缓存；可见行集合变化时只排版新进入的行。范围也作为身份：文件
/// 追加后重读的最后一行起点不变、终点变了，必须重新排版。
struct RetainedRow {
    start: u64,
    end: u64,
    /// 排版时已经有高亮记号（记号后到时要重排上色）。
    colored: bool,
    paragraph: PreviewParagraph,
}

struct RetainedNumber {
    start: u64,
    line_number: u64,
    paragraph: PreviewParagraph,
}

/// 视口里的一行：窗口下标、相对文字视口顶部的 y、高度。
#[derive(Debug, Clone, Copy, PartialEq)]
struct VisibleRow {
    index: usize,
    visible_y: f32,
    height: f32,
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for TextPreviewViewer<'_, Message>
where
    Message: 'static,
    Renderer: TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<TextPreviewViewerState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(TextPreviewViewerState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let state = tree.state.downcast_mut::<TextPreviewViewerState>();
        if state.palette != self.palette {
            state.palette = self.palette;
            state.retained_rows.clear();
        }
        sync_with_document(state, self.document, self.view, size);
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<TextPreviewViewerState>();
        let outcome = interaction::handle_event(
            state,
            self.document,
            event,
            layout.bounds(),
            cursor,
            clipboard,
        );
        if outcome.captured {
            shell.capture_event();
        }
        if let Some(redraw) = outcome.redraw {
            shell.request_redraw_at(redraw);
        }
        if let Some(delta) = outcome.wheel {
            shell.publish((self.on_wheel)(delta));
        }
        queue_viewport_report(state, self.document);
        for event in state.pending_events.drain(..) {
            shell.publish((self.on_event)(event));
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        let geometry = scrollbar::virtual_geometry(state, self.document, bounds.height);
        // 横向同竖向：报给细线滑块的内容宽 = 视口宽 + 能滚的距离，两边最大偏移相等。
        let content_bounds = Rectangle {
            width: bounds.width + state.max_scroll_x(),
            height: geometry.content_height,
            ..bounds
        };
        // 独立预览窗口打开时宿主对这个 id 发 focus 操作：键盘一开始就归查看器。
        operation.focusable(Some(&self.id), bounds, state);
        operation.scrollable(
            Some(&self.id),
            bounds,
            content_bounds,
            Vector::new(state.scroll_x, geometry.offset),
            &mut ScrollBridge {
                state,
                document: self.document,
            },
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        if scrollbar::is_over_track(state, self.document, bounds, cursor) {
            mouse::Interaction::default()
        } else if cursor.is_over(text_bounds(bounds, state.gutter_width)) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        let gutter_bounds = Rectangle {
            width: state.gutter_width,
            ..bounds
        };
        let text_bounds = text_bounds(bounds, state.gutter_width);

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            viewer_background_color(theme),
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: gutter_bounds,
                ..renderer::Quad::default()
            },
            gutter_background_color(theme),
        );

        if self.document.is_empty() {
            draw_empty_placeholder(renderer, text_bounds, state.view(), theme);
            return;
        }
        let visible = visible_rows(state, self.document);
        draw_selection_ranges(renderer, state, self.document, &visible, text_bounds, theme);
        draw_visible_rows(
            renderer,
            state,
            self.document,
            &visible,
            text_bounds,
            theme,
            viewport,
        );

        // 浮层（右键菜单、提示）在新的一层，先排进去也盖在后面画的行号之上。
        interaction::draw_overlays(renderer, state, bounds, theme);
        let Some(clip_bounds) = gutter_bounds.intersection(viewport) else {
            return;
        };
        let cursor_line_start = state.cursor.and_then(|cursor| {
            let index = self.document.row_index(cursor.row).ok()?;
            Some(self.document.rows()[index].line_start)
        });
        for entry in &visible {
            let row = &self.document.rows()[entry.index];
            let Some(number) = state
                .retained_numbers
                .iter()
                .find(|number| number.start == row.start)
            else {
                continue;
            };
            let line_bounds =
                line_number_bounds(gutter_bounds, text_bounds.y + entry.visible_y, state.view());
            let position = line_bounds.anchor(
                number.paragraph.min_bounds(),
                alignment::Horizontal::Right,
                alignment::Vertical::Top,
            );
            // 光标所在行的行号用正文色，其余淡一级。
            let color = if cursor_line_start == Some(row.line_start) {
                current_line_number_color(theme)
            } else {
                line_number_color(theme)
            };
            renderer.fill_paragraph(&number.paragraph, position, color, clip_bounds);
        }
    }
}

/// Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 缩放，Alt+Z 切换换行。
fn view_shortcut(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<TextViewChange> {
    let keyboard::Key::Character(character) = key else {
        return None;
    };
    match character.as_str() {
        "=" | "+" if modifiers.command() => Some(TextViewChange::ZoomIn),
        "-" | "_" if modifiers.command() => Some(TextViewChange::ZoomOut),
        "0" if modifiers.command() => Some(TextViewChange::ZoomReset),
        "z" | "Z" if modifiers.alt() && !modifiers.command() => Some(TextViewChange::ToggleWrap),
        _ => None,
    }
}

/// 与文档、视图设置对齐：换了文档全部重置；字号或换行变了重新排版并保持看的那一行；
/// 窗口内容变了刷新可见段落；文档发来新的跳转就照做。
fn sync_with_document(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    view: TextViewSettings,
    layout: Size,
) {
    if state.document_id != Some(document.id()) {
        *state = TextPreviewViewerState {
            document_id: Some(document.id()),
            anchor: document.rows().front().map_or(0, |row| row.start),
            modifiers: state.modifiers,
            // 文件改写重开、改编码换了文档：键盘焦点留在查看器上。
            is_focused: state.is_focused,
            ..TextPreviewViewerState::default()
        };
    }
    let view_changed = state.view != Some(view);
    if view_changed {
        let previous_line_height = state.line_height;
        state.view = Some(view);
        state.line_height = view.line_height();
        // 锚点行不变，行内像素按字号比例换算：缩放后看的还是那一行。
        if previous_line_height > 0.0 {
            state.pixel *= state.line_height / previous_line_height;
        }
        state.heights.clear();
        state.retained_rows.clear();
        state.retained_numbers.clear();
        state.max_row_width = 0.0;
        state.scroll_x = 0.0;
    }
    let digit_count = line_number_digit_count(document);
    if view_changed || state.digit_count != digit_count {
        state.digit_count = digit_count;
        state.gutter_width = gutter_width(digit_count, view);
        state.retained_numbers.clear();
    }
    let content_width = (layout.width - state.gutter_width - TEXT_PREVIEW_GUTTER_SPACING).max(0.0);
    let viewport_height = (layout.height - Padding::new(TEXT_PREVIEW_VIEWER_PADDING).y()).max(0.0);
    let layout_changed = (state.content_width - content_width).abs() >= f32::EPSILON
        || (state.viewport_height - viewport_height).abs() >= f32::EPSILON;
    let content_changed = state.revision != Some(document.revision());
    state.content_width = content_width;
    state.viewport_height = viewport_height;
    state.revision = Some(document.revision());
    if content_changed {
        // 丢掉已经不在窗口里的行的高度。
        let (first, last) = window_span(document);
        state
            .heights
            .retain(|start, _| (first..last).contains(start));
    }
    if let Some(jump) = document.jump() {
        if state.jump_revision != Some(jump.revision) {
            state.jump_revision = Some(jump.revision);
            state.drag_fraction = None;
            state.anchor = jump.row;
            state.pixel = 0.0;
            refresh_retained_content(state, document);
            if jump.place == JumpPlace::Bottom {
                scroll_viewer_by(state, document, f32::MAX / 4.0);
            }
        }
    }
    if content_changed || layout_changed || view_changed {
        normalize_anchor(state, document);
        refresh_retained_content(state, document);
        // 量出来的行高可能让当前位置越过窗口底部，重新夹一次。
        scroll_viewer_by(state, document, 0.0);
    }
}

/// 可见范围或光标变了就排队一条报告（在 update 里发出）。
fn queue_viewport_report(state: &mut TextPreviewViewerState, document: &TextPreviewDocument) {
    let rows = document.rows();
    let visible = visible_rows(state, document);
    let (Some(first), Some(last)) = (visible.first(), visible.last()) else {
        return;
    };
    let viewport = TextViewport {
        first_row: rows[first.index].start,
        last_row: rows[last.index].start,
        at_bottom: is_at_bottom(state, document),
        cursor: state.cursor,
    };
    if state.reported_viewport != Some(viewport) {
        state.reported_viewport = Some(viewport);
        state
            .pending_events
            .push(TextViewerEvent::ViewportChanged(viewport));
    }
}

struct ScrollBridge<'a> {
    state: &'a mut TextPreviewViewerState,
    document: &'a TextPreviewDocument,
}

impl widget::operation::Scrollable for ScrollBridge<'_> {
    fn snap_to(&mut self, _offset: RelativeOffset<Option<f32>>) {}

    fn scroll_to(&mut self, _offset: AbsoluteOffset<Option<f32>>) {}

    fn scroll_by(
        &mut self,
        offset: AbsoluteOffset,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
    ) {
        scroll_viewer_horizontally(self.state, offset.x);
        if scroll_viewer_by(self.state, self.document, offset.y) {
            self.state.drag_fraction = None;
            refresh_retained_content(self.state, self.document);
        }
    }
}

/// 重建可见行的段落与行号缓存：仍在视口里的复用，只排版新进入的行；排版出的真实高度
/// 回填行高（只增不减），不换行时记下最宽行。
fn refresh_retained_content(state: &mut TextPreviewViewerState, document: &TextPreviewDocument) {
    let text_width = state.text_width();
    // 宽度还没建立（第一次布局之前）时排版出的高度是错的，不能写进行高表。
    if text_width <= 0.0 {
        return;
    }
    let view = state.view();
    let width_changed = (state.retained_text_width - text_width).abs() >= f32::EPSILON;
    let mut previous = std::mem::take(&mut state.retained_rows);
    let mut retained = Vec::new();
    let rows = document.rows();
    for entry in visible_rows(state, document) {
        let row = &rows[entry.index];
        let tokens = document.row_tokens(row.start);
        let reused = (!(width_changed && view.wrap))
            .then(|| {
                previous.iter().position(|retained| {
                    retained.start == row.start
                        && retained.end == row.end
                        && retained.colored == tokens.is_some()
                })
            })
            .flatten()
            .map(|position| previous.swap_remove(position));
        let paragraph = match reused {
            Some(reused) => reused.paragraph,
            None => {
                let paragraph = text_line_paragraph(
                    &row.text,
                    text_width,
                    view,
                    tokens.map(|tokens| &tokens[..]),
                    state.palette,
                );
                let bounds = paragraph.min_bounds();
                note_measured_height(state, row.start, bounds.height);
                state.max_row_width = state.max_row_width.max(bounds.width);
                paragraph
            }
        };
        retained.push(RetainedRow {
            start: row.start,
            end: row.end,
            colored: tokens.is_some(),
            paragraph,
        });
    }
    state.retained_rows = retained;
    state.retained_text_width = text_width;
    state.scroll_x = state.scroll_x.min(state.max_scroll_x());

    let gutter_width = state.gutter_width;
    let mut previous = std::mem::take(&mut state.retained_numbers);
    let mut numbers = Vec::new();
    for entry in visible_rows(state, document) {
        let row = &rows[entry.index];
        // 行号只在原始行的第一段；还没统计到的行先空着。
        let Some(line_number) = row.line_number.filter(|_| row.is_line_start()) else {
            continue;
        };
        let reused = previous
            .iter()
            .position(|retained| retained.start == row.start && retained.line_number == line_number)
            .map(|position| previous.swap_remove(position));
        numbers.push(reused.unwrap_or_else(|| RetainedNumber {
            start: row.start,
            line_number,
            paragraph: line_number_paragraph(line_number + 1, gutter_width, view),
        }));
    }
    state.retained_numbers = numbers;
}

#[cfg(test)]
mod tests;
