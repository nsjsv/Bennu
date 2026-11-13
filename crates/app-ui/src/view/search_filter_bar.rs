//! 搜索结果上方的筛选栏：一行小按钮（范围、类型、文本范围、时间、大小、正则），每个写着
//! 当前值，改过的高亮；点开是下拉。末尾是结果数、提示图标、关闭。窗口窄时按钮折到下一行，
//! 所有筛选始终看得见。

use file_search::{SearchMatchMode, SearchTextScope};
use iced::widget::{button, container, row, tooltip, Column, Row, Space};
use iced::{Alignment, Element, Length};

use super::search_panel::tooltip_label;
use super::{themed_icon, IconTone};
use crate::app::FileBrowser;
use crate::appearance::list_panel_style;
use crate::model::search::SearchFilterPresetState;
use crate::model::{
    Message, SearchDateField, SearchDatePreset, SearchDirectoryScope, SearchEntryTypePreset,
    SearchFilterMenu, SearchFilterPick, SearchResultCompletion, SearchSizePreset,
    SearchWorkspaceState, SEARCH_RESULT_TOTAL_LIMIT,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::anchored_popup::{anchored_dropdown, dismiss_on_outside_press};
use bennu_theme::controls::{
    icon_button_style, segmented_choice_row, text_field, toggle_button_style, SegmentedChoice,
};
use bennu_theme::icons::{icon_tone_style, rotated_chevron_right_view, IconSymbol};
use bennu_theme::styles::{context_menu_item_button_style, context_menu_style, muted_text_color};
use bennu_theme::typography::CAPTION_SIZE;

const FILTER_BUTTON_HEIGHT: f32 = 28.0;
const FILTER_ICON_SIZE: f32 = 13.0;
const CHEVRON_SIZE: f32 = 10.0;
/// 下拉宽度按内容定死：里面的选项横向铺满，没有定宽时会一路撑到窗口那么宽。
const NARROW_MENU_WIDTH: f32 = 200.0;
const WIDE_MENU_WIDTH: f32 = 260.0;
const MENU_ITEM_CHECK_SIZE: f32 = 14.0;
/// 类型名在按钮上最多写两个，再多写数量。
const NAMED_TYPE_LIMIT: usize = 2;

pub(super) fn search_filter_bar<'a>(
    browser: &'a FileBrowser,
    search: &'a SearchWorkspaceState,
    loaded_results: usize,
) -> Element<'a, Message> {
    let open = browser.search_filter_menu;
    let filters = &search.filters;
    let type_order = browser
        .user_config()
        .context_menus
        .search_entry_type_items();
    let mut buttons = Row::new()
        .push(scope_filter(search, open))
        .push(types_filter(filters, &type_order, open))
        .push(text_scope_filter(filters, open))
        .push(time_filter(filters, open))
        .push(size_filter(filters, open))
        .push(regex_toggle(filters));
    if !filters.is_default() {
        buttons = buttons.push(
            button(readable_text(tr!("Reset filters")).size(CAPTION_SIZE))
                .on_press(Message::SearchFiltersReset)
                .height(Length::Fixed(FILTER_BUTTON_HEIGHT))
                .padding([4, 10])
                .style(icon_button_style),
        );
    }
    let buttons = buttons
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .wrap()
        .vertical_spacing(6);

    container(
        row![buttons, status_area(search, loaded_results)]
            .spacing(10)
            .align_y(Alignment::Start),
    )
    .padding([7, 10])
    .width(Length::Fill)
    .style(list_panel_style)
    .into()
}

/// 结果数、提示图标、关闭。
fn status_area<'a>(search: &SearchWorkspaceState, loaded_results: usize) -> Element<'a, Message> {
    let mut status = Row::new().spacing(8).align_y(Alignment::Center);
    if let Some(count) = result_count_label(search, loaded_results) {
        status = status.push(readable_text(count).size(CAPTION_SIZE).style(|theme| {
            iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }
        }));
    }
    let notices = search_notices(search, loaded_results);
    if !notices.is_empty() {
        status = status.push(tooltip(
            themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, 14.0),
            tooltip_label(notices.join("\n")),
            tooltip::Position::Bottom,
        ));
    }
    status
        .push(tooltip(
            button(themed_icon(IconSymbol::Close, IconTone::Normal, 13.0))
                .on_press(Message::SearchWorkspaceClosed)
                .padding([6, 8])
                .style(icon_button_style),
            tooltip_label(tr!("Close search")),
            tooltip::Position::Bottom,
        ))
        .height(Length::Fixed(FILTER_BUTTON_HEIGHT))
        .into()
}

