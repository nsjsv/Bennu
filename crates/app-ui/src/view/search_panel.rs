use iced::widget::{button, container, mouse_area, row, scrollable, tooltip, Column};
use iced::{Alignment, Element, Length};

use super::{themed_icon, IconTone};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    context_menu_style, enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction,
    path_suggestion_item_style, path_suggestions_style, transparent_button_style,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::model::{Message, ScrollbarRegion};
use bennu_preview::panel_text::readable_text;
use bennu_theme::anchored_popup::anchored_popup;
use bennu_theme::controls::icon_button_style;
use bennu_theme::controls::text_field;
use bennu_theme::icons::IconSymbol;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::typography::CAPTION_SIZE;

const SEARCH_INPUT_WIDTH: f32 = 140.0;
const SEARCHING_INPUT_WIDTH: f32 = 320.0;
const SEARCH_HISTORY_PANEL_MAX_HEIGHT: f32 = 320.0;
const SEARCH_HISTORY_KEYWORD_MAX_CHARS: usize = 28;

/// 平时窄，搜索时变宽，长一点的词也看得全。
fn search_input_width(browser: &FileBrowser) -> Length {
    Length::Fixed(if browser.active_search().is_some() {
        SEARCHING_INPUT_WIDTH
    } else {
        SEARCH_INPUT_WIDTH
    })
}

pub(crate) fn search_input_id() -> iced::widget::Id {
    iced::widget::Id::from("search-input")
}

pub(super) fn search_input_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let input_value = browser
        .active_search()
        .map(|search| search.input.as_str())
        .unwrap_or_default();
    let input = text_field(&tr!("Search"), input_value)
        .id(search_input_id())
        .on_input(Message::SearchInputChanged)
        .on_submit(Message::SearchSubmitted)
        .width(Length::Fill);
    let content = if input_value.is_empty() {
        row![input].spacing(4).align_y(Alignment::Center)
    } else {
        row![
            input,
            tooltip(
                button(themed_icon(IconSymbol::Close, IconTone::Normal, 12.0))
                    .on_press(Message::SearchKeywordCleared)
                    .padding([8, 8])
                    .style(icon_button_style),
                tooltip_label(tr!("Clear search text")),
                tooltip::Position::Bottom,
            )
        ]
        .spacing(4)
        .align_y(Alignment::Center)
    };
    let anchor = container(content.width(Length::Fill)).width(search_input_width(browser));
    let popup = browser
        .search_history_interaction
        .popup_is_visible(&browser.user_config().search_history)
        .then(|| search_history_panel(browser));
    anchored_popup(anchor, popup)
}

fn search_history_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let mut history = Column::new()
        .push(
            container(readable_text(tr!("Recent searches")))
                .padding([6, 8])
                .width(Length::Fill),
        )
        .spacing(3)
        .padding(4)
        .width(Length::Fill);
    for keyword in browser.user_config().search_history.entries() {
        history = history.push(search_history_row(keyword));
    }
    history = history.push(
        button(readable_text(tr!("Clear search history")))
            .on_press(Message::SearchHistoryCleared)
            .padding([6, 8])
            .width(Length::Fill)
            .style(transparent_button_style()),
    );

    let region = ScrollbarRegion::SearchHistory;
    let history = scrollable(smooth_scroll_content(history, region.clone()))
        .id(smooth_scroll_id(&region))
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .height(Length::Shrink);
    let history = enhanced_scrollbar(history, ScrollbarAxis::Vertical);

    mouse_area(
        container(history)
            .width(Length::Fill)
            .max_height(SEARCH_HISTORY_PANEL_MAX_HEIGHT)
            .style(path_suggestions_style),
    )
    .on_enter(Message::SearchHistoryPopupPointerEntered)
    .on_exit(Message::SearchHistoryPopupPointerExited)
    .into()
}

fn search_history_row(keyword: &str) -> Element<'static, Message> {
    let displayed_keyword =
        format_middle_ellipsized_text(keyword, SEARCH_HISTORY_KEYWORD_MAX_CHARS);
    let select_keyword = button(readable_text(displayed_keyword).width(Length::Fill))
        .on_press(Message::SearchHistoryKeywordSelected(keyword.to_owned()))
        .padding([6, 8])
        .width(Length::Fill)
        .style(transparent_button_style());
    let remove_keyword = tooltip(
        button(themed_icon(IconSymbol::Close, IconTone::Normal, 11.0))
            .on_press(Message::SearchHistoryKeywordRemoved(keyword.to_owned()))
            .padding([6, 7])
            .style(icon_button_style),
        tooltip_label(tr!("Remove from search history")),
        tooltip::Position::Bottom,
    );

    container(
        row![select_keyword, remove_keyword]
            .spacing(3)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .style(path_suggestion_item_style)
    .into()
}

pub(super) fn tooltip_label(label: String) -> Element<'static, Message> {
    container(readable_text(label).size(CAPTION_SIZE))
        .padding([5, 7])
        .style(context_menu_style)
        .into()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config;
    use crate::model::search::{SearchWorkspaceSessionId, SearchWorkspaceState};
    use crate::model::BrowserLocation;

    #[test]
    fn the_search_box_widens_while_searching() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/tmp"));
        assert_eq!(
            search_input_width(&browser),
            Length::Fixed(SEARCH_INPUT_WIDTH)
        );
        browser.tab_mut().location = BrowserLocation::Search(Box::new(SearchWorkspaceState::new(
            PathBuf::from("/tmp"),
            PathBuf::from("/home/user"),
            None,
            SearchWorkspaceSessionId(1),
        )));
        assert_eq!(
            search_input_width(&browser),
            Length::Fixed(SEARCHING_INPUT_WIDTH)
        );
    }
}
