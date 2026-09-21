use std::path::PathBuf;

use iced::advanced::mouse::{click, Click};
use iced::advanced::text::{Paragraph, Renderer as TextRenderer};
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::advanced::Renderer as _;
use iced::advanced::{graphics, layout, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::keyboard;
use iced::widget::text_editor;
use iced::{
    alignment, mouse, Element, Event, Length, Padding, Point, Rectangle, Size, Theme, Vector,
};

mod render;
mod selection;
mod style;

use crate::text_preview::{TextPreviewDocument, TEXT_PREVIEW_LINE_HEIGHT, TEXT_PREVIEW_TEXT_SIZE};
use render::{
    draw_empty_placeholder, draw_selection_ranges, draw_visible_text_lines, line_number_bounds,
    line_number_paragraph, text_bounds, text_line_paragraph, text_preview_line_number_gutter_width,
};
use selection::{apply_key_binding, TextPos};
use style::{divider_color, placeholder_color, viewer_background_color};

const TEXT_PREVIEW_VIEWER_PADDING: f32 = 8.0;
const TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING: f32 = 6.0;
const TEXT_PREVIEW_GUTTER_DIGIT_WIDTH: f32 = 10.0;
const TEXT_PREVIEW_GUTTER_MIN_WIDTH: f32 = 30.0;
const TEXT_PREVIEW_GUTTER_SPACING: f32 = 8.0;
const TEXT_PREVIEW_DIVIDER_WIDTH: f32 = 1.0;
pub(crate) const TEXT_PREVIEW_DRAG_THRESHOLD_PX: f32 = 3.0;
pub(crate) const TEXT_PREVIEW_VIEWER_ID: &str = "text-preview-viewer";

type PreviewParagraph = graphics::text::Paragraph;

pub(crate) fn text_preview_viewer<'a, Message>(
    document: &'a TextPreviewDocument,
    scroll_height: f32,
    on_scroll: impl Fn(i32, f32, f32) -> Message + 'a,
    on_wheel: impl Fn(mouse::ScrollDelta) -> Message + 'a,
    on_content_height: impl Fn(f32) -> Message + 'a,
    on_anchor_moved: impl Fn(i32, f32, f32) -> Message + 'a,
) -> Element<'a, Message>
where
    Message: 'a + 'static,
{
    Element::new(TextPreviewViewer {
        id: widget::Id::new(TEXT_PREVIEW_VIEWER_ID),
        document,
        scroll_height,
        on_scroll: Box::new(on_scroll),
        on_wheel: Box::new(on_wheel),
        on_content_height: Box::new(on_content_height),
        on_anchor_moved: Box::new(on_anchor_moved),
    })
}

struct TextPreviewViewer<'a, Message> {
    id: widget::Id,
    document: &'a TextPreviewDocument,
    scroll_height: f32,
    on_scroll: Box<dyn Fn(i32, f32, f32) -> Message + 'a>,
    on_wheel: Box<dyn Fn(mouse::ScrollDelta) -> Message + 'a>,
    on_content_height: Box<dyn Fn(f32) -> Message + 'a>,
    on_anchor_moved: Box<dyn Fn(i32, f32, f32) -> Message + 'a>,
}

/// 滚动几何完全由查看器自管：
/// 锚定逻辑行 + 行内像素偏移（与 cosmic_text 的滚动模型一致），
/// 行高由本层量测缓存（只增不减）。文本行数据直接来自 document，
/// 不持有全文编辑器副本；光标/选择见 selection 子模块。
#[derive(Default)]
struct TextPreviewViewerState {
    source_path: Option<PathBuf>,
    generation: Option<u64>,
    content_revision: Option<u64>,
    content_width: f32,
    scroll_height: f32,
    scroll_line: usize,
    scroll_pixel: f32,
    // 锚定行上方的行高总和（增量维护，避免逐帧前缀遍历）。
    anchor_prefix: f32,
    line_heights: Vec<Option<f32>>,
    pending_scroll_report: Option<(i32, f32, f32)>,
    cached_content_height: f32,
    reported_content_height: Option<f32>,
    // 光标 = 选择焦点；anchor 与 cursor 相异时存在选区。
    cursor: Option<TextPos>,
    anchor: Option<TextPos>,
    last_click: Option<Click>,
    drag_click: Option<click::Kind>,
    // 拖动选择阈值：按下后移动超过阈值才算拖动，防止点击微动带出选区；
    // 一旦激活持续生效直到释放，避免阈值边缘抖动。
    drag_press: Option<Point>,
    drag_active: bool,
    is_focused: bool,
    retained_text_lines: Vec<TextPreviewVisibleTextLine>,
    retained_text_line_width: f32,
    retained_line_numbers: Vec<TextPreviewLineNumber>,
    retained_line_number_digit_count: usize,
    retained_line_number_gutter_width: f32,
}

