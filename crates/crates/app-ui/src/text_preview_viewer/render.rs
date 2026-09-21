//! 查看器绘制辅助：选区高亮、可见行段落与行号槽的布局换算。

use iced::advanced::text::{Paragraph as _, Renderer as TextRenderer};
use iced::advanced::Renderer as _;
use iced::advanced::{renderer, text};
use iced::{alignment, Font, Padding, Pixels, Point, Rectangle, Size, Theme};

use super::style::{placeholder_color, selection_color, text_color};
use super::{
    selection, visible_logical_lines, PreviewParagraph, TextPreviewViewerState,
    TEXT_PREVIEW_DIVIDER_WIDTH, TEXT_PREVIEW_GUTTER_DIGIT_WIDTH,
    TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING, TEXT_PREVIEW_GUTTER_MIN_WIDTH,
    TEXT_PREVIEW_GUTTER_SPACING, TEXT_PREVIEW_LINE_HEIGHT, TEXT_PREVIEW_TEXT_SIZE,
    TEXT_PREVIEW_VIEWER_PADDING,
};
use crate::text_preview::TextPreviewDocument;

/// 选区高亮：直接按查看器坐标系绘制（选区状态自管后不再有
/// 编辑器视口与查看器锚点的偏移差换算）。
pub(super) fn draw_selection_ranges(
    renderer: &mut iced::Renderer,
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    text_bounds: Rectangle,
    theme: &Theme,
) {
    let origin = text_bounds.position();
    for quad in selection::selection_quads(state, document) {
        let bounds = Rectangle {
            x: quad.x + origin.x,
            y: quad.y + origin.y,
            ..quad
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

pub(super) fn draw_visible_text_lines(
    renderer: &mut iced::Renderer,
    state: &TextPreviewViewerState,
    text_bounds: Rectangle,
    theme: &Theme,
    viewport: &Rectangle,
) {
    let Some(clip_bounds) = text_bounds.intersection(viewport) else {
        return;
    };
    let color = text_color(theme);
    for entry in visible_logical_lines(state, text_bounds.height) {
        let Some(text_line) = state
            .retained_text_lines
            .iter()
            .find(|line| line.line_index == entry.line_index)
        else {
            continue;
        };
        renderer.fill_paragraph(
            &text_line.paragraph,
            Point::new(text_bounds.x, text_bounds.y + entry.visible_y),
            color,
            clip_bounds,
        );
    }
}

/// 空文件时以占位色提示。
pub(super) fn draw_empty_placeholder(
    renderer: &mut iced::Renderer,
    text_bounds: Rectangle,
    theme: &Theme,
) {
    renderer.fill_text(
        text::Text {
            content: "(empty file)".into(),
            bounds: text_bounds.size(),
            size: Pixels(TEXT_PREVIEW_TEXT_SIZE),
            line_height: text::LineHeight::Relative(TEXT_PREVIEW_LINE_HEIGHT),
            font: Font::MONOSPACE,
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

pub(super) fn text_bounds(bounds: Rectangle, document: &TextPreviewDocument) -> Rectangle {
    let gutter_width = text_preview_line_number_gutter_width(document);
    let x = bounds.x + gutter_width + TEXT_PREVIEW_GUTTER_SPACING + TEXT_PREVIEW_DIVIDER_WIDTH;
    Rectangle {
        x,
        y: bounds.y,
        width: (bounds.width - (x - bounds.x)).max(0.0),
        height: bounds.height,
    }
    .shrink(Padding::new(TEXT_PREVIEW_VIEWER_PADDING))
}

pub(super) fn text_preview_line_number_gutter_width(document: &TextPreviewDocument) -> f32 {
    let digit_width = document.line_number_digit_count() as f32 * TEXT_PREVIEW_GUTTER_DIGIT_WIDTH;
    let padding_width = TEXT_PREVIEW_GUTTER_HORIZONTAL_PADDING * 2.0;

    (digit_width + padding_width).max(TEXT_PREVIEW_GUTTER_MIN_WIDTH)
}

pub(super) fn line_number_bounds(gutter_bounds: Rectangle, y: f32) -> Rectangle {
    Rectangle {
        x: gutter_bounds.x,
        y,
        width: gutter_bounds.width,
        height: TEXT_PREVIEW_TEXT_SIZE * TEXT_PREVIEW_LINE_HEIGHT,
    }
}

pub(super) fn line_number_paragraph(
    line_number: usize,
    digit_count: usize,
    gutter_width: f32,
) -> PreviewParagraph {
    let size = Pixels(TEXT_PREVIEW_TEXT_SIZE);
    let line_height = text::LineHeight::Relative(TEXT_PREVIEW_LINE_HEIGHT);
    let label = format!("{line_number:>digit_count$}");
    PreviewParagraph::with_text(text::Text {
        content: label.as_str(),
        bounds: Size::new(gutter_width, line_height.to_absolute(size).0),
        size,
        line_height,
        font: Font::MONOSPACE,
        align_x: text::Alignment::Right,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    })
}

pub(super) fn text_line_paragraph(content: &str, width: f32, height: f32) -> PreviewParagraph {
    PreviewParagraph::with_text(text::Text {
        content,
        bounds: Size::new(width, height),
        size: Pixels(TEXT_PREVIEW_TEXT_SIZE),
        line_height: text::LineHeight::Relative(TEXT_PREVIEW_LINE_HEIGHT),
        font: Font::MONOSPACE,
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::WordOrGlyph,
    })
}
