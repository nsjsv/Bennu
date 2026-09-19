use file_core::{
    DirectoryEntry, DirectoryMetadataAvailability, EntryMetadata, FileKind, SortDirection,
};
use iced::widget::{container, mouse_area, row, scrollable, text_input, Column, Row, Space, Stack};
use iced::{Alignment, Element, Length};

use crate::app::panes::{BrowserPaneView, DirectoryContentAvailability};
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content_with_shift, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, icon_svg_style,
    list_header_cell_style, list_header_reorder_indicator_style, list_header_style,
    list_panel_style, list_row_style, ListHeaderCellVisualState,
};
use crate::column_entry_bounds::track_column_entry_bounds;
use crate::config::ViewDensityLevel;
use crate::file_entry_view::{
    entry_text_input_style, entry_thumbnail_or_icon, FileEntryIconDensity, FileEntryIconTone,
    FileEntryVisualState,
};
use crate::formatting::format_system_time;
use crate::icons::rotated_chevron_right_view;
use crate::measured_middle_ellipsized_text::measured_middle_ellipsized_text;
use crate::model::{
    BrowserPaneId, DirectoryLoadingPlaceholderEntry, FilePropertiesPermissions, ListColumnConfig,
    ListColumnKind, Message, ScrollbarRegion,
};
use crate::typography::readable_text;
use crate::view::rename_input_id;

const LIST_ROW_TEXT_SIZE: u32 = 15;
const LIST_HEADER_TEXT_SIZE: u32 = 12;
const LIST_HEADER_SORT_ICON_SIZE: f32 = 12.0;
const LIST_CONTENT_PADDING: iced::Padding = iced::Padding {
    top: 0.0,
    right: 6.0,
    bottom: 4.0,
    left: 6.0,
};
pub(crate) const LIST_ROW_HEIGHT: f32 = 46.0;
pub(crate) const LIST_OVERSCAN_ROWS: usize = 16;
/// 分组标题行的 chrome 高度:与表头总高同族(24 单元 + 8 内边距),
/// 属面板 chrome 不随密度缩放;行流构建与渲染共用本常量保证同口径。
pub(crate) const LIST_GROUP_HEADER_HEIGHT: f32 = 32.0;

/// 列表条目几何在 100% 基础上按当前档位比例缩放；
/// 表头、列宽和面板外层留白不参与密度缩放。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ListGeometry {
    pub(crate) scale: f32,
    pub(crate) row_height: f32,
    pub(crate) text_size: f32,
    pub(crate) row_padding: iced::Padding,
    pub(crate) row_spacing: f32,
    pub(crate) indent_width: f32,
    pub(crate) toggle_width: f32,
    pub(crate) toggle_icon_size: f32,
    pub(crate) icon_density: FileEntryIconDensity,
}

impl ListGeometry {
    pub(crate) fn for_level(level: ViewDensityLevel) -> Self {
        let scale = level.scale();
        // 缩放尺寸一律取整到整数像素：行高等整数几何让整行对齐的 viewport
        // 运算保持精确，虚拟范围、键盘揭示与缩略图调度不会各舍入到不同行。
        let scaled = |base: f32| (base * scale).round();
        let row_padding_horizontal = scaled(LIST_ROW_PADDING[1] as f32);
        Self {
            scale,
            row_height: scaled(LIST_ROW_HEIGHT),
            text_size: scaled(LIST_ROW_TEXT_SIZE as f32),
            row_padding: iced::Padding {
                top: 0.0,
                right: row_padding_horizontal,
                bottom: 0.0,
                left: row_padding_horizontal,
            },
            row_spacing: scaled(LIST_ROW_SPACING as f32),
            indent_width: scaled(LIST_INDENT_WIDTH),
            toggle_width: scaled(LIST_TOGGLE_WIDTH),
            toggle_icon_size: scaled(LIST_TOGGLE_ICON_SIZE),
            icon_density: FileEntryIconDensity::List(level),
        }
    }
}

pub(crate) fn list_initial_rows(window_height: f32, level: ViewDensityLevel) -> usize {
    crate::virtual_range::initial_rows_for_height(
        window_height - LIST_HEADER_HEIGHT,
        ListGeometry::for_level(level).row_height,
        LIST_OVERSCAN_ROWS,
    )
}
const LIST_ROW_PADDING: [u16; 2] = [0, 8];
const LIST_HEADER_PADDING: [u16; 2] = [4, 8];
pub(crate) const LIST_ROW_SPACING: u32 = 6;
const LIST_INDENT_WIDTH: f32 = 18.0;
const LIST_TOGGLE_WIDTH: f32 = 18.0;
const LIST_TOGGLE_ICON_SIZE: f32 = 14.0;
const LIST_HEADER_CELL_HEIGHT: f32 = 24.0;
pub(crate) const LIST_HEADER_HEIGHT: f32 =
    LIST_HEADER_CELL_HEIGHT + LIST_HEADER_PADDING[0] as f32 * 2.0;
