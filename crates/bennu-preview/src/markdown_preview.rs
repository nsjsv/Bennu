//! Markdown 渲染模式的画法：借用文档里解析好的内容（`markdown_render`），按字号缩放排版，
//! 行内格式、代码块高亮、真表格、搜索匹配的底色都用富文本段画。每块包一层带 id 的容器，
//! 搜索跳到某一块时按 id 找到它滚过去。

use std::ops::Range;

use iced::advanced::widget::operation::{self, Operation, Outcome};
use iced::advanced::widget::{self as advanced_widget};
use iced::font::{Style as FontStyle, Weight};
use iced::widget::scrollable::{AbsoluteOffset, Direction, Scrollbar};
use iced::widget::text::Span;
use iced::widget::{container, rich_text, row, rule, scrollable, span, table, Column, Space};
use iced::{Alignment, Color, Element, Font, Length, Pixels, Rectangle, Task, Vector};
use pulldown_cmark::{Alignment as ColumnAlignment, HeadingLevel};

use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};
use bennu_theme::ui_colors;

use crate::markdown_render::{
    CodeLine, InlineStyle, MarkdownBlock, MarkdownUnit, RenderedMarkdown, TextBlockKind,
};
use crate::panel_text::readable_text;
use crate::scroll_wiring::ScrollRegionWiring;
use crate::text_document::RenderedMatch;
use crate::text_highlight::{HighlightPalette, TokenKind};
use crate::text_view::TextViewSettings;
use bennu_localization::tr;

const BODY_TEXT_SIZE: f32 = 14.0;
const CODE_TEXT_SIZE: f32 = 13.0;
const BLOCK_SPACING: f32 = 10.0;
const LIST_INDENT: f32 = 20.0;
const MARKER_WIDTH: f32 = 22.0;

/// 渲染内容里的搜索匹配：按（单元, 起点）排好，`current` 是当前匹配的下标。
#[derive(Clone, Copy)]
pub struct MarkdownMarks<'a> {
    pub matches: &'a [RenderedMatch],
    pub current: Option<usize>,
}

/// 画一块用的东西。
#[derive(Clone, Copy)]
struct Look<'a> {
    zoom: f32,
    palette: HighlightPalette,
    marks: MarkdownMarks<'a>,
}

impl Look<'_> {
    fn size(&self, base: f32) -> Pixels {
        Pixels((base * self.zoom).round())
    }

    /// 这个单元里的匹配：（单元内字节范围，是不是当前匹配）。
    fn marks_in(&self, unit: usize) -> Vec<(Range<usize>, bool)> {
        let matches = self.marks.matches;
        let first = matches.partition_point(|found| found.unit < unit);
        matches[first..]
            .iter()
            .enumerate()
            .take_while(|(_, found)| found.unit == unit)
            .map(|(offset, found)| {
                (
                    found.range.clone(),
                    self.marks.current == Some(first + offset),
                )
            })
            .collect()
    }
}

pub(crate) fn markdown_block_id(block: usize) -> advanced_widget::Id {
    advanced_widget::Id::from(format!("markdown-preview-block-{block}"))
}

pub fn markdown_preview_body<'a, Message>(
    rendered: &'a RenderedMarkdown,
    view: TextViewSettings,
    palette: HighlightPalette,
    marks: MarkdownMarks<'a>,
    scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    let look = Look {
        zoom: view.zoom_percent as f32 / 100.0,
        palette,
        marks,
    };
    // 四周留白与原文模式的查看器一致，文字不贴边框。
    let mut content = Column::new()
        .spacing(BLOCK_SPACING * look.zoom)
        .padding([12, 16])
        .width(Length::Fill);
    if rendered.blocks().is_empty() {
        content = content.push(readable_text(tr!("(empty file)")).size(look.size(BODY_TEXT_SIZE)));
    }
    for (index, block) in rendered.blocks().iter().enumerate() {
        content = content.push(
            container(block_view(block, look))
                .id(markdown_block_id(index))
                .width(Length::Fill),
        );
    }
    let scroller = scroll
        .scrollable(content.into())
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .height(Length::Fill)
        .width(Length::Fill);
    enhanced_scrollbar(scroller, ScrollbarAxis::Vertical)
}

