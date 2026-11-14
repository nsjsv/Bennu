//! 查看器里的搜索：匹配的底色（随段落缓存，搜索变了才重算）、搜索与跳行快捷键、关掉搜索条
//! 后把键盘交回查看器的操作。

use std::any::Any;

use iced::advanced::text::Renderer as TextRenderer;
use iced::advanced::widget::operation::{Focusable, Operation};
use iced::advanced::{renderer, widget};
use iced::{keyboard, Color, Font, Rectangle, Task, Theme};

use super::selection::{columns_in_row, row_range_quads};
use super::{PreviewParagraph, TextPreviewViewerState, VisibleRow};
use crate::text_document::{FindEvent, SearchDirection, TextPreviewDocument, TextRow};

/// 选中的文字不超过这么多字节、又在一行之内时，Ctrl+F 拿它当搜索词。
const SEED_MAX_BYTES: usize = 200;

/// 一处匹配在一行里的底色矩形（相对这一行左上角）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MatchMark {
    pub(super) bounds: Rectangle,
    pub(super) current: bool,
}

/// 这一显示行里各个匹配的底色。匹配按起点排好，一处匹配最多跨两段，所以只要再看起点在
/// 这一行之前的最后一个。
pub(super) fn row_match_marks(
    document: &TextPreviewDocument,
    row: &TextRow,
    paragraph: &PreviewParagraph,
    base: f32,
) -> Vec<MatchMark> {
    let matches = document.search_matches();
    if matches.is_empty() {
        return Vec::new();
    }
    let current = document.current_match();
    let first = matches
        .partition_point(|found| found.start.row < row.start)
        .saturating_sub(1);
    let mut marks = Vec::new();
    for found in &matches[first..] {
        if found.start.row > row.start {
            break;
        }
        let Some((from, to)) = columns_in_row(row, found.start, found.end) else {
            continue;
        };
        let is_current = current == Some(*found);
        marks.extend(
            row_range_quads(paragraph, &row.text, base, from, to)
                .into_iter()
                .map(|bounds| MatchMark {
                    bounds,
                    current: is_current,
                }),
        );
    }
    marks
}

pub(super) fn draw_match_marks(
    renderer: &mut impl TextRenderer<Paragraph = PreviewParagraph, Font = Font>,
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    visible: &[VisibleRow],
    text_bounds: Rectangle,
    theme: &Theme,
) {
    let rows = document.rows();
    for entry in visible {
        let start = rows[entry.index].start;
        let Some(retained) = state.retained_rows.iter().find(|row| row.start == start) else {
            continue;
        };
        for mark in &retained.marks {
            let bounds = Rectangle {
                x: mark.bounds.x + text_bounds.x - state.scroll_x,
                y: mark.bounds.y + text_bounds.y + entry.visible_y,
                ..mark.bounds
            };
            let Some(bounds) = bounds.intersection(&text_bounds) else {
                continue;
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    ..renderer::Quad::default()
                },
                match_color(theme, mark.current),
            );
        }
    }
}

/// 所有匹配浅黄，当前匹配橙；深色主题底色本来就深，透明度低一点。
fn match_color(theme: &Theme, current: bool) -> Color {
    let dark = matches!(
        bennu_theme::ui_colors(theme).mode,
        bennu_theme::AppearanceMode::Dark
    );
    match (current, dark) {
        (false, false) => Color::from_rgba8(250, 204, 21, 0.38),
        (false, true) => Color::from_rgba8(250, 204, 21, 0.24),
        (true, false) => Color::from_rgba8(249, 115, 22, 0.55),
        (true, true) => Color::from_rgba8(251, 146, 60, 0.55),
    }
}

/// 查看器聚焦时的搜索与跳行快捷键：Ctrl+F、Ctrl+G、F3、Shift+F3。
pub(super) fn find_shortcut(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<FindEvent> {
    match key.as_ref() {
        keyboard::Key::Named(keyboard::key::Named::F3) if modifiers.shift() => {
            Some(FindEvent::Step(SearchDirection::Backward))
        }
        keyboard::Key::Named(keyboard::key::Named::F3) => {
            Some(FindEvent::Step(SearchDirection::Forward))
        }
        keyboard::Key::Character(character) if modifiers.command() => {
            match character.to_ascii_lowercase().as_str() {
                "f" => Some(FindEvent::OpenSearch {
                    seed: selection_seed(state, document),
                }),
                "g" => Some(FindEvent::OpenGoto),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 选区在一行之内、不太长时的文字。
fn selection_seed(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> Option<String> {
    let (anchor, cursor) = state.selection_anchor.zip(state.cursor)?;
    if anchor.row != cursor.row || anchor.column == cursor.column {
        return None;
    }
    let index = document.row_index(anchor.row).ok()?;
    let text = &document.rows()[index].text;
    let low = text.floor_char_boundary(anchor.column.min(cursor.column));
    let high = text.floor_char_boundary(anchor.column.max(cursor.column));
    (high - low <= SEED_MAX_BYTES && low < high).then(|| text[low..high].to_owned())
}

/// 把键盘交给文本查看器（关掉搜索条、跳行之后）。按类型找到查看器，不需要宿主给的部件 id：
/// 其余可聚焦的控件都失焦，查看器在自己报告的 `custom` 里被聚焦。
pub(crate) fn focus_text_viewer<T: Send + 'static>() -> Task<T> {
    iced::advanced::widget::operate(FocusTextViewer)
}

struct FocusTextViewer;

impl<T> Operation<T> for FocusTextViewer {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        state.unfocus();
    }

    fn custom(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Any) {
        if let Some(viewer) = state.downcast_mut::<TextPreviewViewerState>() {
            viewer.is_focused = true;
        }
    }
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