const LIST_HEADER_DROP_INDICATOR_WIDTH: f32 = 3.0;
pub(crate) const LIST_COLUMN_GAP_WIDTH: f32 = 5.0;
const LIST_COLUMN_RESIZE_DIVIDER_WIDTH: f32 = LIST_COLUMN_GAP_WIDTH;

pub(crate) fn list_browser_view<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    let geometry = ListGeometry::for_level(browser.user_config().list_view_density);
    let mut rows = Column::new().spacing(0).width(Length::Fill);
    rows = rows.push(list_header(browser, pane.id));

    // 分组索引栏数据与行流同源派生;Pending 阶段(加载占位)无组头。
    let mut rail_entries = Vec::new();
    if matches!(
        pane.current_directory_content(),
        DirectoryContentAvailability::Pending
    ) {
        if let Some(placeholder) = pane.directory_loading_placeholder {
            rows = rows.push(vertical_spacer(placeholder.before_height));
            // 列配置每次渲染收集一次，避免每行每帧重复分配 Vec。
            let visible_columns = list_visible_columns(browser);
            for placeholder_entry in &placeholder.entries {
                rows = rows.push(list_placeholder_entry_row(
                    browser,
                    &geometry,
                    &visible_columns,
                    placeholder_entry,
                ));
                if placeholder_entry.trailing_status_height > f32::EPSILON {
                    rows = rows.push(vertical_spacer(placeholder_entry.trailing_status_height));
                }
            }
            rows = rows.push(vertical_spacer(placeholder.after_height));
        }
    } else {
        // 占位行从操作队列派生,与真实条目合入同一条渲染流后再做虚拟
        // 范围切割,保证滚动数学与行渲染共用同一份行序列。
        let transfer_rows = crate::transfer_placeholder_view::build_list_transfer_rows(
            browser,
            pane,
            geometry.row_height,
        );
        // 索引栏条目从同一份行流派生,组头顶点偏移与渲染高度累计同口径。
        rail_entries = crate::transfer_placeholder_view::list_file_group_rail_entries(
            &transfer_rows,
            geometry.row_height,
        );
        let has_placeholders = transfer_rows.iter().any(|row| {
            matches!(
                row,
                crate::transfer_placeholder_view::ListTransferRow::Placeholder { .. }
            )
        });
        if pane.entries.is_empty() && !has_placeholders {
            rows = rows.push(list_message(if pane.is_trash_view {
                "Trash is empty"
            } else {
                "No items"
            }));
        } else {
            let range = pane
                .column_viewports
                .get(pane.current_dir)
                .map(|viewport| {
                    crate::transfer_placeholder_view::list_transfer_rows_range_for_viewport(
                        &transfer_rows,
                        geometry.row_height,
                        LIST_HEADER_HEIGHT,
                        viewport.offset_y,
                        viewport.height,
                        LIST_OVERSCAN_ROWS,
                    )
                })
                .unwrap_or_else(|| {
                    crate::transfer_placeholder_view::initial_list_transfer_rows_range(
                        &transfer_rows,
                        geometry.row_height,
                        list_initial_rows(
                            browser.main_window_height,
                            browser.user_config().list_view_density,
                        ),
                    )
                });
            rows = rows.push(vertical_spacer(range.before_height));

            // 列配置每次渲染收集一次，避免每行每帧重复分配 Vec。
            let visible_columns = list_visible_columns(browser);
            for row_index in range.start..range.end {
                let Some(row) = transfer_rows.get(row_index) else {
                    break;
                };
                match row {
                    crate::transfer_placeholder_view::ListTransferRow::Entry {
                        visible,
                        transfer,
                        stripe_index,
                    } => {
                        rows = rows.push(list_entry_row(
                            browser,
                            pane,
                            &geometry,
                            &visible_columns,
                            visible.entry,
                            transfer.as_ref(),
                            visible.depth,
                            *stripe_index,
                            visible.animation_progress,
                            crate::transfer_placeholder_view::selection_run_position_in_transfer_rows(
                                &transfer_rows,
                                row_index,
                                pane.selected_paths,
                            ),
                        ));
                    }
                    crate::transfer_placeholder_view::ListTransferRow::DirectoryStatusRow {
                        message,
                        depth,
                        height,
                        stripe_index,
                    } => {
                        rows = rows.push(list_directory_status_row(
                            message,
                            *depth,
                            *stripe_index,
                            *height,
                            &geometry,
                        ));
                    }
                    crate::transfer_placeholder_view::ListTransferRow::Placeholder {
                        placeholder,
                        stripe_index,
                    } => {
                        rows = rows.push(
                            crate::transfer_placeholder_view::transfer_placeholder_list_row(
                                placeholder,
                                &geometry,
                                &visible_columns,
                                *stripe_index,
                            ),
                        );
                    }
                    crate::transfer_placeholder_view::ListTransferRow::GroupHeader {
                        title,
                        count,
                        height,
                        ..
                    } => {
                        rows = rows.push(list_group_header_row(title, *count, *height, &geometry));
                    }
                }
            }
            rows = rows.push(vertical_spacer(range.after_height));
        }
    }
    let scrollbar_region = ScrollbarRegion::PaneList(pane.id);
    let scrollbar_visibility = browser.scrollbar_visibility_for(&scrollbar_region);
    let list_scroll = scrollable(smooth_scroll_content_with_shift(
        rows,
        scrollbar_region.clone(),
        browser.smooth_scroll_shift_pressed(),
    ))
    .id(smooth_scroll_id(&scrollbar_region))
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        8.0,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .width(Length::Fill)
    .height(Length::Fill)
    .on_scroll(scrollbar_on_scroll(
        scrollbar_region.clone(),
        move |viewport: scrollable::Viewport| {
            let offset = viewport.absolute_offset();
            let bounds = viewport.bounds();
            Message::ListScrolled(pane.id, offset.y, bounds)
        },
    ));
    let list_scroll = enhanced_scrollbar(
        list_scroll,
        scrollbar_visibility,
        browser.scrollbar_viewport_for(&scrollbar_region),
        ScrollbarAxis::Vertical,
        8.0,
    );

    let pane_surface: Element<'a, Message> = container(list_scroll)
        .padding(LIST_CONTENT_PADDING)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(list_panel_style)
        .into();
    // 索引栏叠层:分组开启且组数 ≥2 时挂在滚动区之上;当前可视区顶部
    // 所在组在渲染期由派生 offsets 与视口偏移推得,不占持久状态。栏右
    // 缘自带避开滚动条 thumb 的间隙,不会遮挡 overlay 滚动条。
    let pane_surface: Element<'a, Message> = if crate::model::file_group_rail_visible(&rail_entries)
    {
        let viewport_position = pane
            .column_viewports
            .get(pane.current_dir)
            .map(|viewport| viewport.offset_y)
            .unwrap_or(0.0)
            - LIST_HEADER_HEIGHT;
        let active_group = crate::model::active_file_group_index(&rail_entries, viewport_position);
        Stack::with_children([
            pane_surface,
            crate::file_grouping_rail::file_grouping_rail_view(
                &rail_entries,
                active_group,
                pane.id,
            ),
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        pane_surface
    };

    mouse_area(pane_surface)
        .on_press(Message::BlankAreaPressed(pane.id))
        .on_release(Message::DropTargetReleased(
            pane.id,
            pane.current_dir.clone(),
        ))
        .on_right_press(Message::BlankAreaRightClicked(
            pane.id,
            pane.current_dir.clone(),
        ))
        .on_enter(Message::ColumnBrowserCursorEntered(pane.id))
        .on_exit(Message::ColumnBrowserCursorExited(pane.id))
        .into()
}

fn vertical_spacer(height: f32) -> Element<'static, Message> {
    Space::new().height(Length::Fixed(height.max(0.0))).into()
}