fn block_view<'a, Message: 'a>(block: &'a MarkdownBlock, look: Look<'a>) -> Element<'a, Message> {
    match block {
        MarkdownBlock::Text {
            kind,
            marker,
            content,
        } => text_block(*kind, marker.as_deref(), content, look),
        MarkdownBlock::Code {
            language,
            content,
            lines,
        } => code_block(language.as_deref(), content, lines, look),
        MarkdownBlock::Table {
            alignments,
            header,
            rows,
        } => table_block(alignments, header, rows, look),
        MarkdownBlock::Rule => rule::horizontal(1).into(),
    }
}

fn text_block<'a, Message: 'a>(
    kind: TextBlockKind,
    marker: Option<&'a str>,
    content: &'a MarkdownUnit,
    look: Look<'a>,
) -> Element<'a, Message> {
    let (size, bold) = match kind {
        TextBlockKind::Heading(level) => (heading_size(level), true),
        _ => (BODY_TEXT_SIZE, false),
    };
    let body = rich_text(unit_spans(content, bold, look))
        .size(look.size(size))
        .width(Length::Fill);
    match kind {
        TextBlockKind::Paragraph | TextBlockKind::Heading(_) => body.into(),
        TextBlockKind::ListItem { depth } => row![
            Space::new().width(Length::Fixed(depth as f32 * LIST_INDENT * look.zoom)),
            rich_text([plain_span(marker.unwrap_or("•"))])
                .size(look.size(BODY_TEXT_SIZE))
                .width(Length::Fixed(MARKER_WIDTH * look.zoom)),
            body,
        ]
        .spacing(6)
        .align_y(Alignment::Start)
        .into(),
        TextBlockKind::Quote { depth } => row![
            Space::new().width(Length::Fixed(
                depth.saturating_sub(1) as f32 * LIST_INDENT * look.zoom
            )),
            container(Space::new().width(Length::Fixed(3.0)).height(Length::Fill))
                .height(Length::Fill)
                .style(|theme| container::Style {
                    background: Some(ui_colors(theme).outline_variant.into()),
                    ..container::Style::default()
                }),
            body,
        ]
        .spacing(10)
        .height(Length::Shrink)
        .into(),
    }
}

fn heading_size(level: HeadingLevel) -> f32 {
    match level {
        HeadingLevel::H1 => 28.0,
        HeadingLevel::H2 => 24.0,
        HeadingLevel::H3 => 20.0,
        HeadingLevel::H4 => 18.0,
        HeadingLevel::H5 => 16.0,
        HeadingLevel::H6 => 14.0,
    }
}

