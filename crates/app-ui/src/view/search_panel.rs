use std::fmt;

use file_search::{SearchFileKind, SearchMatchMode, SearchTextScope};
use iced::widget::{
    button, container, mouse_area, pick_list, responsive, row, scrollable, tooltip, Button, Column,
    Row, Space,
};
use iced::{Alignment, Element, Length};

use crate::anchored_popup::anchored_popup;
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    context_menu_style, enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction,
    list_panel_style, list_row_style, navigation_icon_button_style, navigation_text_input_style,
    path_suggestion_item_style, path_suggestions_style, selected_row_style,
    transparent_button_style,
};
use crate::file_entry_view::{file_entry_symbol_icon, FileEntryIconTone, FileEntryVisualState};
use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::icons::IconSymbol;
use crate::model::search::SearchFilterPresetState;
use crate::model::{
    Message, ScrollbarRegion, SearchDateField, SearchDatePreset, SearchDirectoryScope,
    SearchEntryTypePreset, SearchResultCompletion, SearchSizePreset, SEARCH_RESULT_TOTAL_LIMIT,
};
use crate::typography::{localized_text, readable_text};
use crate::virtual_range::{initial_virtual_range, virtual_range_for_viewport};

use super::option_controls::{
    inactive_segmented_choice_row, segmented_choice_button_style, segmented_choice_row,
    SegmentedChoice,
};
use super::{themed_icon, IconTone};

const SEARCH_INPUT_WIDTH: f32 = 140.0;
const WIDE_SEARCH_TOOLBAR_WIDTH: f32 = 1_000.0;
const MEDIUM_SEARCH_TOOLBAR_WIDTH: f32 = 560.0;
const SEARCH_DATE_FIELD_WIDTH: f32 = 122.0;
const SEARCH_DATE_PRESET_WIDTH: f32 = 138.0;
const SEARCH_SIZE_PRESET_WIDTH: f32 = 160.0;
const SEARCH_TEXT_SCOPE_WIDTH: f32 = 274.0;
const SEARCH_FILTER_BUTTON_HEIGHT: f32 = 30.0;
const SEARCH_HISTORY_PANEL_MAX_HEIGHT: f32 = 320.0;
const SEARCH_HISTORY_KEYWORD_MAX_CHARS: usize = 28;
pub(crate) const SEARCH_RESULT_ROW_HEIGHT: f32 = 78.0;
const SEARCH_RESULT_OVERSCAN_ROWS: usize = 12;
const SEARCH_RESULT_INITIAL_ROWS: usize = SEARCH_RESULT_OVERSCAN_ROWS * 2 + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchDateFieldOption(SearchDateField);

impl fmt::Display for SearchDateFieldOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&crate::localization::translate_current(self.0.label()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchDatePresetOption(SearchDatePreset);

impl fmt::Display for SearchDatePresetOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&crate::localization::translate_current(self.0.label()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchSizePresetOption(SearchSizePreset);

impl fmt::Display for SearchSizePresetOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&crate::localization::translate_current(self.0.label()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchToolbarDensity {
    Wide,
    Medium,
    Narrow,
}

impl SearchToolbarDensity {
    fn for_width(width: f32) -> Self {
        if width >= WIDE_SEARCH_TOOLBAR_WIDTH {
            Self::Wide
        } else if width >= MEDIUM_SEARCH_TOOLBAR_WIDTH {
            Self::Medium
        } else {
            Self::Narrow
        }
    }

    fn entry_type_columns(self) -> usize {
        match self {
            Self::Wide => 8,
            Self::Medium => 4,
            Self::Narrow => 2,
        }
    }
}

fn search_input_width() -> Length {
    Length::Fixed(SEARCH_INPUT_WIDTH)
}

pub(crate) fn search_input_id() -> iced::widget::Id {
    iced::widget::Id::from("search-input")
}

pub(super) fn search_input_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let input_value = browser
        .search_workspace
        .as_ref()
        .map(|workspace| workspace.input.as_str())
        .unwrap_or_default();
    let input = iced::widget::text_input(
        &crate::localization::translate_current("Search"),
        input_value,
    )
    .id(search_input_id())
    .on_input(Message::SearchInputChanged)
    .on_submit(Message::SearchSubmitted)
    .padding([8, 10])
    .size(15)
    .style(navigation_text_input_style)
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
                    .style(navigation_icon_button_style()),
                tooltip_label("Clear search text"),
                tooltip::Position::Bottom,
            )
        ]
        .spacing(4)
        .align_y(Alignment::Center)
    };
    let anchor = container(content.width(Length::Fill)).width(search_input_width());
    let popup = browser
        .search_history_interaction
        .popup_is_visible(&browser.user_config().search_history)
        .then(|| search_history_panel(browser));
    anchored_popup(anchor, popup)
}