fn list_header<'a>(browser: &'a FileBrowser, pane_id: BrowserPaneId) -> Element<'a, Message> {
    let visible_columns = browser
        .user_config()
        .list_view_preferences
        .visible_columns()
        .collect::<Vec<_>>();
    let hovered_column = browser.hovered_list_header_column(pane_id);
    let dragged_column = browser.list_column_being_reordered();
    let drop_target = browser.list_column_reorder_insertion_target();
    let sort = browser.user_config().list_view_preferences.sort();
    let mut header = Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for (index, column) in visible_columns.iter().enumerate() {
        if let Some(previous) = index
            .checked_sub(1)
            .and_then(|previous| visible_columns.get(previous))
        {
            header = header.push(list_column_resize_divider(pane_id, previous.kind));
        }
        let sort_direction = column
            .kind
            .sort_field()
            .filter(|field| *field == sort.field)
            .map(|_| sort.direction);
        let visual_state = if drop_target == Some(column.kind) {
            ListHeaderCellVisualState::DropTarget
        } else if dragged_column == Some(column.kind) {
            ListHeaderCellVisualState::Dragged
        } else if hovered_column == Some(column.kind) {
            ListHeaderCellVisualState::Hovered
        } else {
            ListHeaderCellVisualState::Idle
        };
        header = header.push(header_cell(pane_id, column, sort_direction, visual_state));
    }
    if let Some(last_column) = visible_columns.last() {
        header = header.push(list_column_resize_divider(pane_id, last_column.kind));
    }

    mouse_area(
        container(header)
            .padding(LIST_HEADER_PADDING)
            .width(Length::Fill)
            .style(list_header_style),
    )
    .on_right_press(Message::ListHeaderRightClicked(pane_id))
    .into()
}

