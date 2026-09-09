mod address_bar;
mod application_logs;
mod archive_creation;
mod archive_extraction;
mod auxiliary_window_layout;
mod batch_rename;
mod checksum;
mod context_menu_settings;
mod convert;
mod document_preview_panel;
mod file_operation_verification_settings;
mod floating_panels;
mod markdown_preview;
mod network_connections;
mod network_settings;
mod option_controls;
mod preview_panel;
mod preview_settings;
mod properties_window;
mod rendering_settings;
mod right_preview_panel;
mod search_panel;
mod search_settings;
mod settings_group;
mod settings_window;
mod shortcut_settings;
mod sidebar_panel;
mod sqlite_preview_panel;
mod tab_bar;
pub(crate) mod tab_motion;
mod text_preview_panel;
mod toggle_switch;
mod toolbar_controls;
mod transfer_conflict;
mod trash_warning;
mod window_chrome;
mod window_control_settings;
mod window_drag_region;

pub(crate) use window_chrome::{
    auxiliary_window_content, floating_preview_window_content, separate_window_content,
    window_resize_frame, MainPaneWindowChromeRole,
};

pub(crate) use address_bar::address_input_id;
pub(crate) use preview_panel::view_preview_window;
pub(crate) use properties_window::view_properties_window;
pub(crate) use search_panel::{search_input_id, SEARCH_RESULT_ROW_HEIGHT};
pub(crate) use settings_window::view_settings_window;
pub(crate) use tab_motion::translated_with_width_overflow;

use std::path::Path;

use file_core::{DirectoryEntry, FileKind};
use iced::widget::{container, mouse_area, opaque, row, stack, Column, Row, Space, Svg};
use iced::{Alignment, Element, Length, Point, Theme};

use crate::app::panes::BrowserPaneView;
use crate::app::smooth_scroll::smooth_scroll_id;
use crate::app::FileBrowser;
use crate::appearance::{
    app_content_style, column_resize_divider_style, drag_preview_badge_style,
    drag_preview_style, icon_svg_style,
    selected_icon_svg_style, selected_tab_item_style, tab_split_overlay_style,
    warning_icon_svg_style,
};
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::floating_surface::{
    dismissable_blocking_floating_surface, floating_surface, modal_floating_surface,
    replaceable_context_menu_floating_surface, FloatingContent, FloatingPlacement,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icon_grid_view::icon_grid_view;
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::list_view::list_browser_view;
use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserViewMode, Message, ScrollbarRegion, SplitAxis,
    WindowChromeLayout, WindowControlSide, TRASH_LOCATION_LABEL,
};
use crate::operation_queue_view::{
    operation_queue_indicator, operation_queue_panel, OPERATION_QUEUE_INDICATOR_BOTTOM,
    OPERATION_QUEUE_INDICATOR_RIGHT, OPERATION_QUEUE_PANEL_BOTTOM,
};
use crate::selection_marquee::selection_marquee_layer;
use crate::three_column_view::column_browser_view;
use crate::typography::readable_text;

use self::network_connections::network_connection_editor_panel;
use address_bar::address_bar;
use archive_creation::archive_creation_panel;
use archive_extraction::archive_extraction_panel;
use batch_rename::batch_rename_panel;
use checksum::checksum_panel;
use convert::convert_panel;
use floating_panels::{
    context_menu_panel, destructive_action_confirmation_panel, error_notification_panel,
    file_drop_operation_panel, open_with_panel,
};
use rendering_settings::renderer_restart_notice_panel;
use right_preview_panel::right_preview_panel;
use search_panel::{search_input_panel, search_results_view};
use sidebar_panel::sidebar_view;
use tab_bar::tab_bar;
use toolbar_controls::{
    navigation_button_group, right_preview_panel_toggle_button, view_mode_button_group,
};
use transfer_conflict::transfer_conflict_panel;
use window_chrome::{pane_navigation_layout, window_control_group, PaneNavigationLayout};
use window_drag_region::window_drag_region;