/// 基准行高：与编辑器 Relative(TEXT_PREVIEW_LINE_HEIGHT) 语义一致，
/// 不再依赖编辑器 buffer metrics。
fn base_line_height() -> f32 {
    TEXT_PREVIEW_TEXT_SIZE * TEXT_PREVIEW_LINE_HEIGHT
}

/// 段落按逻辑行缓存；可见行集合不变时不重建，
/// 绘制 y 由可见窗口现算，超长换行行只 shape 一次。
struct TextPreviewVisibleTextLine {
    line_index: usize,
    height: f32,
    paragraph: PreviewParagraph,
}

struct TextPreviewLineNumber {
    line_index: usize,
    paragraph: PreviewParagraph,
}

struct VisibleLogicalLine {
    line_index: usize,
    visible_y: f32,
    height: f32,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for TextPreviewViewer<'_, Message>
where
    Message: 'static,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<TextPreviewViewerState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(TextPreviewViewerState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.scroll_height))
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits
            .width(Length::Fill)
            .height(Length::Fixed(self.scroll_height))
            .resolve(Length::Fill, Length::Fixed(self.scroll_height), Size::ZERO);
        let gutter_width = text_preview_line_number_gutter_width(self.document);
        let content_offset =
            gutter_width + TEXT_PREVIEW_GUTTER_SPACING + TEXT_PREVIEW_DIVIDER_WIDTH;
        let content_width = (size.width - content_offset).max(0.0);