fn search_history_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let mut history = Column::new()
        .push(
            container(localized_text("Recent searches").size(12))
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
        button(
            readable_text(crate::localization::translate_current(
                "Clear search history",
            ))
            .size(12),
        )
        .on_press(Message::SearchHistoryCleared)
        .padding([6, 8])
        .width(Length::Fill)
        .style(transparent_button_style()),
    );

    let region = ScrollbarRegion::SearchHistory;
    let visibility = browser.scrollbar_visibility_for(&region);
    let history = scrollable(smooth_scroll_content(history, region.clone()))
        .id(smooth_scroll_id(&region))
        .direction(enhanced_vertical_scrollbar_direction(visibility, 6.0))
        .style(enhanced_scrollbar_style(visibility))
        .on_scroll(scrollbar_on_scroll(region.clone(), |_| {
            Message::SearchHistoryScrolled
        }))
        .height(Length::Shrink);
    let history = enhanced_scrollbar(
        history,
        visibility,
        browser.scrollbar_viewport_for(&region),
        ScrollbarAxis::Vertical,
        6.0,
    );

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
    let select_keyword = button(
        readable_text(displayed_keyword)
            .size(13)
            .width(Length::Fill),
    )
    .on_press(Message::SearchHistoryKeywordSelected(keyword.to_owned()))
    .padding([6, 8])
    .width(Length::Fill)
    .style(transparent_button_style());
    let remove_keyword = tooltip(
        button(themed_icon(IconSymbol::Close, IconTone::Normal, 11.0))
            .on_press(Message::SearchHistoryKeywordRemoved(keyword.to_owned()))
            .padding([6, 7])
            .style(navigation_icon_button_style()),
        tooltip_label("Remove from search history"),
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

pub(super) fn search_results_view(browser: &FileBrowser) -> Element<'_, Message> {
    let Some(workspace) = browser.search_workspace.as_ref() else {
        return container(readable_text("")).into();
    };
    let mut rows = Column::new().spacing(0).width(Length::Fill);
    let range = if workspace.window.viewport_height > 0.0 {
        virtual_range_for_viewport(
            workspace.window.hits.len(),
            SEARCH_RESULT_ROW_HEIGHT,
            workspace.window.viewport_offset_y,
            workspace.window.viewport_height,
            SEARCH_RESULT_OVERSCAN_ROWS,
        )
    } else {
        initial_virtual_range(
            workspace.window.hits.len(),
            SEARCH_RESULT_ROW_HEIGHT,
            SEARCH_RESULT_INITIAL_ROWS,
        )
    };
    if range.before_height > 0.0 {
        rows = rows.push(Space::new().height(Length::Fixed(range.before_height)));
    }
    for (index, hit) in workspace.window.hits[range.start..range.end]
        .iter()
        .enumerate()
    {
        let row_index = range.start + index;
        rows = rows.push(search_result_row(
            hit.clone(),
            row_index,
            workspace.selection.is_selected(&hit.path),
            workspace.selection.focused_path() == Some(hit.path.as_path()),
            browser.file_entry_content_modifier(&hit.path),
        ));
    }
    if range.after_height > 0.0 {
        rows = rows.push(Space::new().height(Length::Fixed(range.after_height)));
    }
    if let Some(failure) = &workspace.window.failure {
        rows = rows.push(search_message(failure.clone()));
    }
    if workspace.window.is_loading {
        rows = rows.push(search_message("Searching..."));
    } else if workspace.window.hits.is_empty() && workspace.window.failure.is_none() {
        rows = rows.push(search_message("No search results"));
    }
    if !workspace.window.is_loading {
        if let Some(completion) = workspace.window.completion {
            rows = rows.push(search_completion_message(completion));
        }
        if workspace.window.truncated {
            rows = rows.push(search_message(format!(
                "Result limit reached ({}); the oldest results were dropped. Narrow the search to see more.",
                SEARCH_RESULT_TOTAL_LIMIT
            )));
        }
    }

    let region = ScrollbarRegion::SearchResults;
    let visibility = browser.scrollbar_visibility_for(&region);
    let results = scrollable(smooth_scroll_content(rows, region.clone()))
        .id(smooth_scroll_id(&region))
        .direction(enhanced_vertical_scrollbar_direction(visibility, 8.0))
        .style(enhanced_scrollbar_style(visibility))
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(scrollbar_on_scroll(region.clone(), |viewport| {
            let offset = viewport.absolute_offset();
            Message::SearchResultsScrolled {
                offset_y: offset.y,
                viewport_height: viewport.bounds().height,
            }
        }));
    let results = enhanced_scrollbar(
        results,
        visibility,
        browser.scrollbar_viewport_for(&region),
        ScrollbarAxis::Vertical,
        8.0,
    );

    let mut content = Column::new().push(search_workspace_toolbar(browser));
    if workspace.content_search_is_degraded() {
        content = content.push(search_content_degraded_notice());
    }
    if workspace.filters.match_mode == SearchMatchMode::Regex {
        content = content.push(search_regex_notice());
    }
    content
        .push(
            container(results)
                .padding([4, 6])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(list_panel_style),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn search_workspace_toolbar(browser: &FileBrowser) -> Element<'_, Message> {
    let workspace = browser
        .search_workspace
        .as_ref()
        .expect("search toolbar requires an active workspace");
    let root_label = if workspace.root.selected_scope() == SearchDirectoryScope::AllIndexedLocations
    {
        "Search in all indexed locations".to_owned()
    } else {
        format!(
            "Search in {}",
            format_middle_ellipsized_text(&workspace.root.path().to_string_lossy(), 72)
        )
    };
    let filters = workspace.filters.clone();
    let available_directory_scopes = workspace.root.available_scopes();
    let selected_directory_scope = workspace.root.selected_scope();
    let last_location_path = workspace
        .root
        .last_location_directory()
        .map(std::path::Path::to_path_buf);
    let selected = browser.active_search_selection().unwrap_or_default();

    responsive(move |viewport_size| {
        search_workspace_toolbar_layout(
            root_label.clone(),
            available_directory_scopes.clone(),
            selected_directory_scope,
            last_location_path.clone(),
            filters.clone(),
            selected.clone(),
            SearchToolbarDensity::for_width(viewport_size.width),
        )
    })
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

fn search_workspace_toolbar_layout(
    root_label: String,
    available_directory_scopes: Vec<SearchDirectoryScope>,
    selected_directory_scope: SearchDirectoryScope,
    last_location_path: Option<std::path::PathBuf>,
    filters: SearchFilterPresetState,
    selected: Vec<std::path::PathBuf>,
    density: SearchToolbarDensity,
) -> Element<'static, Message> {
    let header = row![
        themed_icon(IconSymbol::Search, IconTone::Normal, 15.0),
        localized_text(root_label).size(13).width(Length::Fill),
        tooltip(
            button(themed_icon(IconSymbol::Close, IconTone::Normal, 13.0))
                .on_press(Message::SearchWorkspaceClosed)
                .padding([6, 8])
                .style(navigation_icon_button_style()),
            tooltip_label("Close search"),
            tooltip::Position::Bottom,
        )
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let entry_types = search_entry_type_grid(&filters, density.entry_type_columns());
    let directory_scope = search_directory_scope_filter(
        &available_directory_scopes,
        selected_directory_scope,
        last_location_path,
    );
    let controls = search_filter_controls(&filters, density);
    let mut toolbar = Column::new()
        .push(header)
        .push(directory_scope)
        .push(entry_types)
        .push(controls)
        .spacing(6);
    // 自定义大小输入行跟在筛选控件区下方，随档位切回非 Custom 消失。
    if custom_size_input_is_expanded(&filters) {
        toolbar = toolbar.push(custom_size_input_row(&filters));
    }
    if !selected.is_empty() {
        toolbar = toolbar.push(search_selection_actions(selected));
    }

    container(toolbar)
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(list_panel_style)
        .into()
}

fn search_entry_type_grid(
    filters: &SearchFilterPresetState,
    column_count: usize,
) -> Element<'static, Message> {
    let mut rows = Column::new().spacing(5).width(Length::Fill);
    let chunk_count = SearchEntryTypePreset::COMMON.chunks(column_count).count();
    for (chunk_index, entry_types) in SearchEntryTypePreset::COMMON
        .chunks(column_count)
        .enumerate()
    {
        let mut type_row = Row::new().spacing(6).width(Length::Fill);
        for entry_type in entry_types {
            type_row = type_row.push(search_entry_type_button(*entry_type, filters));
        }
        // 自定义入口固定收尾：贴在类型网格最后一行末尾，避免独占一整行。
        if chunk_index + 1 == chunk_count {
            type_row = type_row.push(custom_extension_button(filters));
        }
        rows = rows.push(type_row);
    }
    if filters.custom_extensions_open {
        rows = rows.push(custom_extension_input_row(filters));
    }
    rows.into()
}

fn search_entry_type_button(
    entry_type: SearchEntryTypePreset,
    filters: &SearchFilterPresetState,
) -> Element<'static, Message> {
    let selected = filters.entry_type_is_selected(entry_type);
    search_filter_button(
        search_entry_type_icon(entry_type),
        if selected {
            IconTone::Selected
        } else {
            IconTone::Normal
        },
        crate::localization::translate_current(entry_type.label()),
        Message::SearchEntryTypeToggled(entry_type),
    )
    .width(Length::FillPortion(1))
    .style(segmented_choice_button_style(selected))
    .into()
}

