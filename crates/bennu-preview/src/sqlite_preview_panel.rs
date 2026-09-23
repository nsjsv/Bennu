//! SQLite 预览面板（表列表/数据网格/SQL 查询）：自 app-ui
//! view/sqlite_preview_panel.rs 下沉。标签行（segmented_choice_row，设置
//! 窗口共用词汇）与滚动接线、拖拽结束事件经参数由宿主注入，其余逐字节
//! 保真。

use iced::widget::{button, column, container, mouse_area, row, scrollable, text_input, Space};
use iced::{Background, Border, Element, Length};

use bennu_theme::measured_text::format_middle_ellipsized_text;
use bennu_theme::scrollbar::{
    enhanced_scrollbar, enhanced_scrollbar_both, ScrollbarAxis, ScrollbarViewport,
};
use bennu_theme::styles::{
    base_text_color, enhanced_both_scrollbar_direction, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, navigation_text_input_style, ScrollbarVisibility,
};
use bennu_theme::ui_colors;

use crate::engine::{SqlitePreviewState, SQLITE_DEFAULT_TABLES_WIDTH};
use crate::panel_text::{localized_text, readable_text};
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use crate::sqlite_preview::{
    SqlQueryOutcome, SqliteCellValue, SqliteDatabasePreview, SqlitePreviewMessage, SqlitePreviewTab,
};

const SQLITE_TABLE_NAME_MAX_CHARS: usize = 40;
const SQLITE_COLUMN_WIDTH: f32 = 170.0;
const SQLITE_CELL_MAX_CHARS: usize = 64;
const SQLITE_SCROLLBAR_WIDTH: f32 = 6.0;
const SQLITE_RESIZE_HANDLE_WIDTH: f32 = 6.0;