const TOOLBAR_ICON_SIZE: f32 = 16.0;
const VIEW_MODE_ICON_SIZE: f32 = 16.0;
const TAB_ICON_SIZE: f32 = 14.0;
const TAB_CLOSE_ICON_SIZE: f32 = 12.0;
const TAB_CLOSE_SLOT_WIDTH: f32 = 22.0;
const TAB_BAR_EXPANDED_HEIGHT: f32 = 34.0;
const TAB_FILL_PORTION: u16 = 1000;
const TAB_DRAG_PREVIEW_WIDTH: f32 = 220.0;
pub(super) const MENU_ICON_SIZE: f32 = 16.0;
const TAB_LABEL_MAX_CHARS: usize = 24;
const ERROR_NOTIFICATION_CONTENT_OFFSET_X: f32 = 18.0;
const ERROR_NOTIFICATION_FLOAT_Y: f32 = 18.0;
const RENDERER_RESTART_NOTICE_ERROR_OFFSET_Y: f32 = 58.0;
const DRAG_PREVIEW_ICON_SIZE: f32 = 18.0;
const DRAG_PREVIEW_LABEL_MAX_CHARS: usize = 34;
const DRAG_PREVIEW_OFFSET_X: f32 = 14.0;
const DRAG_PREVIEW_OFFSET_Y: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserFloatingInput {
    Plain,
    Modal,
    DismissibleBlocking,
    ContextMenuReplacement,
}

impl BrowserFloatingInput {
    fn with_additional_panel(self, next: Self) -> Self {
        match (self, next) {
            (Self::Modal, _) | (_, Self::Modal) => Self::Modal,
            (Self::DismissibleBlocking, _) | (_, Self::DismissibleBlocking) => {
                Self::DismissibleBlocking
            }
            (Self::ContextMenuReplacement, _) | (_, Self::ContextMenuReplacement) => {
                Self::ContextMenuReplacement
            }
            (Self::Plain, Self::Plain) => Self::Plain,
        }
    }
}

pub(crate) fn rename_input_id() -> iced::widget::Id {
    iced::widget::Id::new("rename-input")
}

pub(crate) fn batch_rename_preview_name_input_id(path: &Path) -> iced::widget::Id {
    iced::widget::Id::from(format!("batch-rename-preview-name-{}", path.display()))
}

pub(crate) fn column_browser_scroll_id(pane_id: BrowserPaneId) -> iced::widget::Id {
    smooth_scroll_id(&ScrollbarRegion::ColumnBrowser(pane_id))
}

