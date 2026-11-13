//! 列表在搜索结果里多出来的东西：名字里搜到的字加粗、名字下面一行正文摘要、位置列。

use std::path::Path;

use file_core::DirectoryEntry;
use iced::widget::text::Wrapping;
use iced::widget::{container, rich_text, row, Space};
use iced::{Element, Length};

use super::{list_column_width, ListGeometry};
use crate::app::panes::BrowserPaneView;
use crate::file_entry_view::FileEntryVisualState;
use crate::model::{FileEntryContentModifier, Message, SearchHitDetail};
use crate::search_result_text::{emphasized_spans, location_text};
use bennu_theme::measured_text::{
    measured_middle_ellipsized_emphasized_text, measured_middle_ellipsized_text,
};
use bennu_theme::typography::CAPTION_SIZE;

/// 摘要比名字淡一些，选中、悬停时跟着行的文字颜色走。
const SNIPPET_TEXT_ALPHA: f32 = 0.72;
/// 摘要行与行高的比例：默认档行高 46 时摘要行 18。
const SNIPPET_LINE_TO_ROW_RATIO: f32 = 18.0 / 46.0;

/// 搜索结果名字下面那行摘要的高度，随档位缩放；行流的几何与渲染共用。
pub(crate) fn list_snippet_line_height(row_height: f32) -> f32 {
    (row_height * SNIPPET_LINE_TO_ROW_RATIO).round()
}

fn search_detail<'a>(
    pane: BrowserPaneView<'a>,
    entry: &DirectoryEntry,
) -> Option<&'a SearchHitDetail> {
    pane.location.search()?.detail(&entry.path)
}

/// 搜索结果里名字有搜到的字时，名字按加粗片段显示；其他时候 None，用普通名字。
pub(super) fn emphasized_name<'a>(
    pane: BrowserPaneView<'a>,
    entry: &DirectoryEntry,
    text_size: f32,
) -> Option<Element<'a, Message>> {
    let detail = search_detail(pane, entry)?;
    if detail.name_highlights.is_empty() {
        return None;
    }
    Some(measured_middle_ellipsized_emphasized_text(
        entry.name().to_string_lossy().into_owned(),
        detail.name_highlights.clone(),
        text_size,
    ))
}

/// 名字下面的正文摘要行：从名字文字的起点开始，铺到行尾，超出部分裁掉。
pub(super) fn snippet_line<'a>(
    pane: BrowserPaneView<'a>,
    entry: &DirectoryEntry,
    geometry: &ListGeometry,
    depth: usize,
    visual_state: FileEntryVisualState,
    modifier: FileEntryContentModifier,
    line_height: f32,
) -> Option<Element<'a, Message>> {
    let detail = search_detail(pane, entry)?;
    let snippet = detail.snippet.as_deref()?;
    // 与名字列里名字文字的起点对齐：缩进、展开箭头槽、图标，各隔一个行内间距。
    let name_start = depth as f32 * geometry.indent_width
        + geometry.toggle_width
        + geometry.icon_density.thumbnail_size()
        + geometry.row_spacing * 2.0;
    let text_size = (CAPTION_SIZE * geometry.scale).round();
    let content_style = visual_state.content_style(modifier);
    Some(
        container(row![
            Space::new().width(Length::Fixed(name_start)),
            rich_text(emphasized_spans(snippet, &detail.snippet_highlights))
                .size(text_size)
                .wrapping(Wrapping::None),
        ])
        .height(Length::Fixed(line_height))
        .width(Length::Fill)
        .clip(true)
        .style(move |theme| {
            let mut style = content_style(theme);
            style.text_color = style
                .text_color
                .map(|color| color.scale_alpha(SNIPPET_TEXT_ALPHA));
            style
        })
        .into(),
    )
}

/// 位置列：所在文件夹，家目录缩成 `~`；放不下时省略中间。
pub(super) fn location_cell<'a>(
    entry: &DirectoryEntry,
    home: &Path,
    geometry: &ListGeometry,
    width: f32,
) -> Element<'a, Message> {
    container(measured_middle_ellipsized_text(
        location_text(&entry.path, home),
        geometry.text_size,
    ))
    .width(list_column_width(width))
    .clip(true)
    .into()
}

#[cfg(test)]
mod tests {
    use super::list_snippet_line_height;

    #[test]
    fn snippet_line_scales_with_row_height() {
        assert_eq!(list_snippet_line_height(46.0), 18.0);
        assert_eq!(list_snippet_line_height(69.0), 27.0);
    }
}