// ponytail: 200 行 × 每列直接渲染 widget，表列数极大（数百列）时才需要虚拟化。
pub fn sqlite_preview_panel<'a, Message>(
    preview: &'a SqliteDatabasePreview,
    state: Option<&'a SqlitePreviewState>,
    tables_visibility: ScrollbarVisibility,
    tables_viewport: Option<ScrollbarViewport>,
    data_visibility: ScrollbarVisibility,
    data_viewport: Option<ScrollbarViewport>,
    tabs: Element<'a, Message>,
    tables_scroll: ScrollRegionWiring<'a, Message>,
    data_scroll: ScrollRegionWiring<'a, Message>,
    on_resize_drag_finished: impl Fn() -> Message + 'a,
) -> Element<'a, Message>
where
    // enhanced_scrollbar 系（bennu-theme）要求 Message: 'static；滚动
    // 接线闭包的生命周期独立为 'a，宿主注入 'static 闭包即可覆盖。
    Message: 'static + Clone + From<PreviewMessage>,
{
    let active_tab = state
        .map(|state| state.active_tab)
        .unwrap_or(SqlitePreviewTab::Tables);
    let body = match active_tab {
        SqlitePreviewTab::Tables => tables_tab(
            preview,
            state,
            tables_visibility,
            tables_viewport,
            data_visibility,
            data_viewport,
            tables_scroll,
            data_scroll,
            on_resize_drag_finished,
        ),
        SqlitePreviewTab::Sql => sql_tab(state, data_visibility, data_viewport, data_scroll),
    };

    column![tabs, body]
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn tables_tab<'a, Message>(
    preview: &'a SqliteDatabasePreview,
    state: Option<&'a SqlitePreviewState>,
    tables_visibility: ScrollbarVisibility,
    tables_viewport: Option<ScrollbarViewport>,
    data_visibility: ScrollbarVisibility,
    data_viewport: Option<ScrollbarViewport>,
    tables_scroll: ScrollRegionWiring<'a, Message>,
    data_scroll: ScrollRegionWiring<'a, Message>,
    on_resize_drag_finished: impl Fn() -> Message + 'a,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    if preview.tables.is_empty() {
        return localized_text("This database has no tables")
            .size(14)
            .into();
    }

    // 直接切换：数据到达前保持空白，不显示加载占位。
    let data_area: Element<'a, Message> = match state.and_then(|state| state.table_data.as_ref()) {
        Some(data) => sqlite_grid(
            &data.columns,
            &data.rows,
            data.truncated,
            data_visibility,
            data_viewport,
            data_scroll,
        ),
        None => column![].width(Length::Fill).height(Length::Fill).into(),
    };

    // enhanced_scrollbar 的 Stack 外壳强制 Fill，必须用固定宽度容器包住表列表，
    // 否则拖动时 tables_width 的变化不会反映到布局。
    let tables_width = state
        .map(|state| state.tables_width)
        .unwrap_or(SQLITE_DEFAULT_TABLES_WIDTH);
    let tables_panel: Element<'a, Message> = container(sqlite_table_list(
        preview,
        state,
        tables_visibility,
        tables_viewport,
        tables_scroll,
    ))
    .width(Length::Fixed(tables_width))
    .height(Length::Fill)
    .into();

    row![
        tables_panel,
        sqlite_tables_resize_handle(on_resize_drag_finished),
        data_area
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn sqlite_tables_resize_handle<Message>(
    on_drag_finished: impl Fn() -> Message,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    // 与主窗口侧栏一致的拖动手柄：透明占位 + 横向调整光标。
    let handle = container(Space::new().width(Length::Fixed(SQLITE_RESIZE_HANDLE_WIDTH)))
        .height(Length::Fill);
    mouse_area(handle)
        .on_press(Message::from(PreviewMessage::SqliteTablesResizeStarted))
        .on_release(on_drag_finished())
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}

fn sqlite_table_list<'a, Message>(
    preview: &'a SqliteDatabasePreview,
    state: Option<&'a SqlitePreviewState>,
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let filter_value = state.map(|state| state.table_filter.as_str()).unwrap_or("");
    let filter = filter_value.to_lowercase();
    let filter_input = container(
        text_input("Filter tables", filter_value)
            .on_input(|value| {
                Message::from(PreviewMessage::SqlitePreview(
                    SqlitePreviewMessage::TableFilterChanged(value),
                ))
            })
            .padding([5, 8])
            .size(12)
            .width(Length::Fill)
            .style(navigation_text_input_style),
    )
    .padding([0, 6])
    .width(Length::Fill);

    let selected_table = state.and_then(|state| state.selected_table.as_deref());
    let mut list = column![].spacing(2);
    let mut matching_count = 0;
    for table in &preview.tables {
        if !filter.is_empty() && !table.name.to_lowercase().contains(&filter) {
            continue;
        }
        matching_count += 1;
        let selected = selected_table == Some(table.name.as_str());
        let label = format_middle_ellipsized_text(&table.name, SQLITE_TABLE_NAME_MAX_CHARS);
        let content = container(
            column![
                readable_text(label).size(13),
                readable_text(table.row_count.to_string()).size(11),
            ]
            .spacing(1),
        )
        .width(Length::Fill)
        .padding([5, 8]);
        let mut entry = button(content)
            .width(Length::Fill)
            .style(table_entry_style(selected));
        if !selected {
            entry = entry.on_press(Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TableSelected(table.name.clone()),
            )));
        }
        list = list.push(entry);
    }
    if matching_count == 0 {
        list = list.push(
            localized_text("No tables match")
                .size(12)
                .width(Length::Fill),
        );
    }

    let ScrollRegionWiring {
        smooth_scroll_wrap,
        scrollable_id,
        on_scroll,
    } = scroll;
    let scroller = scrollable(smooth_scroll_wrap(list.into()))
        .id(scrollable_id)
        .width(Length::Fill)
        .height(Length::Fill)
        .direction(enhanced_vertical_scrollbar_direction(
            visibility,
            SQLITE_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(visibility))
        .on_scroll(on_scroll);

    column![
        filter_input,
        enhanced_scrollbar(
            scroller,
            visibility,
            viewport,
            ScrollbarAxis::Vertical,
            SQLITE_SCROLLBAR_WIDTH,
        )
    ]
    .spacing(6)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn sql_tab<'a, Message>(
    state: Option<&'a SqlitePreviewState>,
    data_visibility: ScrollbarVisibility,
    data_viewport: Option<ScrollbarViewport>,
    data_scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let Some(state) = state else {
        return column![].into();
    };

    // 单行 SQL 输入：回车直接执行，与搜索框一致的交互。
    let editor_area = container(
        text_input("SQL query (Enter to run)", &state.sql_text)
            .on_input(|value| {
                Message::from(PreviewMessage::SqlitePreview(
                    SqlitePreviewMessage::SqlTextChanged(value),
                ))
            })
            .on_submit(Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::SqlSubmitted,
            )))
            .padding([5, 8])
            .size(13)
            .width(Length::Fill)
            .style(navigation_text_input_style),
    )
    .width(Length::Fill);

    // 查询期间保留旧结果，新结果到达后直接替换。
    let result: Element<'a, Message> = match &state.sql_result {
        Some(Ok(outcome)) => sql_result_area(outcome, data_visibility, data_viewport, data_scroll),
        Some(Err(error)) => readable_text(error.clone()).size(13).into(),
        None => column![].into(),
    };

    column![editor_area, result]
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn sql_result_area<'a, Message>(
    outcome: &SqlQueryOutcome,
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    if outcome.columns.is_empty() {
        return localized_text("Statement executed. No result rows.")
            .size(13)
            .into();
    }
    sqlite_grid(
        &outcome.columns,
        &outcome.rows,
        outcome.truncated,
        visibility,
        viewport,
        scroll,
    )
}

