//! 搜索结果在列表、网格里共用的文字：所在文件夹怎么写、摘要里搜到的字怎么加粗、网格的
//! 悬停提示。

use std::ops::Range;
use std::path::Path;

use file_core::DirectoryEntry;
use iced::widget::text::{Span, Wrapping};
use iced::widget::{container, rich_text, Column};
use iced::{Element, Length};

use crate::appearance::context_menu_style;
use crate::model::{Message, SearchHitDetail};
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};

/// 所在文件夹，家目录缩成 `~`。
pub(crate) fn location_text(path: &Path, home: &Path) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    // 家目录拿不到时退成 `/`，那时每条路径都会被缩成 `~`，不缩。
    match parent.strip_prefix(home) {
        Ok(inside_home) if home != Path::new("/") => {
            if inside_home.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", inside_home.to_string_lossy())
            }
        }
        _ => parent.to_string_lossy().into_owned(),
    }
}

/// 搜到的字加粗：文字拆成常规与加粗交替的片段。换行、制表符换成空格（都是单字节，区间
/// 不变），摘要在列表里只占一行。
pub(crate) fn emphasized_spans(
    text: &str,
    highlights: &[Range<usize>],
) -> Vec<Span<'static, (), iced::Font>> {
    let single_line = text.replace(['\n', '\r', '\t'], " ");
    let mut spans = Vec::with_capacity(highlights.len() * 2 + 1);
    let mut cursor = 0;
    for range in highlights {
        if range.start > cursor {
            spans.push(Span::new(single_line[cursor..range.start].to_owned()));
        }
        spans.push(Span::new(single_line[range.clone()].to_owned()).font(BOLD));
        cursor = range.end;
    }
    if cursor < single_line.len() {
        spans.push(Span::new(single_line[cursor..].to_owned()));
    }
    spans
}

/// 网格里搜索结果的悬停提示：名字（搜到的字加粗）、所在文件夹、正文摘要。网格放不下
/// 位置和摘要，都放这里。
pub(crate) fn search_result_tooltip<'a>(
    entry: &DirectoryEntry,
    detail: Option<&SearchHitDetail>,
    home: &Path,
    width: f32,
) -> Element<'a, Message> {
    let name = entry.name().to_string_lossy();
    let name_highlights = detail.map_or(&[][..], |detail| &detail.name_highlights);
    let mut content = Column::new()
        .spacing(3)
        .push(rich_text(emphasized_spans(&name, name_highlights)).wrapping(Wrapping::WordOrGlyph))
        .push(
            readable_text(location_text(&entry.path, home))
                .size(CAPTION_SIZE)
                .wrapping(Wrapping::WordOrGlyph),
        );
    if let Some(detail) = detail {
        if let Some(snippet) = &detail.snippet {
            content = content.push(
                rich_text(emphasized_spans(snippet, &detail.snippet_highlights))
                    .size(CAPTION_SIZE)
                    .wrapping(Wrapping::WordOrGlyph),
            );
        }
    }
    container(content)
        .padding([5, 7])
        .width(Length::Fixed(width))
        .style(context_menu_style)
        .into()
}

#[cfg(test)]
#[path = "search_result_text_tests.rs"]
mod tests;