fn custom_extension_button(filters: &SearchFilterPresetState) -> Element<'static, Message> {
    let selected = filters.custom_extensions_are_active();
    search_filter_button(
        IconSymbol::Plus,
        if selected {
            IconTone::Selected
        } else {
            IconTone::Normal
        },
        crate::localization::translate_current("Custom"),
        Message::SearchCustomExtensionsToggled,
    )
    .width(Length::FillPortion(1))
    .style(segmented_choice_button_style(selected))
    .into()
}

fn custom_extension_input_row(filters: &SearchFilterPresetState) -> Element<'static, Message> {
    container(
        iced::widget::text_input(
            &crate::localization::translate_current("e.g. pdf, docx"),
            &filters.custom_extensions,
        )
        .on_input(Message::SearchCustomExtensionsChanged)
        .padding([5, 8])
        .size(12)
        .style(navigation_text_input_style)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .into()
}

fn search_entry_type_icon(entry_type: SearchEntryTypePreset) -> IconSymbol {
    match entry_type {
        SearchEntryTypePreset::Spreadsheets => IconSymbol::Grid,
        SearchEntryTypePreset::Video => IconSymbol::Video,
        SearchEntryTypePreset::Images => IconSymbol::FileImage,
        SearchEntryTypePreset::Text => IconSymbol::FileText,
        SearchEntryTypePreset::Documents => IconSymbol::File,
        SearchEntryTypePreset::Folders => IconSymbol::Folder,
        SearchEntryTypePreset::Audio => IconSymbol::Music,
        SearchEntryTypePreset::Pdf => IconSymbol::FileText,
        SearchEntryTypePreset::Files => IconSymbol::File,
        SearchEntryTypePreset::Archives => IconSymbol::FileArchive,
        SearchEntryTypePreset::Links => IconSymbol::Link,
    }
}

