//! 查看器绘制辅助：段落构造（字号随缩放、全角文字换无衬线字体）、行号栏宽度、选区高亮。

// 渲染器泛型与主模块同因：本 crate 不绑定具体渲染器 feature，
// 边界见主模块 text_preview_viewer 的构造函数注释。
use iced::advanced::text::{Paragraph as _, Renderer as TextRenderer, Span};
use iced::advanced::{renderer, text};
use iced::{alignment, Font, Padding, Pixels, Point, Rectangle, Size, Theme};

use super::style::{placeholder_color, selection_color, text_color};
use super::{
    selection, PreviewParagraph, TextPreviewViewerState, VisibleRow,
    TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING, TEXT_PREVIEW_GUTTER_MIN_DIGITS,
    TEXT_PREVIEW_GUTTER_SPACING, TEXT_PREVIEW_VIEWER_PADDING,
};
use crate::text_highlight::{HighlightPalette, TextToken};
use crate::text_preview::TextPreviewDocument;
use crate::text_view::{TextViewSettings, TEXT_VIEW_LINE_HEIGHT};

/// 选区高亮：段落坐标减去横向滚动，裁在文字区内。
pub(super) fn draw_selection_ranges(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    visible: &[VisibleRow],
    text_bounds: Rectangle,
    theme: &Theme,
) {
    let origin = text_bounds.position();
    for quad in selection::selection_quads(state, document, visible) {
        let bounds = Rectangle {
            x: quad.x + origin.x - state.scroll_x,
            y: quad.y + origin.y,
            ..quad
        };
        let Some(bounds) = bounds.intersection(&text_bounds) else {
            continue;
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            selection_color(theme),
        );
    }
}

pub(super) fn draw_visible_rows(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    visible: &[VisibleRow],
    text_bounds: Rectangle,
    theme: &Theme,
    viewport: &Rectangle,
) {
    let Some(clip_bounds) = text_bounds.intersection(viewport) else {
        return;
    };
    let color = text_color(theme);
    for entry in visible {
        let start = document.rows()[entry.index].start;
        let Some(row) = state.retained_rows.iter().find(|row| row.start == start) else {
            continue;
        };
        renderer.fill_paragraph(
            &row.paragraph,
            Point::new(
                text_bounds.x - state.scroll_x,
                text_bounds.y + entry.visible_y,
            ),
            color,
            clip_bounds,
        );
    }
}

/// 空文件时以占位色提示。
pub(super) fn draw_empty_placeholder(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    text_bounds: Rectangle,
    view: TextViewSettings,
    theme: &Theme,
) {
    renderer.fill_text(
        text::Text {
            content: bennu_localization::tr!("(empty file)"),
            bounds: text_bounds.size(),
            size: Pixels(view.font_size()),
            line_height: text::LineHeight::Relative(TEXT_VIEW_LINE_HEIGHT),
            font: Font::DEFAULT,
            align_x: text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::WordOrGlyph,
        },
        text_bounds.position(),
        placeholder_color(theme),
        text_bounds,
    );
}

/// 正文区：行号栏与间隔右边，四周留内边距。
pub(super) fn text_bounds(bounds: Rectangle, gutter_width: f32) -> Rectangle {
    let x = bounds.x + gutter_width + TEXT_PREVIEW_GUTTER_SPACING;
    Rectangle {
        x,
        y: bounds.y,
        width: (bounds.width - (x - bounds.x)).max(0.0),
        height: bounds.height,
    }
    .shrink(Padding::new(TEXT_PREVIEW_VIEWER_PADDING))
}

/// 行号栏要容纳的位数（统计完用总行数，统计中用估算值），至少三位，免得行数一过
/// 99 栏宽就跳。
pub(super) fn line_number_digit_count(document: &TextPreviewDocument) -> usize {
    let digits = document.largest_line_number().max(1).ilog10() as usize + 1;
    digits.max(TEXT_PREVIEW_GUTTER_MIN_DIGITS)
}

/// 行号栏宽度：按实际等宽数字的宽度量，字号缩放后跟着变。
pub(super) fn gutter_width(digit_count: usize, view: TextViewSettings) -> f32 {
    let probe = PreviewParagraph::with_text(text::Text {
        content: "0".repeat(digit_count).as_str(),
        bounds: Size::INFINITE,
        size: Pixels(view.font_size()),
        line_height: text::LineHeight::Relative(TEXT_VIEW_LINE_HEIGHT),
        font: Font::MONOSPACE,
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    });
    probe.min_bounds().width.ceil() + TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING * 2.0
}

pub(super) fn line_number_bounds(
    gutter_bounds: Rectangle,
    y: f32,
    view: TextViewSettings,
) -> Rectangle {
    Rectangle {
        x: gutter_bounds.x,
        y,
        width: gutter_bounds.width - TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING,
        height: view.line_height(),
    }
}