/// 已加载的结果数，还有下一页时带「+」。没有结果时不显示，列表的空提示会说明原因。
fn result_count_label(search: &SearchWorkspaceState, loaded_results: usize) -> Option<String> {
    if loaded_results == 0 {
        return None;
    }
    Some(
        if search.window.completion == Some(SearchResultCompletion::MoreAvailable) {
            tr!("%1+ results", loaded_results)
        } else {
            tr!("%1 results", loaded_results)
        },
    )
}

/// 结果之外要说的话：退化、正则限制、翻页失败、部分结果、截断。空结果的原因在列表里说。
fn search_notices(search: &SearchWorkspaceState, loaded_results: usize) -> Vec<String> {
    let mut notices = Vec::new();
    if search.content_search_is_degraded() {
        notices.push(tr!(
            "Content indexing is unavailable; matching file names only."
        ));
    }
    if search.filters.match_mode == SearchMatchMode::Regex {
        notices.push(tr!("Regex mode matches file names only."));
    }
    if loaded_results > 0 {
        if let Some(failure) = &search.window.failure {
            notices.push(failure.clone());
        }
    }
    if !search.window.is_loading {
        if let Some(SearchResultCompletion::Partial { inspected_entries }) =
            search.window.completion
        {
            notices.push(tr!(
                "Partial results after inspecting %1 entries",
                inspected_entries
            ));
        }
        if search.window.truncated {
            notices.push(tr!(
                "Result limit reached (%1); the oldest results were dropped. Narrow the search to see more.",
                SEARCH_RESULT_TOTAL_LIMIT
            ));
        }
    }
    notices
}

fn scope_filter<'a>(
    search: &SearchWorkspaceState,
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    let selected = search.root.selected_scope();
    let last_location = search
        .root
        .last_location_directory()
        .map(|path| path.to_string_lossy().into_owned());
    let menu = SearchFilterMenu::Scope;
    let items = search
        .root
        .available_scopes()
        .into_iter()
        .map(|scope| {
            // 「上次位置」只写档名，后面补上是哪个文件夹。
            let detail = (scope == SearchDirectoryScope::LastLocation)
                .then(|| last_location.clone())
                .flatten();
            menu_item(
                scope.label(),
                detail,
                scope == selected,
                Message::SearchFilterPicked(SearchFilterPick::Scope(scope)),
            )
        })
        .collect::<Vec<_>>();
    anchored_dropdown(
        filter_button(
            Some(IconSymbol::Search),
            selected.label(),
            false,
            menu,
            open,
        ),
        (open == Some(menu)).then(|| menu_panel(items, WIDE_MENU_WIDTH)),
    )
}

fn types_filter<'a>(
    filters: &'a SearchFilterPresetState,
    type_order: &[SearchEntryTypePreset],
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    let menu = SearchFilterMenu::Types;
    let active = !filters.selected_entry_types.is_empty() || filters.custom_extensions_are_active();
    let panel = (open == Some(menu)).then(|| {
        let mut items = type_order
            .iter()
            .map(|entry_type| {
                menu_item(
                    entry_type.label(),
                    None,
                    filters.entry_type_is_selected(*entry_type),
                    Message::SearchEntryTypeToggled(*entry_type),
                )
            })
            .collect::<Vec<_>>();
        items.push(menu_section(
            tr!("Custom extensions"),
            text_field(&tr!("e.g. pdf, docx"), &filters.custom_extensions)
                .on_input(Message::SearchCustomExtensionsChanged)
                .width(Length::Fill)
                .into(),
        ));
        menu_panel(items, NARROW_MENU_WIDTH)
    });
    anchored_dropdown(
        filter_button(
            Some(IconSymbol::File),
            types_label(filters, type_order),
            active,
            menu,
            open,
        ),
        panel,
    )
}

