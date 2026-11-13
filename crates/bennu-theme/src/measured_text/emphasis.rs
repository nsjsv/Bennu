//! 文字里要加粗的片段（搜索时名字里搜到的字）。中间省略后显示的是原文的头、省略号、
//! 尾，原文里的加粗区间要换算到显示文字上；落进被省掉那段的部分不显示。测量也按加粗
//! 排版：粗体更宽，按常规字重量出来的候选会画出界。

use std::ops::Range;

use iced::advanced::text::{self, Span};
use iced::font::Weight;
use iced::Font;

use super::{MiddleEllipsisKind, ELLIPSIS_MARKER};

/// 原文里的加粗区间换算到显示文字上。`displayed` 必须是同一 `kind` 给 `content` 产出的
/// 候选：整段、空，或头 + 省略号 + 尾。
pub(super) fn displayed_emphasis(
    content: &str,
    displayed: &str,
    kind: MiddleEllipsisKind,
    emphasized: &[Range<usize>],
) -> Vec<Range<usize>> {
    if emphasized.is_empty() || displayed.is_empty() {
        return Vec::new();
    }
    if displayed == content {
        return emphasized.to_vec();
    }
    let visible_chars = displayed.chars().count() - ELLIPSIS_MARKER.chars().count();
    let cut = kind.cut(content, visible_chars);
    let tail_offset = cut.head_end + ELLIPSIS_MARKER.len();
    let mut ranges = Vec::new();
    for range in emphasized {
        let head = range.start.min(cut.head_end)..range.end.min(cut.head_end);
        if !head.is_empty() {
            ranges.push(head);
        }
        let tail = range.start.max(cut.tail_start)..range.end.max(cut.tail_start);
        if !tail.is_empty() {
            ranges.push(
                tail.start - cut.tail_start + tail_offset..tail.end - cut.tail_start + tail_offset,
            );
        }
    }
    ranges
}

/// 排好的段落：没有加粗片段时走普通文字排版，有则拆成常规/粗体交替的片段。
pub(super) fn emphasized_paragraph<Paragraph>(
    template: text::Text<&str, Font>,
    emphasis: &[Range<usize>],
) -> Paragraph
where
    Paragraph: text::Paragraph<Font = Font>,
{
    if emphasis.is_empty() {
        return Paragraph::with_text(template);
    }
    let content = template.content;
    let bold = Font {
        weight: Weight::Bold,
        ..template.font
    };
    let mut spans: Vec<Span<'_, (), Font>> = Vec::with_capacity(emphasis.len() * 2 + 1);
    let mut cursor = 0;
    for range in emphasis {
        if range.start > cursor {
            spans.push(Span::new(&content[cursor..range.start]));
        }
        spans.push(Span::new(&content[range.clone()]).font(bold));
        cursor = range.end;
    }
    if cursor < content.len() {
        spans.push(Span::new(&content[cursor..]));
    }
    Paragraph::with_spans(template.with_content(spans.as_slice()))
}

#[cfg(test)]
#[path = "emphasis_tests.rs"]
mod tests;