pub(super) fn auxiliary_window_message(message: &'static str) -> Element<'static, Message> {
    container(readable_text(message).size(14))
        .padding(18)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

pub(crate) fn view_browser(browser: &FileBrowser) -> Element<'_, Message> {
    // 边栏卡片是贴满整个侧边的浮层:从窗口顶沿到窗底,盖在终端抽屉
    // 左段之上(抽屉分隔线从卡片后方横贯窗宽)。工具栏与窗格用定宽
    // 占位让出卡片;预览面板按需挂在 row 尾部,panes_view 以 Fill 收窄。
    let mut pane_row = Row::new()
        .push(Space::new().width(Length::Fixed(browser.sidebar_width)))
        .push(
            container(panes_view(browser))
                .width(Length::Fill)
                .height(Length::Fill),
        );
    // 面板开启才挂进 row;关闭时只有侧栏占位与窗格两项。
    if browser.right_preview_panel_open {
        pane_row = pane_row.push(right_preview_panel(browser));
    }
    let pane_row = pane_row.width(Length::Fill).height(Length::Fill);
    // 工具栏只占边栏右侧:卡片顶到窗口上沿,顶栏左端随之让位;
    // 窗口控制仍在顶栏右端,窗格与终端抽屉不受影响。
    let base_layer: Element<'_, Message> = container(
        iced::widget::column![
            row![
                Space::new().width(Length::Fixed(browser.sidebar_width)),
                main_window_top_bar(browser),
            ]
            .width(Length::Fill)
            .height(Length::Fixed(crate::model::MAIN_TOOLBAR_ROW_HEIGHT)),
            pane_row,
            crate::terminal_panel::view::terminal_panel_area(browser),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(app_content_style)
    .into();
    // 包裹容器必须保持内容尺寸(定宽侧栏条):opaque 层在其整个
    // bounds 内捕获鼠标按下,一旦撑满全窗,侧栏以外的整个界面都会
    // 点击失效。
    let sidebar_overlay: Element<'_, Message> = container(sidebar_view(browser)).into();
    let content: Element<'_, Message> = stack([base_layer, opaque(sidebar_overlay)])
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    let mut floating = Vec::new();
    let mut floating_input = BrowserFloatingInput::Plain;
    if let Some(confirmation) = &browser.destructive_action_confirmation {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: destructive_action_confirmation_panel(confirmation),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(file_drop_prompt) = &browser.file_drop_prompt {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: file_drop_operation_panel(file_drop_prompt),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(conflict) = &browser.transfer_conflict {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: transfer_conflict_panel(conflict, &browser.thumbnail_cache),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(archive_extraction) = &browser.archive_extraction {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: archive_extraction_panel(archive_extraction),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(batch_rename) = &browser.batch_rename {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: batch_rename_panel(
                batch_rename,
                browser.scrollbar_visibility_for(&ScrollbarRegion::BatchRenamePreview),
                browser.scrollbar_viewport_for(&ScrollbarRegion::BatchRenamePreview),
            ),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(editor) = &browser.network_connection_editor {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: network_connection_editor_panel(editor),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(drag_preview) = drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: drag_preview,
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
        });
    } else if let Some(archive_creation) = &browser.archive_creation {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: archive_creation_panel(archive_creation),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(convert) = &browser.convert {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: convert_panel(convert),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(checksum) = &browser.checksum {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: checksum_panel(checksum),
            placement: FloatingPlacement::Center,
        });
    } else if let Some(context_menu) = &browser.context_menu {
        floating_input = BrowserFloatingInput::ContextMenuReplacement;
        let selected_search_entry_types = browser
            .search_workspace
            .as_ref()
            .map(|workspace| workspace.filters.selected_entry_types.as_slice())
            .unwrap_or_default();
        floating.push(FloatingContent {
            element: context_menu_panel(
                context_menu,
                browser.is_trash_view,
                browser.active_pane_id(),
                &browser.user_config().context_menus,
                &browser.user_config().list_view_preferences,
                selected_search_entry_types,
            ),
            placement: FloatingPlacement::At(context_menu.position()),
        });
    } else if let Some(open_with) = &browser.open_with {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: open_with_panel(
                open_with,
                browser.scrollbar_visibility_for(&ScrollbarRegion::OpenWithApplications),
                browser.scrollbar_viewport_for(&ScrollbarRegion::OpenWithApplications),
            ),
            placement: FloatingPlacement::Center,
        });
    }

    if let Some(bounds) = browser.tab_split_overlay_bounds() {
        floating.push(FloatingContent {
            element: tab_split_overlay(bounds.width, bounds.height),
            placement: FloatingPlacement::Free(bounds.top_left),
        });
    }

    if let Some(bounds) = browser.pane_drag_overlay_bounds() {
        floating.push(FloatingContent {
            element: tab_split_overlay(bounds.width, bounds.height),
            placement: FloatingPlacement::Free(bounds.top_left),
        });
    }

    if let Some(tab_preview) = tab_drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: tab_preview,
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
        });
    }

    if let Some(pane_preview) = pane_drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: pane_preview,
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
        });
    }

    if let Some(directory) = browser.terminal_tab_drag_preview() {
        floating.push(FloatingContent {
            element: container(tab_title_content(directory, false, IconTone::Selected))
                .padding([7, 10])
                .width(Length::Fixed(TAB_DRAG_PREVIEW_WIDTH))
                .style(selected_tab_item_style)
                .into(),
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
        });
    }

    if let Some((error, generation)) = browser.current_error_notification() {
        floating.push(FloatingContent {
            element: error_notification_panel(error, generation),
            placement: FloatingPlacement::At(iced::Point::new(
                browser.sidebar_width + ERROR_NOTIFICATION_CONTENT_OFFSET_X,
                ERROR_NOTIFICATION_FLOAT_Y,
            )),
        });
    }

    if browser.renderer_restart_notice_visible {
        let notice_y = if browser.current_error().is_some() {
            ERROR_NOTIFICATION_FLOAT_Y + RENDERER_RESTART_NOTICE_ERROR_OFFSET_Y
        } else {
            ERROR_NOTIFICATION_FLOAT_Y
        };
        floating.push(FloatingContent {
            element: renderer_restart_notice_panel(),
            placement: FloatingPlacement::At(iced::Point::new(
                browser.sidebar_width + ERROR_NOTIFICATION_CONTENT_OFFSET_X,
                notice_y,
            )),
        });
    }

    if browser.operation_queue.is_panel_open() {
        floating_input =
            floating_input.with_additional_panel(BrowserFloatingInput::DismissibleBlocking);
        floating.push(FloatingContent {
            element: operation_queue_panel(
                &browser.operation_queue,
                browser.scrollbar_visibility_for(&ScrollbarRegion::OperationQueue),
                browser.scrollbar_viewport_for(&ScrollbarRegion::OperationQueue),
                browser.operation_progress_animation_frame,
            ),
            placement: FloatingPlacement::BottomLeft {
                left: browser.sidebar_width + 12.0,
                bottom: OPERATION_QUEUE_PANEL_BOTTOM,
            },
        });
    }

    if let Some(indicator) = operation_queue_indicator(
        &browser.operation_queue,
        browser.operation_progress_animation_frame,
    ) {
        floating.push(FloatingContent {
            element: indicator,
            placement: FloatingPlacement::BottomRightInArea {
                area_width: browser.sidebar_width,
                right: OPERATION_QUEUE_INDICATOR_RIGHT,
                bottom: OPERATION_QUEUE_INDICATOR_BOTTOM,
            },
        });
    }

    let browser_surface = match floating_input {
        BrowserFloatingInput::Plain => floating_surface(content, floating),
        BrowserFloatingInput::Modal => modal_floating_surface(content, floating),
        BrowserFloatingInput::DismissibleBlocking => {
            dismissable_blocking_floating_surface(content, floating, Message::DismissFloating)
        }
        BrowserFloatingInput::ContextMenuReplacement => {
            replaceable_context_menu_floating_surface(content, floating, Message::DismissFloating)
        }
    };
    let main_window = browser.main_window_id();
    let frame_state = browser.window_frame_state(main_window);
    let window_content = match browser.user_config().window_controls.layout() {
        WindowChromeLayout::IntegratedNavigation => browser_surface,
        WindowChromeLayout::SeparateTitleBar => separate_window_content(
            browser.window_title(main_window),
            browser_surface,
            &browser.user_config().window_controls,
            main_window,
            frame_state,
        ),
    };
    window_resize_frame(window_content, main_window, frame_state)
}