fn code_block<'a, Message: 'a>(
    language: Option<&'a str>,
    content: &'a MarkdownUnit,
    lines: &'a [CodeLine],
    look: Look<'a>,
) -> Element<'a, Message> {
    let mut code = Column::new().spacing(4).width(Length::Fill);
    if let Some(language) = language {
        code = code.push(
            rich_text([plain_span(language).font(Font::MONOSPACE)])
                .size(look.size(CODE_TEXT_SIZE - 1.0))
                .color(Color {
                    a: 0.7,
                    ..look.palette.color(TokenKind::Comment)
                }),
        );
    }
    // 记号覆盖不到的部分用正文色；行之间补换行。
    let text = content.inline.text.as_str();
    let mut pieces: Vec<(Range<usize>, Option<Color>)> = Vec::new();
    for line in lines {
        let mut position = line.range.start;
        for token in &line.tokens {
            let start = line.range.start + token.range.start;
            let end = line.range.start + token.range.end;
            if start > position {
                pieces.push((position..start, None));
            }
            pieces.push((start..end, Some(look.palette.color(token.kind))));
            position = end;
        }
        // 行尾到下一行开头（含换行符）。
        let next = (line.range.end + 1).min(text.len());
        if next > position {
            pieces.push((position..next, None));
        }
    }
    let spans = marked_spans(
        text,
        pieces.into_iter().map(|(range, color)| {
            (
                range,
                SpanLook {
                    font: Font::MONOSPACE,
                    color,
                    background: None,
                    strike: false,
                },
            )
        }),
        &look.marks_in(content.unit),
        look.palette,
    );
    code = code.push(
        rich_text(spans)
            .size(look.size(CODE_TEXT_SIZE))
            .width(Length::Fill),
    );
    container(code)
        .padding(10)
        .width(Length::Fill)
        .style(|theme| container::Style {
            background: Some(ui_colors(theme).surface_container_low.into()),
            border: iced::Border {
                radius: 6.0.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// 真表格：列按标注对齐，表头加粗、有底色，单元格之间有分隔线；太宽时横向滚动。
fn table_block<'a, Message: 'a>(
    alignments: &'a [ColumnAlignment],
    header: &'a [MarkdownUnit],
    rows: &'a [Vec<MarkdownUnit>],
    look: Look<'a>,
) -> Element<'a, Message> {
    let columns = alignments.iter().enumerate().map(|(column, alignment)| {
        let align = match alignment {
            ColumnAlignment::Center => iced::alignment::Horizontal::Center,
            ColumnAlignment::Right => iced::alignment::Horizontal::Right,
            ColumnAlignment::Left | ColumnAlignment::None => iced::alignment::Horizontal::Left,
        };
        let head: Element<'a, Message> = match header.get(column) {
            Some(cell) => rich_text(unit_spans(cell, true, look))
                .size(look.size(BODY_TEXT_SIZE))
                .into(),
            None => Space::new().into(),
        };
        table::column(
            head,
            move |row: &'a Vec<MarkdownUnit>| -> Element<'a, Message> {
                match row.get(column) {
                    Some(cell) => rich_text(unit_spans(cell, false, look))
                        .size(look.size(BODY_TEXT_SIZE))
                        .into(),
                    None => Space::new().into(),
                }
            },
        )
        .align_x(align)
    });
    let grid = table::table(columns, rows.iter())
        .padding_x(10.0 * look.zoom)
        .padding_y(6.0 * look.zoom)
        .separator_x(1.0)
        .separator_y(1.0);
    scrollable(grid)
        .direction(Direction::Horizontal(
            Scrollbar::new().width(4).scroller_width(4),
        ))
        .style(enhanced_scrollbar_style)
        .width(Length::Fill)
        .into()
}

fn plain_span(text: &str) -> Span<'_, (), Font> {
    span(text)
}

#[derive(Clone, Copy)]
struct SpanLook {
    font: Font,
    color: Option<Color>,
    background: Option<Color>,
    strike: bool,
}

/// 一个单元的行内格式 → 段。
fn unit_spans<'a>(unit: &'a MarkdownUnit, bold: bool, look: Look<'a>) -> Vec<Span<'a, (), Font>> {
    let palette = look.palette;
    let looks = unit
        .inline
        .styled_ranges()
        .map(move |(range, style)| (range, inline_look(style, bold, palette)));
    marked_spans(&unit.inline.text, looks, &look.marks_in(unit.unit), palette)
}

fn inline_look(style: InlineStyle, bold: bool, palette: HighlightPalette) -> SpanLook {
    let font = if style.code {
        Font::MONOSPACE
    } else {
        Font {
            weight: if bold || style.bold {
                Weight::Bold
            } else {
                Weight::Normal
            },
            style: if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..Font::DEFAULT
        }
    };
    SpanLook {
        font,
        color: style.link.then(|| palette.color(TokenKind::Link)),
        background: style.code.then(|| palette.inline_code_background()),
        strike: style.strike,
    }
}