        sync_with_document(
            tree.state.downcast_mut::<TextPreviewViewerState>(),
            self.document,
            content_width,
            self.scroll_height,
        );
        let _ = renderer;
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        let text_bounds = text_bounds(bounds, self.document);
        if let Some((lines, offset_y, viewport_height)) = state.pending_scroll_report.take() {
            shell.publish((self.on_scroll)(lines, offset_y, viewport_height));
        }
        if state.reported_content_height != Some(state.cached_content_height) {
            state.reported_content_height = Some(state.cached_content_height);
            shell.publish((self.on_content_height)(state.cached_content_height));
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_in(text_bounds) else {
                    if state.is_focused && !cursor.is_over(bounds) {
                        state.is_focused = false;
                        state.drag_click = None;
                    }
                    return;
                };

                let click = selection::press(state, self.document, position);
                state.last_click = Some(click);
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let moved = cursor
                    .position_in(text_bounds)
                    .is_some_and(|position| selection::drag(state, self.document, position));
                if moved {
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if selection::release(state) {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                shell.publish((self.on_wheel)(*delta));
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                text,
                ..
            }) if state.is_focused => {
                if let Some(binding) =
                    text_editor::Binding::<Message>::from_key_press(text_editor::KeyPress {
                        key: key.clone(),
                        modified_key: modified_key.clone(),
                        physical_key: physical_key.clone(),
                        modifiers: *modifiers,
                        text: text.clone(),
                        status: text_editor::Status::Focused {
                            is_hovered: cursor.is_over(bounds),
                        },
                    })
                {
                    let previous_line = state.scroll_line;
                    apply_key_binding(binding, state, self.document, clipboard);
                    refresh_retained_content(
                        state,
                        self.document,
                        text_bounds.width,
                        text_bounds.height,
                    );
                    if state.scroll_line != previous_line {
                        let lines = state.scroll_line as i32 - previous_line as i32;
                        shell.publish((self.on_anchor_moved)(
                            lines,
                            viewer_scroll_absolute(state),
                            text_bounds.height,
                        ));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let id = self.id.clone();
        let state = tree.state.downcast_mut::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        let text_width = text_bounds(bounds, self.document).width;
        let content_bounds = Rectangle {
            height: bounds.height + state.cached_content_height,
            ..bounds
        };
        let translation = Vector::new(0.0, viewer_scroll_absolute(state));

        operation.scrollable(
            Some(&id),
            bounds,
            content_bounds,
            translation,
            &mut ScrollBridge {
                state,
                document: self.document,
                text_width,
            },
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(text_bounds(layout.bounds(), self.document)) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<TextPreviewViewerState>();
        let bounds = layout.bounds();
        let gutter_width = text_preview_line_number_gutter_width(self.document);
        let gutter_bounds = Rectangle {
            width: gutter_width,
            ..bounds
        };
        let text_bounds = text_bounds(bounds, self.document);
        let divider_bounds = Rectangle {
            x: bounds.x + gutter_width + TEXT_PREVIEW_GUTTER_SPACING / 2.0,
            y: bounds.y,
            width: TEXT_PREVIEW_DIVIDER_WIDTH,
            height: bounds.height,
        };

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            viewer_background_color(theme),
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: divider_bounds,
                ..renderer::Quad::default()
            },
            divider_color(theme),
        );

        if self.document.line_count() <= 1 && self.document.content().is_empty() {
            draw_empty_placeholder(renderer, text_bounds, theme);
        } else {
            draw_selection_ranges(renderer, state, self.document, text_bounds, theme);
            draw_visible_text_lines(renderer, state, text_bounds, theme, viewport);
        }

        let Some(clip_bounds) = gutter_bounds.intersection(viewport) else {
            return;
        };
        let line_number_color = placeholder_color(theme);
        let text_origin_y = bounds.y + TEXT_PREVIEW_VIEWER_PADDING;
        for entry in visible_logical_lines(state, bounds.height - TEXT_PREVIEW_VIEWER_PADDING * 2.0)
        {
            let Some(line_number) = state
                .retained_line_numbers
                .iter()
                .find(|line_number| line_number.line_index == entry.line_index)
            else {
                continue;
            };
            let line_bounds = line_number_bounds(gutter_bounds, text_origin_y + entry.visible_y);
            let position = line_bounds.anchor(
                line_number.paragraph.min_bounds(),
                alignment::Horizontal::Right,
                alignment::Vertical::Top,
            );
            renderer.fill_paragraph(
                &line_number.paragraph,
                position,
                line_number_color,
                clip_bounds,
            );
        }
    }
}

/// 与 document 对齐：源变化全重置；分块追加只扩容行高表（前缀保留，
/// 滚动锚点与已量测行高不变）；宽度/高度变化刷新可见段落。
fn sync_with_document(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    content_width: f32,
    scroll_height: f32,
) {
    let needs_source_update = state.source_path.as_deref() != Some(document.path())
        || state.generation != Some(document.generation());
    let needs_layout_update = (state.content_width - content_width).abs() >= f32::EPSILON
        || (state.scroll_height - scroll_height).abs() >= f32::EPSILON;
    let previous_line_count = state.line_heights.len();
    let needs_line_update = state.content_revision != Some(document.content_revision())
        || previous_line_count != document.line_count();

    if !needs_source_update && !needs_layout_update && !needs_line_update {
        return;
    }

    if needs_source_update {
        state.source_path = Some(document.path().to_path_buf());
        state.generation = Some(document.generation());
        state.scroll_line = 0;
        state.scroll_pixel = 0.0;
        state.last_click = None;
        state.drag_click = None;
        state.drag_press = None;
        state.drag_active = false;
        state.is_focused = false;
        state.retained_text_lines.clear();
        state.retained_line_numbers.clear();
    }

    state.content_width = content_width;
    state.scroll_height = scroll_height;
    let padding = Padding::new(TEXT_PREVIEW_VIEWER_PADDING);
    let text_width = (content_width - padding.x()).max(0.0);
    let viewport_height = (scroll_height - padding.y()).max(0.0);

    if needs_source_update {
        let line_count = document.line_count();
        state.line_heights = vec![None; line_count];
        state.cached_content_height = line_count as f32 * base_line_height();
        state.content_revision = Some(document.content_revision());
    } else if needs_line_update {
        // 分块追加：扩展行高表，前缀与滚动几何原样保留。
        let line_count = document.line_count();
        let base = base_line_height();
        if line_count >= previous_line_count {
            state.cached_content_height += (line_count - previous_line_count) as f32 * base;
        } else {
            state.cached_content_height = line_count as f32 * base;
        }
        state.line_heights.resize(line_count, None);
        state.content_revision = Some(document.content_revision());
    }
    refresh_retained_content(state, document, text_width, viewport_height);
}

fn update_retained_text_lines_for_size(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    text_width: f32,
    viewport_height: f32,
) {
    let visible_lines = visible_logical_lines(state, viewport_height);
    let mut previous = std::mem::take(&mut state.retained_text_lines);
    let width_changed = (state.retained_text_line_width - text_width).abs() >= f32::EPSILON;

    // 行集滚动时复用仍在视口内的段落，只 shape 新进入的行，
    // 单次滚动的排版成本与进入视口的行数成正比。
    state.retained_text_lines = visible_lines
        .iter()
        .filter_map(|visible_line| {
            if !width_changed {
                if let Some(position) = previous.iter().position(|retained| {
                    retained.line_index == visible_line.line_index
                        && (retained.height - visible_line.height).abs() < f32::EPSILON
                }) {
                    return Some(previous.remove(position));
                }
            }
            let Some(text) = document.line(visible_line.line_index) else {
                return None;
            };
            let paragraph = text_line_paragraph(text, text_width, visible_line.height);
            // 段落真实换行高度回填行高表（只增不减）。
            note_measured_line_height(
                state,
                visible_line.line_index,
                paragraph.min_bounds().height,
            );
            Some(TextPreviewVisibleTextLine {
                line_index: visible_line.line_index,
                height: visible_line.height,
                paragraph,
            })
        })
        .collect();
    state.retained_text_line_width = text_width;
}

fn update_retained_line_numbers_for_size(
    state: &mut TextPreviewViewerState,
    digit_count: usize,
    gutter_width: f32,
    viewport_height: f32,
) {
    let visible_lines = visible_logical_lines(state, viewport_height);
    let mut previous = std::mem::take(&mut state.retained_line_numbers);
    let width_changed = state.retained_line_number_digit_count != digit_count
        || (state.retained_line_number_gutter_width - gutter_width).abs() >= f32::EPSILON;

    state.retained_line_numbers = visible_lines
        .iter()
        .filter_map(|visible_line| {
            if !width_changed {
                if let Some(position) = previous
                    .iter()
                    .position(|retained| retained.line_index == visible_line.line_index)
                {
                    return Some(previous.remove(position));
                }
            }
            Some(TextPreviewLineNumber {
                line_index: visible_line.line_index,
                paragraph: line_number_paragraph(
                    visible_line.line_index + 1,
                    digit_count,
                    gutter_width,
                ),
            })
        })
        .collect();
    state.retained_line_number_digit_count = digit_count;
    state.retained_line_number_gutter_width = gutter_width;
}

/// 行高：优先用本层量测缓存，未量测行按基准行高估算。
fn line_height_of(state: &TextPreviewViewerState, line_index: usize) -> f32 {
    state
        .line_heights
        .get(line_index)
        .copied()
        .flatten()
        .unwrap_or_else(base_line_height)
}

/// 量测行高只增不减：编辑器内部会裁剪不可见行的 layout，
/// 观测值缩小时保留旧值，滚动几何因此单调稳定。
fn note_measured_line_height(state: &mut TextPreviewViewerState, line_index: usize, height: f32) {
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    if state.line_heights.len() <= line_index {
        state.line_heights.resize(line_index + 1, None);
    }
    let base = base_line_height();
    let previous = state.line_heights[line_index].unwrap_or(base);
    // 量测与基准常量存在 1-2 ulp 表示差，微差视为无变化，
    // 避免行高表被抬高后边界滚动停在行缝上。
    if height <= previous + 0.01 {
        return;
    }
    state.line_heights[line_index] = Some(height);
    state.cached_content_height += height - previous;
    if line_index < state.scroll_line {
        // 锚定行上方的行变高/变矮，前缀缓存同步增量维护。
        state.anchor_prefix += height - previous;
    }
}

fn viewer_scroll_absolute(state: &TextPreviewViewerState) -> f32 {
    state.anchor_prefix + state.scroll_pixel
}

/// 从当前锚点出发按像素增量滚动，返回锚定行前进的行数。
/// 只跨过增量覆盖的行，成本与滚动距离成正比，与文件深度无关。
fn scroll_viewer_by(state: &mut TextPreviewViewerState, delta_y: f32, viewport_height: f32) -> i32 {
    let line_count = state.line_heights.len();
    if line_count == 0 {
        return 0;
    }

    let mut line_index = state.scroll_line;
    let mut pixel = state.scroll_pixel + delta_y;
    let mut prefix = state.anchor_prefix;

    // 向下跨行（pixel 达到当前行高即前进到下一行）。
    while line_index + 1 < line_count {
        let height = line_height_of(state, line_index);
        if pixel < height {
            break;
        }
        pixel -= height;
        prefix += height;
        line_index += 1;
    }
    // 最后一行的行内偏移不能超过行高。
    pixel = pixel.min(line_height_of(state, line_index));

    // 向上跨行（pixel 为负即退回上一行）。
    while pixel < 0.0 && line_index > 0 {
        line_index -= 1;
        let height = line_height_of(state, line_index);
        pixel += height;
        prefix -= height;
    }
    if pixel < 0.0 {
        pixel = 0.0;
    }

    // 底部钳制：内容剩余不足一视口时整体上移。
    // 先扣行内偏移，再逐行回退锚定行，每退一行扣减该行行高。
    let max_total = (state.cached_content_height - viewport_height).max(0.0);
    let mut amount = (prefix + pixel) - max_total;
    if amount > 0.0 {
        if pixel >= amount {
            pixel -= amount;
        } else {
            amount -= pixel;
            pixel = 0.0;
            while amount > 0.0 && line_index > 0 {
                line_index -= 1;
                let height = line_height_of(state, line_index);
                prefix -= height;
                if height > amount {
                    pixel = height - amount;
                    break;
                }
                amount -= height;
            }
        }
    }

    let applied_lines = line_index as i32 - state.scroll_line as i32;
    state.scroll_line = line_index;
    state.scroll_pixel = pixel;
    state.anchor_prefix = prefix;
    applied_lines
}

/// 从锚定行起收集可见逻辑行；行内偏移使首行可为部分行，
/// 部分行由绘制裁剪，保证逐像素滚动内容连续不跳变。
fn visible_logical_lines(
    state: &TextPreviewViewerState,
    viewport_height: f32,
) -> Vec<VisibleLogicalLine> {
    let base = base_line_height();
    let limit = viewport_height + base;
    let mut entries = Vec::new();
    let mut y = -state.scroll_pixel;
    let mut line_index = state.scroll_line;
    let line_count = state.line_heights.len();

    while line_index < line_count && y < limit {
        let height = line_height_of(state, line_index);
        if y + height > 0.0 {
            entries.push(VisibleLogicalLine {
                line_index,
                visible_y: y,
                height,
            });
        }
        y += height;
        line_index += 1;
    }

    entries
}

struct ScrollBridge<'a> {
    state: &'a mut TextPreviewViewerState,
    document: &'a TextPreviewDocument,
    text_width: f32,
}

impl widget::operation::Scrollable for ScrollBridge<'_> {
    fn snap_to(&mut self, _offset: RelativeOffset<Option<f32>>) {}