fn header_cell(
    pane_id: BrowserPaneId,
    column: &ListColumnConfig,
    sort_direction: Option<SortDirection>,
    visual_state: ListHeaderCellVisualState,
) -> Element<'static, Message> {
    let content: Element<'static, Message> = if let Some(direction) = sort_direction {
        row![
            readable_text(column.kind.label()).size(LIST_HEADER_TEXT_SIZE),
            list_sort_direction_indicator(direction),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    } else {
        readable_text(column.kind.label())
            .size(LIST_HEADER_TEXT_SIZE)
            .into()
    };
    let mut layers = Stack::new()
        .width(list_column_width(column.width))
        .height(Length::Fixed(LIST_HEADER_CELL_HEIGHT));
    layers = layers.push(
        container(content)
            .padding([0, 6])
            .center_y(Length::Fill)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(list_header_cell_style(visual_state)),
    );
    if visual_state == ListHeaderCellVisualState::DropTarget {
        layers = layers.push(list_header_drop_indicator());
    }

    mouse_area(
        container(layers)
            .center_y(Length::Fixed(LIST_HEADER_CELL_HEIGHT))
            .width(list_column_width(column.width))
            .height(Length::Fixed(LIST_HEADER_CELL_HEIGHT)),
    )
    .on_press(Message::ListColumnReorderStarted(pane_id, column.kind))
    .on_enter(Message::ListHeaderColumnEntered(pane_id, column.kind))
    .on_exit(Message::ListHeaderColumnExited(pane_id, column.kind))
    .on_release(Message::DragSelectionFinished)
    .interaction(iced::mouse::Interaction::Grab)
    .into()
}

fn list_sort_direction_indicator(direction: SortDirection) -> Element<'static, Message> {
    let rotation = match direction {
        SortDirection::Ascending => -90.0,
        SortDirection::Descending => 90.0,
    };
    rotated_chevron_right_view(rotation, LIST_HEADER_SORT_ICON_SIZE)
        .style(icon_svg_style())
        .into()
}