fn search_filter_controls(
    filters: &SearchFilterPresetState,
    density: SearchToolbarDensity,
) -> Element<'static, Message> {
    match density {
        // 宽窗口一行排开：文本范围 | 日期字段 | 日期范围 | 大小 | 命令按钮。
        SearchToolbarDensity::Wide => {
            row![
                container(search_text_scope_filter(filters))
                    .width(Length::Fixed(SEARCH_TEXT_SCOPE_WIDTH)),
                search_date_field_filter(
                    filters.date_field,
                    Length::Fixed(SEARCH_DATE_FIELD_WIDTH),
                ),
                search_date_preset_filter(
                    filters.date_preset,
                    Length::Fixed(SEARCH_DATE_PRESET_WIDTH),
                ),
                search_size_preset_filter(
                    filters.size_preset,
                    Length::Fixed(SEARCH_SIZE_PRESET_WIDTH),
                ),
                search_filter_commands(filters),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        }
        // 中窗口：大小下拉紧跟日期行；命令按钮折到下一行。
        SearchToolbarDensity::Medium => Column::new()
            .push(search_text_scope_filter(filters))
            .push(
                row![
                    search_date_field_filter(
                        filters.date_field,
                        Length::Fixed(SEARCH_DATE_FIELD_WIDTH),
                    ),
                    search_date_preset_filter(
                        filters.date_preset,
                        Length::Fixed(SEARCH_DATE_PRESET_WIDTH),
                    ),
                    search_size_preset_filter(
                        filters.size_preset,
                        Length::Fixed(SEARCH_SIZE_PRESET_WIDTH),
                    ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .push(search_filter_commands(filters))
            .spacing(5)
            .into(),
        // 窄窗口：大小与日期系控件同段折行，任何密度不隐藏。
        SearchToolbarDensity::Narrow => Column::new()
            .push(search_text_scope_filter(filters))
            .push(
                row![
                    search_date_field_filter(filters.date_field, Length::FillPortion(1)),
                    search_date_preset_filter(filters.date_preset, Length::FillPortion(1)),
                    search_size_preset_filter(filters.size_preset, Length::FillPortion(1)),
                ]
                .spacing(6),
            )
            .push(search_filter_commands(filters))
            .spacing(5)
            .into(),
    }
}

fn search_directory_scope_filter(
    available_scopes: &[SearchDirectoryScope],
    selected: SearchDirectoryScope,
    last_location_path: Option<std::path::PathBuf>,
) -> Element<'static, Message> {
    segmented_choice_row(search_directory_scope_choices(
        available_scopes,
        selected,
        last_location_path,
    ))
}

fn search_directory_scope_choices(
    available_scopes: &[SearchDirectoryScope],
    selected: SearchDirectoryScope,
    last_location_path: Option<std::path::PathBuf>,
) -> Vec<SegmentedChoice> {
    available_scopes
        .iter()
        .copied()
        .map(|scope| SegmentedChoice {
            label: scope.label(),
            selected: scope == selected,
            message: Message::SearchDirectoryScopeSelected(scope),
            // "上次位置"档只给目录名，悬停补完整路径消歧。
            tooltip: if scope == SearchDirectoryScope::LastLocation {
                last_location_path
                    .as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
            } else {
                None
            },
        })
        .collect()
}

fn search_text_scope_filter(filters: &SearchFilterPresetState) -> Element<'static, Message> {
    let regex_mode = filters.match_mode == SearchMatchMode::Regex;
    // 正则只在名称上执行：置灰范围选择器并固定显示 Name only，与提交的查询语义一致。
    let name_content_selected =
        !regex_mode && filters.text_scope == SearchTextScope::NameAndContent;
    let name_only_selected = regex_mode || filters.text_scope == SearchTextScope::NameOnly;
    let choices = vec![
        SegmentedChoice {
            label: "Name & content",
            selected: name_content_selected,
            message: Message::SearchTextScopeSelected(SearchTextScope::NameAndContent),
            tooltip: None,
        },
        SegmentedChoice {
            label: "Name only",
            selected: name_only_selected,
            message: Message::SearchTextScopeSelected(SearchTextScope::NameOnly),
            tooltip: None,
        },
    ];
    if regex_mode {
        inactive_segmented_choice_row(choices)
    } else {
        segmented_choice_row(choices)
    }
}

fn search_date_field_filter(selected: SearchDateField, width: Length) -> Element<'static, Message> {
    pick_list(
        SearchDateField::ALL.map(SearchDateFieldOption),
        Some(SearchDateFieldOption(selected)),
        |selected| Message::SearchDateFieldSelected(selected.0),
    )
    .width(width)
    .text_size(12)
    .padding([5, 6])
    .into()
}

