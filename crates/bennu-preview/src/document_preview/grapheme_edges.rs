//! 词里每个字的边界落在词框的哪儿。
//!
//! pdftotext 只给每个词的框，不给每个字的。按默认字体量出词里每个字的宽度，按比例摊到词框里：复制按字
//! 切，一定准；高亮的边界与 PDF 里真实字形可能差一点。只在选区两端落进的那一两个词上现量，不预先量
//! 整份文档。

use iced::advanced::text::{self, Paragraph as _};
use iced::{alignment, Font, Pixels, Size};

use super::text_layer::grapheme_count;

/// 量字宽用的字号：只取比例，够大让取整误差可以忽略就行。
const MEASURE_SIZE: f32 = 100.0;

/// 每个字边界占词宽的比例：字数 + 1 个，从 0 到 1，递增。量不出来（空词、从右往左排的字）时平均分。
pub(crate) fn grapheme_edges(word: &str) -> Vec<f32> {
    let count = grapheme_count(word);
    let even = || {
        (0..=count)
            .map(|index| index as f32 / count.max(1) as f32)
            .collect()
    };
    if count == 0 {
        return even();
    }
    let paragraph = <iced::Renderer as text::Renderer>::Paragraph::with_text(text::Text {
        content: word,
        bounds: Size::INFINITE,
        size: Pixels(MEASURE_SIZE),
        line_height: text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    });
    let width = paragraph.min_width();
    let edges: Option<Vec<f32>> = (0..=count)
        .map(|index| {
            paragraph
                .grapheme_position(0, index)
                .map(|point| point.x / width)
        })
        .collect();
    match edges {
        Some(mut edges) if width > 0.0 && edges.windows(2).all(|pair| pair[0] <= pair[1]) => {
            edges[0] = 0.0;
            edges[count] = 1.0;
            edges
        }
        _ => even(),
    }
}

/// 词框里比例为 `fraction` 的地方离哪个字边界最近（0 = 词首，字数 = 词尾）。
pub(crate) fn nearest_edge(edges: &[f32], fraction: f32) -> usize {
    edges
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            (*left - fraction)
                .abs()
                .total_cmp(&(*right - fraction).abs())
        })
        .map_or(0, |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一样的字摊得一样宽；边界从 0 到 1 递增，个数是字数加一。
    #[test]
    fn edges_run_from_start_to_end_of_the_word() {
        let edges = grapheme_edges("aaaa");
        assert_eq!(edges.len(), 5);
        assert_eq!((edges[0], edges[4]), (0.0, 1.0));
        assert!((edges[2] - 0.5).abs() < 0.02, "{edges:?}");
        assert_eq!(grapheme_edges("é中a").len(), 4);
        assert_eq!(nearest_edge(&edges, 0.49), 2);
        assert_eq!(nearest_edge(&edges, -0.3), 0);
        assert_eq!(nearest_edge(&edges, 1.4), 4);
    }
}