fn list_header_drop_indicator() -> Element<'static, Message> {
    row![
        container(Space::new())
            .width(Length::Fixed(LIST_HEADER_DROP_INDICATOR_WIDTH))
            .height(Length::Fill)
            .style(list_header_reorder_indicator_style),
        Space::new().width(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn list_column_resize_divider(
    pane_id: BrowserPaneId,
    column: ListColumnKind,
) -> Element<'static, Message> {
    let divider = Space::new()
        .width(Length::Fixed(LIST_COLUMN_RESIZE_DIVIDER_WIDTH))
        .height(Length::Fill);

    mouse_area(divider)
        .on_press(Message::ListColumnResizeStarted(pane_id, column))
        .on_release(Message::DragSelectionFinished)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}

fn list_message(message: &'static str) -> Element<'static, Message> {
    container(readable_text(message).size(LIST_ROW_TEXT_SIZE))
        .padding(LIST_ROW_PADDING)
        .width(Length::Fill)
        .into()
}

/// 分组标题行:跨全宽纯静态容器,不包 mouse_area、不发消息——天然
/// 无 hover/选中态、不响应条目右键菜单,数量直接拼接数字。
/// 标题是构造时本地化的成品 String,readable_text 的 String 分支不查
/// 词条、只负责文本整形(中文标题需要 advanced shaping)。
fn list_group_header_row(
    title: &str,
    count: usize,
    height: f32,
    geometry: &ListGeometry,
) -> Element<'static, Message> {
    container(
        row![
            readable_text(title.to_owned()).size(LIST_HEADER_TEXT_SIZE),
            readable_text(format!("({count})")).size(LIST_HEADER_TEXT_SIZE),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding(geometry.row_padding)
    .height(Length::Fixed(height))
    .center_y(Length::Fixed(height))
    .width(Length::Fill)
    .style(crate::appearance::group_header_style)
    .into()
}

fn list_directory_status_row(
    message: &'static str,
    depth: usize,
    stripe_index: usize,
    height: f32,
    geometry: &ListGeometry,
) -> Element<'static, Message> {
    // 状态行的缩进与名称列的组合槽位同源，整体随档位比例缩放。
    let indent_width =
        (depth as f32 * LIST_INDENT_WIDTH + LIST_TOGGLE_WIDTH + 26.0) * geometry.scale;
    container(
        container(row![
            Space::new().width(Length::Fixed(indent_width)),
            readable_text(message).size(geometry.text_size),
        ])
        .height(Length::Fixed(geometry.row_height))
        .center_y(Length::Fixed(geometry.row_height))
        .padding(geometry.row_padding)
        .width(Length::Fill)
        .style(list_row_style(depth, stripe_index)),
    )
    .height(Length::Fixed(height))
    .clip(true)
    .into()
}

fn list_entry_row<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    geometry: &ListGeometry,
    visible_columns: &[ListColumnConfig],
    entry: &DirectoryEntry,
    transfer: Option<&crate::transfer_placeholders::TransferPlaceholder>,
    depth: usize,
    stripe_index: usize,
    animation_progress: f32,
    selection_run_position: Option<crate::file_entry_presentation::SelectionRunPosition>,
) -> Element<'a, Message> {
    let visual_state = FileEntryVisualState::from_entry_context(pane, &entry.path, false);
    let modifier = browser.file_entry_content_modifier(&entry.path);
    let icon_tone = visual_state.icon_tone();
    let row_content = container(list_entry_cells(
        browser,
        pane,
        geometry,
        visible_columns,
        entry,
        transfer,
        depth,
        icon_tone,
    ))
    .width(Length::Fill)
    .style(visual_state.content_style(modifier));

    let row_container = container(row_content)
        .padding(geometry.row_padding)
        .height(Length::Fixed(geometry.row_height))
        .center_y(Length::Fixed(geometry.row_height))
        .width(Length::Fill);
    let row_container =
        if let Some(style) = visual_state.row_style_for_selection_run(selection_run_position) {
            row_container.style(style)
        } else {
            // 条纹序号来自行流构建期的目录段计数器,与流绝对下标解耦:
            // 组头/状态行/占位行插拔不再翻转后续条目的条纹相位。
            row_container.style(list_row_style(depth, stripe_index))
        };

    let row_area = mouse_area(row_container)
        .on_enter(Message::EntryHovered(pane.id, entry.path.clone()))
        .on_exit(Message::EntryHoverCleared(pane.id, entry.path.clone()))
        .on_press(Message::FlatEntryClicked(pane.id, entry.path.clone()))
        .on_release(Message::EntryReleased(pane.id, entry.path.clone()))
        .on_right_press(Message::EntryRightClicked(pane.id, entry.path.clone()))
        .interaction(iced::mouse::Interaction::Pointer);

    let row_area = if entry.kind == FileKind::Directory && !pane.is_trash_view {
        row_area.on_middle_press(Message::OpenDirectoryFromMiddleClick(
            pane.id,
            entry.path.clone(),
        ))
    } else {
        row_area
    };

    let animated_row = container(row_area)
        .height(Length::Fixed(
            geometry.row_height * animation_progress.clamp(0.0, 1.0),
        ))
        .clip(true);
    track_column_entry_bounds(animated_row, pane.id, entry.path.clone())
}

fn list_placeholder_entry_row<'a>(
    browser: &'a FileBrowser,
    geometry: &ListGeometry,
    visible_columns: &[ListColumnConfig],
    placeholder: &'a DirectoryLoadingPlaceholderEntry,
) -> Element<'a, Message> {
    let entry = &placeholder.entry;
    let modifier = browser.file_entry_content_modifier(&entry.path);
    let row_content = container(list_placeholder_entry_cells(
        browser,
        geometry,
        entry,
        placeholder.depth,
        visible_columns,
    ))
    .width(Length::Fill)
    .style(FileEntryVisualState::Normal.content_style(modifier));

    container(
        container(row_content)
            .padding(geometry.row_padding)
            .height(Length::Fixed(geometry.row_height))
            .center_y(Length::Fixed(geometry.row_height))
            .width(Length::Fill)
            .style(list_row_style(placeholder.depth, placeholder.row_index)),
    )
    .height(Length::Fixed(
        geometry.row_height * placeholder.animation_progress.clamp(0.0, 1.0),
    ))
    .clip(true)
    .into()
}

fn list_entry_cells<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    geometry: &ListGeometry,
    visible_columns: &[ListColumnConfig],
    entry: &DirectoryEntry,
    transfer: Option<&crate::transfer_placeholders::TransferPlaceholder>,
    depth: usize,
    icon_tone: FileEntryIconTone,
) -> Row<'a, Message> {
    // 元数据解析每行一次，避免随列数重复查找。
    let metadata = pane.metadata_for_entry(entry);
    let mut row_content = Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for (index, column) in visible_columns.iter().enumerate() {
        if index > 0 {
            row_content = row_content.push(list_column_gap());
        }
        row_content = row_content.push(list_entry_cell(
            browser, pane, geometry, entry, transfer, depth, icon_tone, &metadata, column,
        ));
    }
    row_content.push(list_column_gap())
}

