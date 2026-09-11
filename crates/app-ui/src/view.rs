mod about_settings;
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

use iced::alignment::Vertical;
use iced::widget::{
    container, image, mouse_area, opaque, row, stack, Column, Row, Space, Stack, Svg,
};
use iced::{Alignment, Element, Length, Point, Theme};

use crate::app::panes::BrowserPaneView;
use crate::app::smooth_scroll::smooth_scroll_id;
use crate::app::FileBrowser;
use crate::appearance::{
    app_content_style, column_resize_divider_style, faded_drag_preview_label_style,
    faded_drag_preview_pill_style, icon_svg_style, selected_icon_svg_style,
    selected_tab_item_style, tab_split_overlay_style, warning_icon_svg_style,
};
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::floating_surface::{
    dismissable_blocking_floating_surface, floating_surface, modal_floating_surface,
    replaceable_context_menu_floating_surface, FloatingContent, FloatingPlacement,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icon_grid_view::icon_grid_view;
use crate::icons::IconSymbol;
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
            captures_pointer: true,
        });
    } else if let Some(file_drop_prompt) = &browser.file_drop_prompt {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: file_drop_operation_panel(file_drop_prompt),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
        });
    } else if let Some(conflict) = &browser.transfer_conflict {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: transfer_conflict_panel(conflict, &browser.thumbnail_cache),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
        });
    } else if let Some(archive_extraction) = &browser.archive_extraction {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: archive_extraction_panel(archive_extraction),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
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
            captures_pointer: true,
        });
    } else if let Some(editor) = &browser.network_connection_editor {
        floating_input = BrowserFloatingInput::Modal;
        floating.push(FloatingContent {
            element: network_connection_editor_panel(editor),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
        });
    } else if let Some((drag_preview, drag_preview_placement)) = drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: drag_preview,
            placement: drag_preview_placement,
            captures_pointer: false,
        });
        if let Some(action_capsule) = file_drag_action_capsule_panel(browser) {
            floating.push(FloatingContent {
                element: action_capsule,
                placement: FloatingPlacement::Free(Point::new(
                    browser.cursor_position.x + DRAG_ACTION_CAPSULE_OFFSET_X,
                    browser.cursor_position.y + DRAG_ACTION_CAPSULE_OFFSET_Y,
                )),
                captures_pointer: false,
            });
        }
    } else if let Some(archive_creation) = &browser.archive_creation {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: archive_creation_panel(archive_creation),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
        });
    } else if let Some(convert) = &browser.convert {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: convert_panel(convert),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
        });
    } else if let Some(checksum) = &browser.checksum {
        floating_input = BrowserFloatingInput::DismissibleBlocking;
        floating.push(FloatingContent {
            element: checksum_panel(checksum),
            placement: FloatingPlacement::Center,
            captures_pointer: true,
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
            captures_pointer: true,
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
            captures_pointer: true,
        });
    }

    if let Some(bounds) = browser.tab_split_overlay_bounds() {
        floating.push(FloatingContent {
            element: tab_split_overlay(bounds.width, bounds.height),
            placement: FloatingPlacement::Free(bounds.top_left),
            captures_pointer: true,
        });
    }

    if let Some(bounds) = browser.pane_drag_overlay_bounds() {
        floating.push(FloatingContent {
            element: tab_split_overlay(bounds.width, bounds.height),
            placement: FloatingPlacement::Free(bounds.top_left),
            captures_pointer: true,
        });
    }

    if let Some(tab_preview) = tab_drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: tab_preview,
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
            captures_pointer: true,
        });
    }

    if let Some(pane_preview) = pane_drag_preview_panel(browser) {
        floating.push(FloatingContent {
            element: pane_preview,
            placement: FloatingPlacement::Free(drag_preview_position(browser.cursor_position)),
            captures_pointer: true,
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
            captures_pointer: true,
        });
    }

    if let Some((error, generation)) = browser.current_error_notification() {
        floating.push(FloatingContent {
            element: error_notification_panel(error, generation),
            placement: FloatingPlacement::At(iced::Point::new(
                browser.sidebar_width + ERROR_NOTIFICATION_CONTENT_OFFSET_X,
                ERROR_NOTIFICATION_FLOAT_Y,
            )),
            captures_pointer: true,
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
            captures_pointer: true,
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
            captures_pointer: true,
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
            captures_pointer: true,
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
            .push(right_preview_panel_toggle_button(
                browser.right_preview_panel_open,
            ));
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
                .push(right_preview_panel_toggle_button(
                    browser.right_preview_panel_open,
                ))
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

// 拖拽预览:提起的选中条目按按下瞬间的相对位置围绕光标排布,
// 离光标越远越淡。最大半径与拖出软件的 Wayland 位图上限一致。
const DRAG_PREVIEW_FADE_RADIUS: f32 = 256.0;
// 此距离内不参与淡出,核心组保持全浓。
const DRAG_PREVIEW_FADE_SOLID_DISTANCE: f32 = 96.0;
const DRAG_PREVIEW_MAX_TILES: usize = 128;
const DRAG_PREVIEW_TILE_SIZE: f32 = 24.0;
const DRAG_PREVIEW_LABEL_SIZE: f32 = 12.0;
const DRAG_PREVIEW_LABEL_WIDTH: f32 = 132.0;
const DRAG_PREVIEW_LABEL_MAX_CHARS: usize = 20;
// 条目行:24 图标 + 6 间距 + 132 文件名,全部裸露无背景。
const DRAG_PREVIEW_PILL_WIDTH: f32 = 162.0;
const DRAG_PREVIEW_PILL_HEIGHT: f32 = 24.0;
const DRAG_PREVIEW_SUMMARY_TEXT_SIZE: f32 = 12.0;
// 动作胶囊相对光标的摆放偏移:z 序在预览之上(后 push),
// 覆盖在提起的缩略图上。
const DRAG_ACTION_CAPSULE_OFFSET_X: f32 = 25.0;
const DRAG_ACTION_CAPSULE_OFFSET_Y: f32 = 25.0;

/// 返回预览浮层与其定位:提起条目组左上角对准最早出现的条目,使负
/// 偏移(按住条目右下时)也能完整显示;聚合行右下角钉在指针尖上,
/// 往指针左上展开,不挡指针也不被指针挡。
fn drag_preview_panel(
    browser: &FileBrowser,
) -> Option<(Element<'_, Message>, FloatingPlacement)> {
    // 临时调试:环境变量触发,绕过拖拽状态直接渲染聚合行,验证浮层渲染链。
    if std::env::var_os("FM_DEBUG_DRAG_SUMMARY").is_some() {
        let summary = crate::wayland_drag_icon::file_drag_group_summary_text(37, 12);
        return Some((
            drag_preview_summary_row(&summary),
            FloatingPlacement::AnchorBottomRight {
                anchor: browser.cursor_position,
            },
        ));
    }
    let drag = browser.file_drag.as_ref()?;
    if !drag.displays_iced_drag_preview() || drag.preview_entries.is_empty() {
        return None;
    }
    // 出界判断必须用全部选中条目的包围盒:淡出半径外的条目虽不
    // 渲染,但它们代表着"屏幕铺不下"这件事本身。
    let offsets: Vec<iced::Vector> = drag
        .preview_entries
        .iter()
        .map(|entry| entry.offset)
        .collect();
    let origin_x = offsets
        .iter()
        .map(|offset| offset.x)
        .fold(0.0_f32, f32::min);
    let origin_y = offsets
        .iter()
        .map(|offset| offset.y)
        .fold(0.0_f32, f32::min);
    let stack_width = offsets
        .iter()
        .map(|offset| offset.x - origin_x)
        .fold(0.0_f32, f32::max)
        + DRAG_PREVIEW_PILL_WIDTH;
    let stack_height = offsets
        .iter()
        .map(|offset| offset.y - origin_y)
        .fold(0.0_f32, f32::max)
        + DRAG_PREVIEW_PILL_HEIGHT;
    let stack_origin = Point::new(
        browser.cursor_position.x + origin_x,
        browser.cursor_position.y + origin_y,
    );
    // 列表是虚拟化的,bounds 快照只覆盖屏幕上可见的行:选中数超过
    // 快照数就说明有选中内容在屏幕外。此时(或包围盒超出列表可视
    // 区——它比窗口小,上下还隔着工具栏等)屏幕已铺不下选中内容,
    // 收拢为一行总数文字,不再逐个铺开。
    let selection_overflows_screen =
        drag.sources.len() > drag.preview_entries.len();
    let overflows_viewport = match browser.file_drag_viewport {
        Some(viewport) => {
            let viewport_right = viewport.x + viewport.width;
            let viewport_bottom = viewport.y + viewport.height;
            stack_origin.x < viewport.x
                || stack_origin.y < viewport.y
                || stack_origin.x + stack_width > viewport_right
                || stack_origin.y + stack_height > viewport_bottom
        }
        // 视口快照未就绪:退回窗口边界判断。
        None => {
            stack_origin.x < 0.0
                || stack_origin.y < 0.0
                || stack_origin.x + stack_width > browser.main_window_width
                || stack_origin.y + stack_height > browser.main_window_height
        }
    };
    if selection_overflows_screen || overflows_viewport {
        let (folders, files) = browser.file_drag_group_counts();
        let summary = crate::wayland_drag_icon::file_drag_group_summary_text(folders, files);
        return Some((
            drag_preview_summary_row(&summary),
            FloatingPlacement::AnchorBottomRight {
                anchor: browser.cursor_position,
            },
        ));
    }
    let tiles: Vec<(iced::Vector, Element<'static, Message>)> = drag
        .preview_entries
        .iter()
        .filter_map(|entry| {
            let fade = drag_preview_fade(entry.offset)?;
            Some((
                entry.offset,
                drag_preview_entry_tile(browser, &entry.path, fade),
            ))
        })
        .take(DRAG_PREVIEW_MAX_TILES)
        .collect();
    let layers: Vec<Element<'static, Message>> = tiles
        .into_iter()
        .map(|(offset, tile)| drag_preview_tile_layer(offset, tile, origin_x, origin_y))
        .collect();
    // 浮层的可视范围按 Stack 的布局尺寸划定,不设尺寸就只盖住第一个
    // 胶囊,后续胶囊会被视口剔除;这里显式撑出覆盖全部胶囊的包围盒。
    let stack = Stack::with_children(layers)
        .width(Length::Fixed(stack_width))
        .height(Length::Fixed(stack_height));
    Some((
        stack.into(),
        FloatingPlacement::Free(stack_origin),
    ))
}

/// 拖拽动作胶囊:与提起条目预览同门控(出窗交接原生拖放后一并消失),
/// 文案由 file_drag_action_capsule_label 实时合成,底板与聚合行同款。
fn file_drag_action_capsule_panel(browser: &FileBrowser) -> Option<Element<'static, Message>> {
    let label = browser.file_drag_action_capsule_label()?;
    Some(drag_preview_summary_row(&label))
}

/// 聚合行:总数说明文字,文字胶囊底板与逐个条目同款。
fn drag_preview_summary_row(label: &str) -> Element<'static, Message> {
    container(
        readable_text(label.to_owned())
            .size(DRAG_PREVIEW_SUMMARY_TEXT_SIZE)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding([4, 6])
    .style(|theme| faded_drag_preview_pill_style(theme, 0.0))
    .into()
}

/// 离光标越远越淡:淡出半径线性,核心距离内保持全浓;超出半径不显示。
/// 返回的是"可见度"(1 = 全浓,0 = 消失)。
fn drag_preview_fade(offset: iced::Vector) -> Option<f32> {
    let distance = (offset.x * offset.x + offset.y * offset.y).sqrt();
    let visibility = (DRAG_PREVIEW_FADE_RADIUS - distance)
        / (DRAG_PREVIEW_FADE_RADIUS - DRAG_PREVIEW_FADE_SOLID_DISTANCE);
    (visibility > 0.0).then(|| visibility.clamp(0.0, 1.0))
}

/// 单个条目:内容缩略图(没有则退回文件类型图标)+ 文件名,全部
/// 裸露无背景,随淡出程度向背景色收敛。faded_themed_icon 收的是
/// "向背景混色量"(1 = 消失),上面算出的 fade 是"可见度"
/// (1 = 全浓),传参时做 1 - x 换算。
fn drag_preview_entry_tile(
    browser: &FileBrowser,
    path: &std::path::Path,
    fade: f32,
) -> Element<'static, Message> {
    let symbol = browser.file_drag_icon_symbol(path);
    let tone = if symbol == IconSymbol::TriangleAlert {
        IconTone::Warning
    } else {
        IconTone::Normal
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    let label = format_middle_ellipsized_text(name, DRAG_PREVIEW_LABEL_MAX_CHARS);
    let leading: Element<'static, Message> = match browser.drag_preview_thumbnail(path) {
        Some(handle) => image(handle)
            .width(Length::Fixed(DRAG_PREVIEW_TILE_SIZE))
            .height(Length::Fixed(DRAG_PREVIEW_TILE_SIZE))
            .opacity(fade)
            .into(),
        None => faded_themed_icon(symbol, tone, DRAG_PREVIEW_TILE_SIZE, 1.0 - fade).into(),
    };
    row![
        leading,
        container(
            readable_text(label)
                .size(DRAG_PREVIEW_LABEL_SIZE)
                .width(Length::Fixed(DRAG_PREVIEW_LABEL_WIDTH))
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .style(move |theme| faded_drag_preview_label_style(theme, 1.0 - fade)),
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .into()
}

/// iced 容器无法给内容负偏移,用占位空间把胶囊放到相对光标的
/// 目标位置;Stack 尺寸由占位与胶囊自身撑出。
fn drag_preview_tile_layer(
    offset: iced::Vector,
    tile: Element<'static, Message>,
    origin_x: f32,
    origin_y: f32,
) -> Element<'static, Message> {
    Column::with_children(vec![
        Space::new().height(offset.y - origin_y).into(),
        row![Space::new().width(offset.x - origin_x), tile].into(),
    ])
    .into()
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

/// 图标颜色向背景色褪色(fade 0=原色,1=完全融入背景):拖拽堆叠的
/// 后层用它做出深浅层次。
pub(super) fn faded_themed_icon(
    symbol: IconSymbol,
    tone: IconTone,
    size: f32,
    fade: f32,
) -> Svg<'static, Theme> {
    symbol.view(size).style(move |theme, status| {
        let mut style = icon_tone_style(tone)(theme, status);
        if let Some(color) = style.color.as_mut() {
            let background = crate::matugen_theme::ui_colors(theme).background;
            *color = mix_color(*color, background, fade);
        }
        style
    })
}

fn mix_color(from: iced::Color, to: iced::Color, t: f32) -> iced::Color {
    iced::Color {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a,
    }
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