/// 按样式段切成富文本段，再在搜索匹配的边界处切开、上匹配底色。
fn marked_spans<'a>(
    text: &'a str,
    looks: impl Iterator<Item = (Range<usize>, SpanLook)>,
    marks: &[(Range<usize>, bool)],
    palette: HighlightPalette,
) -> Vec<Span<'a, (), Font>> {
    let mut spans = Vec::new();
    for (range, look) in looks {
        let mut cuts = vec![range.start, range.end];
        for (mark, _) in marks {
            for edge in [mark.start, mark.end] {
                if range.start < edge && edge < range.end {
                    cuts.push(edge);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for piece in cuts.windows(2) {
            let (start, end) = (piece[0], piece[1]);
            let marked = marks
                .iter()
                .find(|(mark, _)| mark.start <= start && end <= mark.end);
            let background = match marked {
                Some((_, true)) => Some(palette.current_match_background()),
                Some((_, false)) => Some(palette.match_background()),
                None => look.background,
            };
            let mut piece = span(&text[start..end])
                .font(look.font)
                .strikethrough(look.strike);
            if let Some(color) = look.color {
                piece = piece.color(color);
            }
            if let Some(background) = background {
                piece = piece.background(background);
            }
            spans.push(piece);
        }
    }
    spans
}

/// 把第 `block` 块滚进视口：已经看得见就不动，否则放到视口约三分之一高处。
pub(crate) fn reveal_markdown_block<T: Send + 'static>(block: usize) -> Task<T> {
    advanced_widget::operate(RevealBlock {
        target: markdown_block_id(block),
        depth: 0,
        scrollables: Vec::new(),
        found: None,
    })
}

#[derive(Clone)]
struct ScrollableSeen {
    depth: usize,
    id: Option<advanced_widget::Id>,
    bounds: Rectangle,
    content_bounds: Rectangle,
    translation: Vector,
}

/// 找到目标块与包着它的最近一层滚动区，算好落点后接一个 `scroll_to`。
struct RevealBlock {
    target: advanced_widget::Id,
    depth: usize,
    /// 当前遍历路径上的滚动区（祖先链）。
    scrollables: Vec<ScrollableSeen>,
    found: Option<(Rectangle, ScrollableSeen)>,
}

impl<T: Send + 'static> Operation<T> for RevealBlock {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.depth += 1;
        operate(self);
        self.depth -= 1;
        let depth = self.depth;
        self.scrollables.retain(|seen| seen.depth < depth);
    }

    fn container(&mut self, id: Option<&advanced_widget::Id>, bounds: Rectangle) {
        if self.found.is_none() && id == Some(&self.target) {
            if let Some(scrollable) = self.scrollables.last() {
                self.found = Some((bounds, scrollable.clone()));
            }
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&advanced_widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn operation::Scrollable,
    ) {
        self.scrollables.push(ScrollableSeen {
            depth: self.depth,
            id: id.cloned(),
            bounds,
            content_bounds,
            translation,
        });
    }

    fn finish(&self) -> Outcome<T> {
        let Some((target, scrollable)) = &self.found else {
            return Outcome::None;
        };
        let Some(id) = scrollable.id.clone() else {
            return Outcome::None;
        };
        let top = target.y - scrollable.content_bounds.y;
        let offset = scrollable.translation.y;
        let height = scrollable.bounds.height;
        if top >= offset && top + target.height.min(height) <= offset + height {
            return Outcome::None;
        }
        let y = (top - height / 3.0).max(0.0);
        Outcome::Chain(Box::new(operation::scrollable::scroll_to(
            id,
            AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        )))
    }
}

#[cfg(test)]
#[path = "markdown_preview_tests.rs"]
mod tests;