    fn scroll_to(&mut self, offset: AbsoluteOffset<Option<f32>>) {
        let Some(target_y) = offset.y else {
            return;
        };
        let viewport_height =
            (self.state.scroll_height - Padding::new(TEXT_PREVIEW_VIEWER_PADDING).y()).max(0.0);
        self.apply_pixel_delta(
            target_y - viewer_scroll_absolute(self.state),
            viewport_height,
        );
    }

    fn scroll_by(&mut self, offset: AbsoluteOffset, bounds: Rectangle, _content_bounds: Rectangle) {
        let padding = Padding::new(TEXT_PREVIEW_VIEWER_PADDING);
        let viewport_height = (bounds.height - padding.y()).max(0.0);
        self.apply_pixel_delta(offset.y, viewport_height);
    }
}
impl ScrollBridge<'_> {
    fn apply_pixel_delta(&mut self, delta_y: f32, viewport_height: f32) {
        let state = &mut *self.state;
        let previous_line = state.scroll_line;
        let applied_lines = scroll_viewer_by(state, delta_y, viewport_height);

        if state.scroll_line != previous_line {
            refresh_retained_content(state, self.document, self.text_width, viewport_height);
        }

        let total_after = viewer_scroll_absolute(state);
        let report = state
            .pending_scroll_report
            .get_or_insert((0, 0.0, viewport_height));
        report.0 += applied_lines;
        report.1 = total_after;
        report.2 = viewport_height;
    }
}

/// 用给定文本宽度重建可见内容缓存；宽度未建立（<= 0）时跳过，
/// 避免错误宽度污染行高表。
fn refresh_retained_content(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    text_width: f32,
    viewport_height: f32,
) {
    if text_width <= 0.0 {
        return;
    }
    update_retained_text_lines_for_size(state, document, text_width, viewport_height);
    update_retained_line_numbers_for_size(
        state,
        document.line_number_digit_count(),
        text_preview_line_number_gutter_width(document),
        viewport_height,
    );
}

#[cfg(test)]
mod tests;