/// 没选写「所有类型」；选一两种写名字（自定义后缀算一种，写「自定义」）；更多写数量。
fn types_label(filters: &SearchFilterPresetState, type_order: &[SearchEntryTypePreset]) -> String {
    let mut names = type_order
        .iter()
        .filter(|entry_type| filters.entry_type_is_selected(**entry_type))
        .map(|entry_type| entry_type.label())
        .collect::<Vec<_>>();
    // 在设置里藏起来的类型仍可能选着（藏之前选的），也要算上。
    names.extend(
        filters
            .selected_entry_types
            .iter()
            .filter(|entry_type| !type_order.contains(entry_type))
            .map(|entry_type| entry_type.label()),
    );
    if filters.custom_extensions_are_active() {
        names.push(tr!("Custom"));
    }
    match names.len() {
        0 => tr!("All types"),
        1 => names.remove(0),
        count if count <= NAMED_TYPE_LIMIT => tr!("%1, %2", names[0].clone(), names[1].clone()),
        count => tr!("%1 types", count),
    }
}

fn text_scope_filter<'a>(
    filters: &SearchFilterPresetState,
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    let menu = SearchFilterMenu::TextScope;
    // 正则只在名字上执行：固定显示「仅文件名」且不能点，与提交的查询一致。
    if filters.match_mode == SearchMatchMode::Regex {
        return filter_button_face(None, tr!("Name only"), false).into();
    }
    let label = |scope| match scope {
        SearchTextScope::NameAndContent => tr!("Name & content"),
        SearchTextScope::NameOnly => tr!("Name only"),
    };
    let items = [SearchTextScope::NameAndContent, SearchTextScope::NameOnly]
        .into_iter()
        .map(|scope| {
            menu_item(
                label(scope),
                None,
                scope == filters.text_scope,
                Message::SearchFilterPicked(SearchFilterPick::TextScope(scope)),
            )
        })
        .collect::<Vec<_>>();
    anchored_dropdown(
        filter_button(
            None,
            label(filters.text_scope),
            filters.text_scope != SearchTextScope::NameAndContent,
            menu,
            open,
        ),
        (open == Some(menu)).then(|| menu_panel(items, NARROW_MENU_WIDTH)),
    )
}

fn time_filter<'a>(
    filters: &SearchFilterPresetState,
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    let menu = SearchFilterMenu::Time;
    let panel = (open == Some(menu)).then(|| {
        let fields = SearchDateField::ALL
            .into_iter()
            .map(|field| SegmentedChoice {
                label: field.label(),
                selected: field == filters.date_field,
                message: Some(Message::SearchDateFieldSelected(field)),
                tooltip: None,
            })
            .collect::<Vec<_>>();
        let mut items = vec![container(segmented_choice_row(fields))
            .padding([4, 6])
            .into()];
        items.extend(SearchDatePreset::ALL.into_iter().map(|preset| {
            menu_item(
                preset.label(),
                None,
                preset == filters.date_preset,
                Message::SearchFilterPicked(SearchFilterPick::DatePreset(preset)),
            )
        }));
        menu_panel(items, WIDE_MENU_WIDTH)
    });
    anchored_dropdown(
        filter_button(
            Some(IconSymbol::History),
            time_label(filters),
            filters.date_preset != SearchDatePreset::Any,
            menu,
            open,
        ),
        panel,
    )
}

/// 「任意时间」或「修改于近 7 天」。
fn time_label(filters: &SearchFilterPresetState) -> String {
    if filters.date_preset == SearchDatePreset::Any {
        return filters.date_preset.label();
    }
    let preset = filters.date_preset.label();
    match filters.date_field {
        SearchDateField::Modified => tr!("Modified · %1", preset),
        SearchDateField::Created => tr!("Created · %1", preset),
        SearchDateField::Accessed => tr!("Accessed · %1", preset),
    }
}

fn size_filter<'a>(
    filters: &'a SearchFilterPresetState,
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    let menu = SearchFilterMenu::Size;
    let panel = (open == Some(menu)).then(|| {
        let mut items = SearchSizePreset::ALL
            .into_iter()
            .map(|preset| {
                menu_item(
                    preset.label(),
                    None,
                    preset == filters.size_preset,
                    Message::SearchFilterPicked(SearchFilterPick::Size(preset)),
                )
            })
            .collect::<Vec<_>>();
        // 自定义大小的两侧输入跟着「自定义」档出现在下拉底部。
        if filters.size_preset == SearchSizePreset::Custom {
            items.push(menu_section(
                tr!("Min size"),
                text_field(&tr!("e.g. 10MB"), &filters.custom_size_min)
                    .on_input(Message::SearchCustomSizeMinChanged)
                    .width(Length::Fill)
                    .into(),
            ));
            items.push(menu_section(
                tr!("Max size"),
                text_field(&tr!("e.g. 2 GB"), &filters.custom_size_max)
                    .on_input(Message::SearchCustomSizeMaxChanged)
                    .width(Length::Fill)
                    .into(),
            ));
        }
        menu_panel(items, WIDE_MENU_WIDTH)
    });
    anchored_dropdown(
        filter_button(
            Some(IconSymbol::HardDrive),
            filters.size_preset.label(),
            filters.size_preset != SearchSizePreset::Any,
            menu,
            open,
        ),
        panel,
    )
}