pub(super) fn line_number_paragraph(
    line_number: u64,
    gutter_width: f32,
    view: TextViewSettings,
) -> PreviewParagraph {
    let label = line_number.to_string();
    PreviewParagraph::with_text(text::Text {
        content: label.as_str(),
        bounds: Size::new(gutter_width, view.line_height()),
        size: Pixels(view.font_size()),
        line_height: text::LineHeight::Relative(TEXT_VIEW_LINE_HEIGHT),
        font: Font::MONOSPACE,
        align_x: text::Alignment::Right,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    })
}

/// 一行的段落。换行时宽度是文字区宽，高度不设限（换行后的真实高度要完整量出来）；
/// 不换行时宽高都不设限，量出的宽度决定横向能滚多远。有高亮记号时按记号上色。
pub(super) fn text_line_paragraph(
    content: &str,
    width: f32,
    view: TextViewSettings,
    tokens: Option<&[TextToken]>,
    palette: HighlightPalette,
) -> PreviewParagraph {
    let spans = styled_spans(content, tokens.unwrap_or_default(), palette);
    PreviewParagraph::with_spans(text::Text {
        content: spans.as_slice(),
        bounds: Size::new(if view.wrap { width } else { f32::INFINITY }, f32::INFINITY),
        size: Pixels(view.font_size()),
        line_height: text::LineHeight::Relative(TEXT_VIEW_LINE_HEIGHT),
        font: Font::MONOSPACE,
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: if view.wrap {
            text::Wrapping::WordOrGlyph
        } else {
            text::Wrapping::None
        },
    })
}

/// 按字体分段（全角文字用界面字体）再按记号切开上色；没被记号覆盖的部分用正文色。
fn styled_spans<'a>(
    content: &'a str,
    tokens: &[TextToken],
    palette: HighlightPalette,
) -> Vec<Span<'a, (), Font>> {
    let mut spans = Vec::new();
    for (run, is_wide) in font_runs(content) {
        let mut position = run.start;
        for token in tokens
            .iter()
            .filter(|token| token.range.start < run.end && token.range.end > run.start)
        {
            let start = token.range.start.max(run.start);
            let end = token.range.end.min(run.end);
            if start > position {
                spans.push(font_span(&content[position..start], is_wide));
            }
            spans.push(font_span(&content[start..end], is_wide).color(palette.color(token.kind)));
            position = end;
        }
        if position < run.end || run.is_empty() {
            spans.push(font_span(&content[position..run.end], is_wide));
        }
    }
    spans
}

/// 全角文字（汉字、假名、谚文、全角标点）用界面无衬线字体，其余等宽。等宽字体自带的
/// CJK 字形常按「两格」排，一格 0.6 em 时一个汉字占 1.2 em，字间空隙明显过大。
fn font_runs(content: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut runs = Vec::new();
    let mut run_start = 0;
    let mut run_is_wide = None;
    for (index, character) in content.char_indices() {
        let is_wide = is_wide_script(character);
        if run_is_wide.is_some_and(|wide| wide != is_wide) {
            runs.push((run_start..index, run_is_wide == Some(true)));
            run_start = index;
        }
        run_is_wide = Some(is_wide);
    }
    runs.push((run_start..content.len(), run_is_wide == Some(true)));
    runs
}

fn font_span(text: &str, is_wide: bool) -> Span<'_, (), Font> {
    Span::new(text).font(if is_wide {
        Font::DEFAULT
    } else {
        Font::MONOSPACE
    })
}

fn is_wide_script(character: char) -> bool {
    matches!(
        character,
        '\u{1100}'..='\u{11ff}'   // 谚文字母
            | '\u{2e80}'..='\u{2fdf}' // 部首
            | '\u{3000}'..='\u{30ff}' // 全角标点、假名
            | '\u{3100}'..='\u{31ff}' // 注音、谚文兼容、假名扩展
            | '\u{3400}'..='\u{4dbf}' // 汉字扩展 A
            | '\u{4e00}'..='\u{9fff}' // 汉字
            | '\u{a960}'..='\u{a97f}'
            | '\u{ac00}'..='\u{d7ff}' // 谚文音节
            | '\u{f900}'..='\u{faff}' // 兼容汉字
            | '\u{fe30}'..='\u{fe4f}' // 竖排标点
            | '\u{ff00}'..='\u{ffef}' // 全角/半角形式
            | '\u{20000}'..='\u{3134f}' // 汉字扩展 B–G
    )
}

#[cfg(test)]
pub(super) fn wide_runs_for_tests(content: &str) -> Vec<(String, bool)> {
    font_runs(content)
        .into_iter()
        .map(|(range, is_wide)| (content[range].to_string(), is_wide))
        .collect()
}

#[cfg(test)]
pub(super) fn styled_spans_for_tests(
    content: &str,
    tokens: &[TextToken],
) -> Vec<(String, bool, bool)> {
    styled_spans(content, tokens, HighlightPalette::Light)
        .into_iter()
        .map(|span| {
            (
                span.text.to_string(),
                span.font == Some(Font::DEFAULT),
                span.color.is_some(),
            )
        })
        .collect()
}