fn panes_view(browser: &FileBrowser) -> Element<'_, Message> {
    match browser.pane_layout {
        BrowserPaneLayout::Single { active } => pane_view(browser, active),
        BrowserPaneLayout::Split {
            axis,
            first,
            second,
            ..
        } => {
            let (first_portion, second_portion) = browser
                .pane_layout
                .effective_split_portions(browser.split_axis_extent(axis));
            let divider = split_resize_divider(axis);
            match axis {
                SplitAxis::Horizontal => Row::new()
                    .spacing(0)
                    .push(
                        container(pane_view(browser, first))
                            .width(Length::FillPortion(first_portion)),
                    )
                    .push(divider)
                    .push(
                        container(pane_view(browser, second))
                            .width(Length::FillPortion(second_portion)),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                SplitAxis::Vertical => Column::new()
                    .spacing(0)
                    .push(
                        container(pane_view(browser, first))
                            .height(Length::FillPortion(first_portion)),
                    )
                    .push(divider)
                    .push(
                        container(pane_view(browser, second))
                            .height(Length::FillPortion(second_portion)),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
            }
        }
    }
}

fn split_resize_divider(axis: SplitAxis) -> Element<'static, Message> {
    let line: Element<'static, Message> = match axis {
        SplitAxis::Horizontal => container(Space::new().width(Length::Fixed(1.0)))
            .height(Length::Fill)
            .style(column_resize_divider_style)
            .into(),
        SplitAxis::Vertical => container(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(column_resize_divider_style)
            .into(),
    };
    let divider: Element<'static, Message> = match axis {
        SplitAxis::Horizontal => Row::new()
            .push(Space::new().width(Length::Fill))
            .push(line)
            .push(Space::new().width(Length::Fill))
            .width(Length::Fixed(crate::model::SPLIT_DIVIDER_WIDTH))
            .height(Length::Fill)
            .into(),
        SplitAxis::Vertical => Column::new()
            .push(Space::new().height(Length::Fill))
            .push(line)
            .push(Space::new().height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fixed(crate::model::SPLIT_DIVIDER_WIDTH))
            .into(),
    };
    mouse_area(divider)
        .on_press(Message::SplitResizeStarted)
        .on_release(Message::DragSelectionFinished)
        .interaction(match axis {
            SplitAxis::Horizontal => iced::mouse::Interaction::ResizingHorizontally,
            SplitAxis::Vertical => iced::mouse::Interaction::ResizingVertically,
        })
        .into()
}