fn list_visible_columns(browser: &FileBrowser) -> Vec<ListColumnConfig> {
    browser
        .user_config()
        .list_view_preferences
        .visible_columns()
        .cloned()
        .collect()
}

fn list_placeholder_entry_cells<'a>(
    browser: &'a FileBrowser,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
    depth: usize,
    visible_columns: &[ListColumnConfig],
) -> Row<'a, Message> {
    let mut row_content = Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for (index, column) in visible_columns.iter().enumerate() {
        if index > 0 {
            row_content = row_content.push(list_column_gap());
        }
        row_content = row_content.push(list_placeholder_entry_cell(
            browser, geometry, entry, depth, column,
        ));
    }
    row_content.push(list_column_gap())
}

fn list_entry_cell<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
    transfer: Option<&crate::transfer_placeholders::TransferPlaceholder>,
    depth: usize,
    icon_tone: FileEntryIconTone,
    metadata: &EntryMetadata,
    column: &ListColumnConfig,
) -> Element<'a, Message> {
    match column.kind {
        ListColumnKind::Name => list_name_cell(
            browser,
            pane,
            geometry,
            entry,
            transfer,
            depth,
            icon_tone,
            column.width,
        ),
        ListColumnKind::Modified => text_cell(modified_text(&metadata), geometry, column.width),
        ListColumnKind::Size => text_cell(
            browser.list_directory_size_text(entry, &metadata),
            geometry,
            column.width,
        ),
        ListColumnKind::Kind => text_cell(kind_text(entry), geometry, column.width),
        ListColumnKind::Owner => text_cell(owner_text(&metadata), geometry, column.width),
        ListColumnKind::Group => text_cell(group_text(&metadata), geometry, column.width),
        ListColumnKind::Permissions => {
            text_cell(permissions_text(&metadata), geometry, column.width)
        }
        ListColumnKind::Accessed => text_cell(accessed_text(&metadata), geometry, column.width),
        ListColumnKind::Created => text_cell(created_text(&metadata), geometry, column.width),
    }
}

fn list_placeholder_entry_cell<'a>(
    browser: &'a FileBrowser,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
    depth: usize,
    column: &ListColumnConfig,
) -> Element<'a, Message> {
    match column.kind {
        ListColumnKind::Name => {
            list_placeholder_name_cell(browser, geometry, entry, depth, column.width)
        }
        ListColumnKind::Modified => {
            text_cell(modified_text(&entry.metadata), geometry, column.width)
        }
        ListColumnKind::Size => text_cell(
            browser.list_directory_size_text(entry, &entry.metadata),
            geometry,
            column.width,
        ),
        ListColumnKind::Kind => text_cell(kind_text(entry), geometry, column.width),
        ListColumnKind::Owner => text_cell(owner_text(&entry.metadata), geometry, column.width),
        ListColumnKind::Group => text_cell(group_text(&entry.metadata), geometry, column.width),
        ListColumnKind::Permissions => {
            text_cell(permissions_text(&entry.metadata), geometry, column.width)
        }
        ListColumnKind::Accessed => {
            text_cell(accessed_text(&entry.metadata), geometry, column.width)
        }
        ListColumnKind::Created => text_cell(created_text(&entry.metadata), geometry, column.width),
    }
}

fn list_column_gap() -> Element<'static, Message> {
    Space::new()
        .width(Length::Fixed(LIST_COLUMN_RESIZE_DIVIDER_WIDTH))
        .into()
}

fn list_column_width(width: f32) -> Length {
    Length::FillPortion(width.round().clamp(1.0, u16::MAX as f32) as u16)
}

fn list_placeholder_name_cell<'a>(
    browser: &'a FileBrowser,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
    depth: usize,
    width: f32,
) -> Element<'a, Message> {
    row![
        Space::new().width(Length::Fixed(depth as f32 * geometry.indent_width)),
        Space::new().width(Length::Fixed(geometry.toggle_width)),
        entry_thumbnail_or_icon(
            browser,
            entry,
            FileEntryIconTone::Normal,
            geometry.icon_density
        ),
        measured_middle_ellipsized_text(
            entry.name().to_string_lossy().into_owned(),
            geometry.text_size as u32,
        )
    ]
    .spacing(geometry.row_spacing)
    .align_y(Alignment::Center)
    .width(list_column_width(width))
    .into()
}