fn search_date_preset_filter(
    selected: SearchDatePreset,
    width: Length,
) -> Element<'static, Message> {
    pick_list(
        SearchDatePreset::ALL.map(SearchDatePresetOption),
        Some(SearchDatePresetOption(selected)),
        |selected| Message::SearchDatePresetSelected(selected.0),
    )
    .width(width)
    .text_size(12)
    .padding([5, 6])
    .into()
}

fn search_size_preset_filter(
    selected: SearchSizePreset,
    width: Length,
) -> Element<'static, Message> {
    pick_list(
        SearchSizePreset::ALL.map(SearchSizePresetOption),
        Some(SearchSizePresetOption(selected)),
        |selected| Message::SearchSizePresetSelected(selected.0),
    )
    .width(width)
    .text_size(12)
    .padding([5, 6])
    .into()
}

/// 自定义大小输入行只跟 Custom 档绑定：下拉本身是开关，无独立展开 bool。
fn custom_size_input_is_expanded(filters: &SearchFilterPresetState) -> bool {
    filters.size_preset == SearchSizePreset::Custom
}

/// 自定义大小输入行：仅在 size_preset == Custom 时展开在筛选控件区下方，
/// 风格对齐自定义后缀输入行；两侧输入共用 120ms 稳定化。
fn custom_size_input_row(filters: &SearchFilterPresetState) -> Element<'static, Message> {
    container(
        row![
            readable_text("Min size").size(12),
            iced::widget::text_input(
                &crate::localization::translate_current("e.g. 10MB"),
                &filters.custom_size_min,
            )
            .on_input(Message::SearchCustomSizeMinChanged)
            .padding([5, 8])
            .size(12)
            .style(navigation_text_input_style)
            .width(Length::Fill),
            readable_text("Max size").size(12),
            iced::widget::text_input(
                &crate::localization::translate_current("e.g. 2 GB"),
                &filters.custom_size_max,
            )
            .on_input(Message::SearchCustomSizeMaxChanged)
            .padding([5, 8])
            .size(12)
            .style(navigation_text_input_style)
            .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}

fn search_filter_commands(filters: &SearchFilterPresetState) -> Element<'static, Message> {
    let selected_more_type_count = filters.selected_more_type_count();
    let more_label = if selected_more_type_count == 0 {
        crate::localization::translate_current("More")
    } else {
        format!(
            "{} ({selected_more_type_count})",
            crate::localization::translate_current("More")
        )
    };
    let regex_mode = filters.match_mode == SearchMatchMode::Regex;
    let mut commands = row![
        search_filter_button(
            IconSymbol::Regex,
            if regex_mode {
                IconTone::Selected
            } else {
                IconTone::Normal
            },
            crate::localization::translate_current("Regex"),
            Message::SearchRegexToggled,
        )
        .style(segmented_choice_button_style(regex_mode)),
        search_filter_button(
            IconSymbol::Plus,
            if selected_more_type_count == 0 {
                IconTone::Normal
            } else {
                IconTone::Selected
            },
            more_label,
            Message::SearchEntryTypesMenuOpened,
        )
        .style(segmented_choice_button_style(selected_more_type_count > 0)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if !filters.is_default() {
        commands = commands.push(
            search_filter_button(
                IconSymbol::Close,
                IconTone::Normal,
                crate::localization::translate_current("Reset filters"),
                Message::SearchFiltersReset,
            )
            .style(segmented_choice_button_style(false)),
        );
    }
    commands.into()
}

fn search_filter_button(
    icon: IconSymbol,
    tone: IconTone,
    label: String,
    message: Message,
) -> Button<'static, Message> {
    button(
        row![themed_icon(icon, tone, 13.0), readable_text(label)]
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .on_press(message)
    .height(Length::Fixed(SEARCH_FILTER_BUTTON_HEIGHT))
    .padding([4, 8])
}

fn search_selection_actions(selected: Vec<std::path::PathBuf>) -> Element<'static, Message> {
    let mut actions = row![
        search_action_button(IconSymbol::Copy, "Copy", Message::CopySelected),
        search_action_button(IconSymbol::ArrowRight, "Cut", Message::MoveSelected),
        search_action_button(IconSymbol::Trash, "Trash", Message::TrashSelected),
        search_action_button(
            IconSymbol::TriangleAlert,
            "Delete",
            Message::SearchDeletePermanentlySelected,
        ),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if let [path] = selected.as_slice() {
        actions = actions.push(search_action_button(
            IconSymbol::FolderOpen,
            "Open folder",
            Message::SearchOpenContainingDirectory(path.clone()),
        ));
    }
    container(actions)
        .width(Length::Fill)
        .align_x(Alignment::End)
        .into()
}

fn search_action_button(
    icon: IconSymbol,
    label: &'static str,
    message: Message,
) -> Element<'static, Message> {
    tooltip(
        button(themed_icon(icon, IconTone::Normal, 13.0))
            .on_press(message)
            .width(Length::Fixed(30.0))
            .height(Length::Fixed(28.0))
            .padding([5, 7])
            .style(navigation_icon_button_style()),
        tooltip_label(label),
        tooltip::Position::Bottom,
    )
    .into()
}

fn search_content_degraded_notice() -> Element<'static, Message> {
    container(
        row![
            themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, 14.0),
            localized_text("Content indexing is unavailable; matching file names only.")
                .size(12)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([7, 10])
    .width(Length::Fill)
    .style(list_panel_style)
    .into()
}
fn search_regex_notice() -> Element<'static, Message> {
    container(
        localized_text("Regex mode matches file names only.")
            .size(12)
            .width(Length::Fill),
    )
    .padding([7, 10])
    .width(Length::Fill)
    .style(list_panel_style)
    .into()
}