/// 全窗工具栏顶栏:窗口控制居两端,导航/地址栏/搜索/视图切换/预览
/// 开关取自活动窗格。集成导航布局时整条是窗口拖拽区;独立标题栏
/// 布局时控制归标题栏,顶栏只承载工具栏本体。
fn main_window_top_bar<'a>(browser: &'a FileBrowser) -> Element<'a, Message> {
    let chrome_role = match browser.user_config().window_controls.layout() {
        WindowChromeLayout::IntegratedNavigation => MainPaneWindowChromeRole::Complete,
        WindowChromeLayout::SeparateTitleBar => MainPaneWindowChromeRole::NoChrome,
    };
    let Some(pane) = browser.pane_view(browser.active_pane_id()) else {
        return Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(crate::model::MAIN_TOOLBAR_ROW_HEIGHT))
            .into();
    };
    let main_window = browser.main_window_id();
    let navigation_content = pane_navigation_content(browser, pane, chrome_role);
    // 高度钉死为共享常量:侧栏/窗格几何的偏移量取同一值,保证不漂移。
    let bar_content: Element<'_, Message> = container(navigation_content)
        .padding(18)
        .width(Length::Fill)
        .height(Length::Fixed(crate::model::MAIN_TOOLBAR_ROW_HEIGHT))
        .into();
    if chrome_role.owns_window_drag_region() {
        window_drag_region(bar_content, main_window)
    } else {
        bar_content
    }
}

fn pane_view(browser: &FileBrowser, pane_id: BrowserPaneId) -> Element<'_, Message> {
    let Some(pane) = browser.pane_view(pane_id) else {
        return Space::new().width(Length::Fill).height(Length::Fill).into();
    };
    // 工具栏已上移为全窗顶栏,窗格本体从 tab 条/内容开始。
    let mut main_content = Column::new().spacing(0);
    if pane.tab_bar_should_occupy_layout() {
        main_content = main_content.push(tab_bar(browser, pane));
    }

    if pane.is_trash_view {
        if let Some(warning_panel) = trash_warning::trash_warning_panel(browser) {
            main_content = main_content.push(warning_panel);
        }
    }

    let marquee = (pane_id == browser.active_pane_id())
        .then_some(browser.selection_marquee.as_ref())
        .flatten();
    let file_content = selection_marquee_layer(browser_content_view(browser, pane), marquee);
    let pane_content = main_content
        .push(file_content)
        .width(Length::Fill)
        .height(Length::Fill);

    mouse_area(pane_content)
        .on_enter(Message::PaneCursorEntered(pane_id))
        .on_exit(Message::PaneCursorExited(pane_id))
        .into()
}