fn list_name_cell<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
    transfer: Option<&crate::transfer_placeholders::TransferPlaceholder>,
    depth: usize,
    icon_tone: FileEntryIconTone,
    width: f32,
) -> Element<'a, Message> {
    let modifier = browser.file_entry_content_modifier(&entry.path);
    let indent = Space::new().width(Length::Fixed(depth as f32 * geometry.indent_width));
    let toggle = list_directory_toggle(pane, geometry, entry);
    let name: Element<'a, Message> = if pane.renaming == Some(&entry.path) {
        text_input(
            &crate::localization::translate_current("File name"),
            pane.rename_input,
        )
        .id(rename_input_id())
        .on_input(Message::RenameInputChanged)
        .on_submit(Message::RenameSelected)
        .style(entry_text_input_style(modifier))
        .width(Length::Fill)
        .into()
    } else {
        measured_middle_ellipsized_text(
            entry.name().to_string_lossy().into_owned(),
            geometry.text_size as u32,
        )
    };

    row![
        indent,
        toggle,
        crate::transfer_placeholder_view::entry_icon_with_transfer(
            entry_thumbnail_or_icon(browser, entry, icon_tone, geometry.icon_density),
            transfer,
            geometry.icon_density.thumbnail_size(),
        ),
        name
    ]
    .spacing(geometry.row_spacing)
    .align_y(Alignment::Center)
    .width(list_column_width(width))
    .into()
}

fn list_directory_toggle<'a>(
    pane: BrowserPaneView<'a>,
    geometry: &ListGeometry,
    entry: &DirectoryEntry,
) -> Element<'a, Message> {
    if entry.kind != FileKind::Directory || pane.is_trash_view {
        return Space::new()
            .width(Length::Fixed(geometry.toggle_width))
            .into();
    }

    let rotation = pane
        .expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded)
        .map(|expanded| 90.0 * expanded.animation_progress.clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let icon =
        rotated_chevron_right_view(rotation, geometry.toggle_icon_size).style(icon_svg_style());
    mouse_area(
        container(icon)
            .center_x(Length::Fixed(geometry.toggle_width))
            .center_y(Length::Fixed(geometry.toggle_width)),
    )
    .on_press(Message::ListDirectoryToggled(pane.id, entry.path.clone()))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}
fn kind_text(entry: &DirectoryEntry) -> std::borrow::Cow<'static, str> {
    let kind = match entry.kind {
        FileKind::Directory => "Folder",
        FileKind::File => "File",
        FileKind::Symlink if entry.is_broken_symlink => "Broken Link",
        FileKind::Symlink => "Link",
        FileKind::Other => "Other",
    };
    // 直接借用翻译结果：英文原文零分配，中文仅一次小分配。
    crate::localization::translate(crate::localization::current_language(), kind)
}
fn text_cell(
    text: impl crate::typography::ReadableTextContent,
    geometry: &ListGeometry,
    width: f32,
) -> Element<'static, Message> {
    container(readable_text(text).size(geometry.text_size))
        .width(list_column_width(width))
        .clip(true)
        .into()
}

fn modified_text(metadata: &EntryMetadata) -> String {
    if metadata.filesystem_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    timestamp_text(metadata.modified)
}

fn owner_text(metadata: &EntryMetadata) -> String {
    if metadata.identity_names_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    metadata
        .owner_name
        .clone()
        .unwrap_or_else(|| "-".to_owned())
}

fn group_text(metadata: &EntryMetadata) -> String {
    if metadata.identity_names_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    metadata
        .group_name
        .clone()
        .unwrap_or_else(|| "-".to_owned())
}

fn permissions_text(metadata: &EntryMetadata) -> String {
    if metadata.filesystem_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    metadata
        .permissions_mode
        .map(FilePropertiesPermissions::from_mode)
        .map(|permissions| {
            format!(
                "{} ({})",
                permissions.symbolic_string(),
                permissions.octal_string()
            )
        })
        .unwrap_or_else(|| "-".to_owned())
}

fn accessed_text(metadata: &EntryMetadata) -> String {
    if metadata.filesystem_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    timestamp_text(metadata.accessed)
}

fn created_text(metadata: &EntryMetadata) -> String {
    if metadata.filesystem_availability != DirectoryMetadataAvailability::Complete {
        return "-".to_owned();
    }
    timestamp_text(metadata.created)
}

fn timestamp_text(value: Option<std::time::SystemTime>) -> String {
    value
        .map(format_system_time)
        .unwrap_or_else(|| "-".to_owned())
}