fn sqlite_grid<'a, Message>(
    columns: &[String],
    rows: &[Vec<SqliteCellValue>],
    truncated: bool,
    visibility: ScrollbarVisibility,
    viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let mut grid = column![grid_header_row(columns)].spacing(1);
    for values in rows {
        grid = grid.push(grid_data_row(values));
    }

    let ScrollRegionWiring {
        smooth_scroll_wrap,
        scrollable_id,
        on_scroll,
    } = scroll;
    let scroller = scrollable(smooth_scroll_wrap(grid.into()))
        .id(scrollable_id)
        .width(Length::Fill)
        .height(Length::Fill)
        .direction(enhanced_both_scrollbar_direction(
            visibility,
            SQLITE_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(visibility))
        .on_scroll(on_scroll);
    let scroller = enhanced_scrollbar_both(scroller, visibility, viewport, SQLITE_SCROLLBAR_WIDTH);

    if truncated {
        column![
            scroller,
            localized_text("Only the first 200 rows are shown.").size(11)
        ]
        .spacing(4)
        .height(Length::Fill)
        .into()
    } else {
        scroller.into()
    }
}

fn grid_header_row<Message: 'static>(columns: &[String]) -> Element<'static, Message> {
    let mut header = row![].spacing(1);
    for column_name in columns {
        header = header.push(
            container(readable_text(column_name.clone()).size(12))
                .width(Length::Fixed(SQLITE_COLUMN_WIDTH))
                .padding([5, 8])
                .style(grid_header_cell_style),
        );
    }
    header.into()
}

fn grid_data_row<Message: 'static>(values: &[SqliteCellValue]) -> Element<'static, Message> {
    let mut data_row = row![].spacing(1);
    for value in values {
        data_row = data_row.push(
            container(readable_text(cell_text(value)).size(13))
                .width(Length::Fixed(SQLITE_COLUMN_WIDTH))
                .padding([4, 8]),
        );
    }
    data_row.into()
}

fn cell_text(value: &SqliteCellValue) -> String {
    match value {
        SqliteCellValue::Null => "NULL".to_owned(),
        SqliteCellValue::Integer(value) => value.to_string(),
        SqliteCellValue::Real(value) => value.to_string(),
        SqliteCellValue::Text(value) => format_middle_ellipsized_text(value, SQLITE_CELL_MAX_CHARS),
        SqliteCellValue::Blob(size) => format!("<BLOB {size} bytes>"),
    }
}

fn grid_header_cell_style(theme: &iced::Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_low)),
        ..container::Style::default()
    }
}

/// 项目约定：列表条目不显示持续描边，反馈全部来自悬浮/按下/选中背景。
fn table_entry_style(
    selected: bool,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        let colors = ui_colors(theme);
        let background = if selected {
            Some(Background::Color(colors.primary_container))
        } else {
            match status {
                button::Status::Hovered => Some(Background::Color(colors.surface_container_high)),
                button::Status::Pressed => {
                    Some(Background::Color(colors.surface_container_highest))
                }
                _ => None,
            }
        };
        button::Style {
            background,
            text_color: if selected {
                colors.on_primary_container
            } else {
                base_text_color(theme)
            },
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}