fn pane_navigation_content<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    chrome_role: MainPaneWindowChromeRole,
) -> Element<'a, Message> {
    match pane_navigation_layout(browser.main_window_width) {
        PaneNavigationLayout::SingleRow => {
            let navigation = Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill);
            let navigation = push_pane_window_controls(
                navigation,
                browser,
                chrome_role,
                WindowControlSide::Left,
            )
            .push(navigation_button_group(pane.id))
            .push(address_bar(browser, pane))
            .push(search_input_panel(browser))
            .push(view_mode_button_group(pane))
            .push(right_preview_panel_toggle_button(browser.right_preview_panel_open));
        push_pane_window_controls(navigation, browser, chrome_role, WindowControlSide::Right)
            .into()
        }
        PaneNavigationLayout::StackedRows => {
            let control_row = Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill);
            let control_row = push_pane_window_controls(
                control_row,
                browser,
                chrome_role,
                WindowControlSide::Left,
            )
            .push(navigation_button_group(pane.id))
            .push(Space::new().width(Length::Fill));
            let control_row = push_pane_window_controls(
                control_row,
                browser,
                chrome_role,
                WindowControlSide::Right,
            );
            let location_row = Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(address_bar(browser, pane))
                .push(search_input_panel(browser))
                .push(view_mode_button_group(pane))
                .push(right_preview_panel_toggle_button(browser.right_preview_panel_open))
                .width(Length::Fill);
            Column::new()
                .spacing(8)
                .push(control_row)
                .push(location_row)
                .width(Length::Fill)
                .into()
        }
    }
}

fn push_pane_window_controls<'a>(
    navigation: Row<'a, Message>,
    browser: &FileBrowser,
    chrome_role: MainPaneWindowChromeRole,
    side: WindowControlSide,
) -> Row<'a, Message> {
    let shows_controls = match side {
        WindowControlSide::Left => chrome_role.shows_left_controls(),
        WindowControlSide::Right => chrome_role.shows_right_controls(),
    };
    if !shows_controls {
        return navigation;
    }
    let main_window = browser.main_window_id();
    navigation.push(window_control_group(
        &browser.user_config().window_controls,
        side,
        main_window,
        browser.window_frame_state(main_window),
    ))
}

fn browser_content_view<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    if browser.search_workspace.is_some() {
        return track_file_drag_hit_test_marker(
            search_results_view(browser),
            FileDragHitTestMarker::BlockedDirectoryTarget { pane_id: pane.id },
        );
    }

    match pane.view_mode {
        BrowserViewMode::Columns => column_browser_view(browser, pane),
        BrowserViewMode::List => track_file_drag_hit_test_marker(
            list_browser_view(browser, pane),
            FileDragHitTestMarker::DirectoryTarget {
                pane_id: pane.id,
                directory: pane.current_dir.clone(),
            },
        ),
        BrowserViewMode::Icons => track_file_drag_hit_test_marker(
            icon_grid_view(browser, pane),
            FileDragHitTestMarker::DirectoryTarget {
                pane_id: pane.id,
                directory: pane.current_dir.clone(),
            },
        ),
    }
}

fn tab_split_overlay(width: f32, height: f32) -> Element<'static, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .style(tab_split_overlay_style)
        .into()
}

fn drag_preview_position(cursor_position: Point) -> Point {
    Point::new(
        cursor_position.x + DRAG_PREVIEW_OFFSET_X,
        cursor_position.y + DRAG_PREVIEW_OFFSET_Y,
    )
}