#[cfg(test)]
#[path = "list_view/directory_status_tests.rs"]
mod directory_status_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use file_core::{DirectoryEntry, EntryMetadata, FileKind};
    use std::path::PathBuf;

    #[test]
    fn list_column_width_uses_proportional_layout_weight() {
        assert_eq!(list_column_width(160.0), Length::FillPortion(160));
        assert_eq!(list_column_width(0.0), Length::FillPortion(1));
    }

    #[test]
    fn list_header_spacing_balances_vertical_whitespace() {
        assert_eq!(
            LIST_CONTENT_PADDING,
            iced::Padding {
                top: 0.0,
                right: 6.0,
                bottom: 4.0,
                left: 6.0,
            }
        );
        assert_eq!(LIST_HEADER_PADDING[0], 4);
    }

    fn test_entry(metadata: EntryMetadata) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from("/workspace/report.txt"),
            FileKind::File,
            metadata,
            false,
            false,
            false,
        )
    }

    #[test]
    fn new_metadata_columns_render_placeholder_when_metadata_is_missing() {
        let entry = test_entry(EntryMetadata::default());

        assert_eq!(owner_text(&entry.metadata), "-");
        assert_eq!(group_text(&entry.metadata), "-");
        assert_eq!(permissions_text(&entry.metadata), "-");
        assert_eq!(accessed_text(&entry.metadata), "-");
        assert_eq!(created_text(&entry.metadata), "-");
    }

    #[test]
    fn permissions_column_formats_symbolic_and_octal_mode() {
        let entry = test_entry(EntryMetadata {
            permissions_mode: Some(0o755),
            ..EntryMetadata::default()
        });

        assert_eq!(permissions_text(&entry.metadata), "rwxr-xr-x (0755)");
    }

    #[test]
    fn list_initial_rows_cover_full_window_without_scroll() {
        let default_level = crate::config::ViewDensityLevel::DEFAULT;
        let range = crate::visible_entries::initial_list_entry_range(
            &(0..34)
                .map(|index| {
                    DirectoryEntry::new(
                        std::path::PathBuf::from(format!("/workspace/item-{index:02}")),
                        FileKind::File,
                        EntryMetadata::default(),
                        false,
                        false,
                        false,
                    )
                })
                .collect::<Vec<_>>(),
            &std::collections::HashMap::new(),
            LIST_ROW_HEIGHT,
            list_initial_rows(900.0, default_level),
        );
        assert_eq!(range.end, 34);

        let rows = list_initial_rows(900.0, default_level);
        assert!(rows as f32 * LIST_ROW_HEIGHT >= 900.0 - LIST_HEADER_HEIGHT);
    }

    #[test]
    fn list_geometry_scales_entry_dimensions_and_keeps_header_fixed() {
        let default_geometry = ListGeometry::for_level(crate::config::ViewDensityLevel::DEFAULT);
        assert_eq!(default_geometry.row_height, LIST_ROW_HEIGHT);
        assert_eq!(default_geometry.text_size, LIST_ROW_TEXT_SIZE as f32);
        assert_eq!(default_geometry.row_spacing, LIST_ROW_SPACING as f32);
        assert_eq!(default_geometry.indent_width, LIST_INDENT_WIDTH);
        assert_eq!(default_geometry.toggle_width, LIST_TOGGLE_WIDTH);
        assert_eq!(default_geometry.toggle_icon_size, LIST_TOGGLE_ICON_SIZE);
        assert_eq!(
            default_geometry.row_padding,
            iced::Padding {
                top: 0.0,
                right: LIST_ROW_PADDING[1] as f32,
                bottom: 0.0,
                left: LIST_ROW_PADDING[1] as f32,
            }
        );

        let level = crate::config::ViewDensityLevel::from_index(4);
        let scale = level.scale();
        let geometry = ListGeometry::for_level(level);
        // 缩放尺寸按设计四舍五入到整数像素。
        assert_eq!(geometry.row_height, (LIST_ROW_HEIGHT * scale).round());
        assert_eq!(
            geometry.text_size,
            (LIST_ROW_TEXT_SIZE as f32 * scale).round()
        );
        assert_eq!(
            geometry.row_spacing,
            (LIST_ROW_SPACING as f32 * scale).round()
        );
        assert_eq!(geometry.indent_width, (LIST_INDENT_WIDTH * scale).round());
        assert_eq!(geometry.toggle_width, (LIST_TOGGLE_WIDTH * scale).round());
        assert_eq!(
            geometry.row_padding.right,
            (LIST_ROW_PADDING[1] as f32 * scale).round()
        );

        // 表头高度是面板 chrome，不随档位缩放；初始窗口仍按真实行高推导。
        assert_eq!(LIST_HEADER_HEIGHT, LIST_HEADER_CELL_HEIGHT + 8.0);
        let expected_rows = ((900.0 - LIST_HEADER_HEIGHT) / geometry.row_height).ceil() as usize
            + LIST_OVERSCAN_ROWS;
        assert_eq!(list_initial_rows(900.0, level), expected_rows);
    }
}