fn regex_toggle<'a>(filters: &SearchFilterPresetState) -> Element<'a, Message> {
    let active = filters.match_mode == SearchMatchMode::Regex;
    button(
        row![
            themed_icon(IconSymbol::Regex, tone(active), FILTER_ICON_SIZE),
            readable_text(tr!("Regex")).size(CAPTION_SIZE),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .on_press(Message::SearchRegexToggled)
    .height(Length::Fixed(FILTER_BUTTON_HEIGHT))
    .padding([4, 9])
    .style(toggle_button_style(active))
    .into()
}

fn tone(active: bool) -> IconTone {
    if active {
        IconTone::OnAccent
    } else {
        IconTone::Normal
    }
}

/// 带当前值的筛选按钮。下拉开着时不发“打开”：点它算点在下拉外面，由下拉自己关掉，
/// 否则会关了又马上重开。
fn filter_button<'a>(
    icon: Option<IconSymbol>,
    label: String,
    active: bool,
    menu: SearchFilterMenu,
    open: Option<SearchFilterMenu>,
) -> Element<'a, Message> {
    filter_button_face(icon, label, active)
        .on_press_maybe((open != Some(menu)).then_some(Message::SearchFilterMenuOpened(menu)))
        .into()
}

fn filter_button_face<'a>(
    icon: Option<IconSymbol>,
    label: String,
    active: bool,
) -> button::Button<'a, Message> {
    let mut content = Row::new().spacing(5).align_y(Alignment::Center);
    if let Some(icon) = icon {
        content = content.push(themed_icon(icon, tone(active), FILTER_ICON_SIZE));
    }
    content = content
        .push(readable_text(label).size(CAPTION_SIZE))
        .push(rotated_chevron_right_view(90.0, CHEVRON_SIZE).style(icon_tone_style(tone(active))));
    button(content)
        .height(Length::Fixed(FILTER_BUTTON_HEIGHT))
        .padding([4, 9])
        .style(toggle_button_style(active))
}

/// 下拉面板：点外面就关。
fn menu_panel<'a>(items: Vec<Element<'a, Message>>, width: f32) -> Element<'a, Message> {
    dismiss_on_outside_press(
        container(Column::with_children(items).spacing(1).padding(4))
            .width(Length::Fixed(width))
            .style(context_menu_style),
        Message::SearchFilterMenuClosed,
    )
}

/// 一行选项：前面是勾（选中时），后面可带一段淡色说明。
fn menu_item<'a>(
    label: String,
    detail: Option<String>,
    checked: bool,
    message: Message,
) -> Element<'a, Message> {
    let check: Element<'a, Message> = if checked {
        themed_icon(IconSymbol::Check, IconTone::Normal, MENU_ITEM_CHECK_SIZE).into()
    } else {
        Space::new()
            .width(Length::Fixed(MENU_ITEM_CHECK_SIZE))
            .height(Length::Fixed(MENU_ITEM_CHECK_SIZE))
            .into()
    };
    let mut content = row![check, readable_text(label)]
        .spacing(8)
        .align_y(Alignment::Center);
    if let Some(detail) = detail {
        content = content.push(readable_text(detail).size(CAPTION_SIZE).style(|theme| {
            iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }
        }));
    }
    button(content)
        .on_press(message)
        .padding([6, 10])
        .width(Length::Fill)
        .style(context_menu_item_button_style())
        .into()
}

/// 下拉底部带标题的输入区（自定义后缀、自定义大小）。
fn menu_section<'a>(title: String, field: Element<'a, Message>) -> Element<'a, Message> {
    container(
        Column::new()
            .push(readable_text(title).size(CAPTION_SIZE).style(|theme| {
                iced::widget::text::Style {
                    color: Some(muted_text_color(theme)),
                }
            }))
            .push(field)
            .spacing(4),
    )
    .padding([6, 10])
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
#[path = "search_filter_bar_tests.rs"]
mod tests;