fn drag_preview_panel(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let drag = browser.file_drag.as_ref()?;
    if !drag.displays_iced_drag_preview() {
        return None;
    }
    let source = drag.sources.first()?;
    let (symbol, tone, label) = drag_preview_item(browser, source);
    let label = format_middle_ellipsized_text(&label, DRAG_PREVIEW_LABEL_MAX_CHARS);
    // 拖拽修饰键意图徽标:Ctrl=复制,其余为移动,实时跟随按键变化。
    let intent_label = match browser.file_drag_transfer_intent() {
        crate::model::TransferConflictMode::Copy => "Copy",
        crate::model::TransferConflictMode::Move => "Move",
    };
    let content = row![
        themed_icon(symbol, tone, DRAG_PREVIEW_ICON_SIZE),
        readable_text(label).size(13),
        container(readable_text(intent_label).size(12))
            .padding([2, 8])
            .style(drag_preview_badge_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    Some(
        container(content)
            .padding([7, 10])
            .style(drag_preview_style)
            .into(),
    )
}

fn drag_preview_item(browser: &FileBrowser, path: &Path) -> (IconSymbol, IconTone, String) {
    if let Some(entry) = browser.entry_for_path(path) {
        return drag_preview_entry_item(entry);
    }

    let name = path.file_name().unwrap_or(path.as_os_str());
    (
        file_entry_icon_symbol(FileKind::Other, name),
        IconTone::Normal,
        name.to_string_lossy().into_owned(),
    )
}

fn drag_preview_entry_item(entry: &DirectoryEntry) -> (IconSymbol, IconTone, String) {
    let symbol = if entry.kind == FileKind::Symlink && entry.is_broken_symlink {
        IconSymbol::TriangleAlert
    } else {
        file_entry_icon_symbol(entry.kind, entry.name())
    };
    let tone = if symbol == IconSymbol::TriangleAlert {
        IconTone::Warning
    } else {
        IconTone::Normal
    };

    (symbol, tone, entry.name().to_string_lossy().into_owned())
}

fn tab_drag_preview_panel(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let preview = browser.tab_drag_preview()?;
    Some(
        container(tab_title_content(
            preview.directory,
            preview.is_trash_view,
            IconTone::Selected,
        ))
        .padding([7, 10])
        .width(Length::Fixed(TAB_DRAG_PREVIEW_WIDTH))
        .style(selected_tab_item_style)
        .into(),
    )
}

fn pane_drag_preview_panel(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let preview = browser.pane_drag_preview()?;
    Some(
        container(tab_title_content(
            preview.directory,
            preview.is_trash_view,
            IconTone::Selected,
        ))
        .padding([7, 10])
        .width(Length::Fixed(TAB_DRAG_PREVIEW_WIDTH))
        .style(selected_tab_item_style)
        .into(),
    )
}

fn tab_title_content<'a>(
    directory: &'a Path,
    is_trash_view: bool,
    tone: IconTone,
) -> Row<'a, Message> {
    let symbol = if is_trash_view {
        IconSymbol::Trash
    } else {
        IconSymbol::Folder
    };
    row![
        themed_icon(symbol, tone, TAB_ICON_SIZE),
        readable_text(tab_title_text(directory, is_trash_view)).size(13),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
}

fn tab_title_text(directory: &Path, is_trash_view: bool) -> String {
    if is_trash_view {
        return crate::localization::translate_current(TRASH_LOCATION_LABEL);
    }

    let title = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| directory.to_string_lossy().into_owned());
    format_middle_ellipsized_text(&title, TAB_LABEL_MAX_CHARS)
}

pub(super) fn themed_icon(symbol: IconSymbol, tone: IconTone, size: f32) -> Svg<'static, Theme> {
    symbol.view(size).style(icon_tone_style(tone))
}

pub(super) fn icon_tone_style(
    tone: IconTone,
) -> fn(&Theme, iced::widget::svg::Status) -> iced::widget::svg::Style {
    match tone {
        IconTone::Normal => icon_svg_style(),
        IconTone::Selected => selected_icon_svg_style(),
        IconTone::Warning => warning_icon_svg_style(),
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum IconTone {
    Normal,
    Selected,
    Warning,
}