fn search_completion_message(completion: SearchResultCompletion) -> Element<'static, Message> {
    match completion {
        SearchResultCompletion::Complete => container(readable_text("")).into(),
        SearchResultCompletion::MoreAvailable => search_message("Scroll to load more results"),
        SearchResultCompletion::Partial { inspected_entries } => search_message(format!(
            "Partial results after inspecting {inspected_entries} entries"
        )),
    }
}

fn search_message(
    message: impl crate::typography::ReadableTextContent,
) -> Element<'static, Message> {
    container(localized_text(message).size(14))
        .padding([14, 12])
        .width(Length::Fill)
        .style(list_row_style(0, 0))
        .into()
}

fn search_result_row(
    hit: file_search::SearchHit,
    row_index: usize,
    selected: bool,
    focused: bool,
    modifier: crate::model::FileEntryContentModifier,
) -> Element<'static, Message> {
    let metadata = format!(
        "{} · {}",
        format_file_size(hit.size),
        hit.path
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let visual_state = if selected || focused {
        FileEntryVisualState::Selected
    } else {
        FileEntryVisualState::Normal
    };
    let mut content = Column::new()
        .spacing(3)
        .push(readable_text(hit.display_name.clone()).size(15))
        .push(readable_text(metadata).size(12));
    if let Some(snippet) = hit.snippet.clone().filter(|snippet| !snippet.is_empty()) {
        content = content.push(readable_text(snippet).size(12));
    }
    let content = container(content).style(visual_state.content_style(modifier));
    let result_icon = match hit.kind {
        SearchFileKind::File => IconSymbol::File,
        SearchFileKind::Directory => IconSymbol::Folder,
        SearchFileKind::Symlink => IconSymbol::Link,
        SearchFileKind::Other => IconSymbol::File,
    };
    let path = hit.path;
    let content = row![
        file_entry_symbol_icon(result_icon, FileEntryIconTone::Normal, 18.0, modifier),
        content
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let row = container(content)
        .padding([8, 12])
        .width(Length::Fill)
        .height(Length::Fixed(SEARCH_RESULT_ROW_HEIGHT));
    let row = if selected || focused {
        row.style(selected_row_style)
    } else {
        row.style(list_row_style(0, row_index))
    };

    mouse_area(row)
        .on_press(Message::SearchResultPressed(path.clone()))
        .on_right_press(Message::SearchResultRightClicked(path))
        .into()
}

fn tooltip_label(label: &'static str) -> Element<'static, Message> {
    container(localized_text(label).size(11))
        .padding([5, 7])
        .style(context_menu_style)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_search_input_uses_compact_fixed_width() {
        let Length::Fixed(width) = search_input_width() else {
            panic!("navigation search input must not participate in Fill layout");
        };
        assert_eq!(width, 140.0);
    }

    #[test]
    fn directory_scope_choices_follow_model_availability() {
        let both = search_directory_scope_choices(
            &[
                SearchDirectoryScope::CurrentFolder,
                SearchDirectoryScope::Home,
            ],
            SearchDirectoryScope::CurrentFolder,
            None,
        );
        assert_eq!(both.len(), 2);
        assert_eq!(both[0].label, "Current folder");
        assert!(both[0].selected);
        assert_eq!(both[1].label, "Home");
        assert!(!both[1].selected);

        let home_only = search_directory_scope_choices(
            &[SearchDirectoryScope::Home],
            SearchDirectoryScope::Home,
            None,
        );
        assert_eq!(home_only.len(), 1);
        assert_eq!(home_only[0].label, "Home");
        assert!(home_only[0].selected);
    }

    #[test]
    fn last_location_scope_carries_path_tooltip_only() {
        let choices = search_directory_scope_choices(
            &[
                SearchDirectoryScope::CurrentFolder,
                SearchDirectoryScope::LastLocation,
                SearchDirectoryScope::Home,
                SearchDirectoryScope::AllIndexedLocations,
            ],
            SearchDirectoryScope::LastLocation,
            Some(std::path::PathBuf::from("/srv/downloads")),
        );

        assert_eq!(choices[0].tooltip, None);
        assert_eq!(choices[1].tooltip.as_deref(), Some("/srv/downloads"));
        assert_eq!(choices[2].tooltip, None);
        assert_eq!(choices[3].tooltip, None);
    }

    #[test]
    fn search_toolbar_density_keeps_stable_type_column_counts() {
        assert_eq!(
            SearchToolbarDensity::for_width(1_000.0).entry_type_columns(),
            8
        );
        assert_eq!(
            SearchToolbarDensity::for_width(700.0).entry_type_columns(),
            4
        );
        assert_eq!(
            SearchToolbarDensity::for_width(420.0).entry_type_columns(),
            2
        );
    }

    #[test]
    fn size_preset_choices_carry_unique_non_empty_labels() {
        let labels: Vec<String> = SearchSizePreset::ALL
            .iter()
            .map(|preset| SearchSizePresetOption(*preset).to_string())
            .collect();
        assert_eq!(labels.len(), 9);
        assert!(labels.iter().all(|label| !label.is_empty()));
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn custom_size_input_row_follows_the_custom_preset() {
        let mut filters = SearchFilterPresetState::default();
        assert!(!custom_size_input_is_expanded(&filters));

        filters.select_size_preset(SearchSizePreset::Custom);
        assert!(custom_size_input_is_expanded(&filters));

        // 切回任意档位：输入行随档位消失。
        filters.select_size_preset(SearchSizePreset::Any);
        assert!(!custom_size_input_is_expanded(&filters));
    }
}
