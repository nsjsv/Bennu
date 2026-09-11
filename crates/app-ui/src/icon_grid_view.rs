use std::path::Path;
use std::time::Duration;

use file_core::{DirectoryEntry, FileKind};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, container, mouse_area, scrollable, text, text_input, tooltip, Column, Row, Space, Stack,
};
use iced::{Alignment, Element, Length};

use crate::app::panes::{BrowserPaneView, DirectoryContentAvailability};
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content_with_shift, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    context_menu_style, enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction,
    icon_grid_expansion_panel_style, icon_svg_style, list_panel_style,
    navigation_icon_button_style,
};
use crate::column_entry_bounds::track_column_entry_bounds;
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::file_entry_view::{
    entry_text_input_style, entry_thumbnail_or_icon, FileEntryIconDensity, FileEntryVisualState,
};
use crate::icon_grid_geometry::{
    grid_gap, icon_label_spacing, label_height, label_line_height, label_size, row_height,
    tile_padding_horizontal, tile_padding_vertical, tile_visual_height, tile_width,
    ICON_GRID_CONTENT_PADDING, ICON_GRID_LABEL_SIZE,
};
use crate::icon_grid_layout::{
    IconGridBandLayout, IconGridFlowSegment, IconGridPanelLayout, IconGridPanelStatus,
    IconGridRowsLayout, ICON_GRID_STATUS_HEIGHT,
};
use crate::icons::rotated_chevron_right_view;
use crate::input_blocking_space::input_blocking_space;
use crate::measured_middle_ellipsized_text::measured_middle_ellipsized_wrapped_text_with_tooltip;
use crate::model::{IconGridExpansionAnchor, IconGridViewport, Message, ScrollbarRegion};
use crate::typography::readable_text;
use crate::view::rename_input_id;

const GRID_PADDING: f32 = ICON_GRID_CONTENT_PADDING;
const DISCLOSURE_BUTTON_SIZE: f32 = 24.0;
const DISCLOSURE_ICON_SIZE: f32 = 14.0;
const PANEL_INDICATOR_SIZE: f32 = 14.0;
const ICON_GRID_NAME_TOOLTIP_WIDTH: f32 = 320.0;
const ICON_GRID_NAME_TOOLTIP_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconGridPanelInput {
    Interactive,
    VisualOnly,
}

pub(crate) fn icon_grid_view<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    let pane_id = pane.id;
    let content: Element<'a, Message> = match pane.current_directory_content() {
        DirectoryContentAvailability::Pending => Space::new().height(Length::Fill).into(),
        DirectoryContentAvailability::Available([]) => grid_message(if pane.is_trash_view {
            "Trash is empty"
        } else {
            "No items"
        }),
        DirectoryContentAvailability::Available(_) => {
            let layout = browser.icon_grid_layout_for_pane(pane);
            render_panel(
                browser,
                pane,
                layout.root(),
                pane.icon_grid_viewport,
                0.0,
                layout.total_height(),
                IconGridPanelInput::Interactive,
                0,
            )
        }
    };
    let scrollbar_region = ScrollbarRegion::PaneIcons(pane_id);
    let scrollbar_visibility = browser.scrollbar_visibility_for(&scrollbar_region);
    let grid_scroll = scrollable(smooth_scroll_content_with_shift(
        content,
        scrollbar_region.clone(),
        browser.smooth_scroll_shift_pressed(),
    ))
        .id(smooth_scroll_id(&scrollbar_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            8.0,
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .on_scroll(scrollbar_on_scroll(
            scrollbar_region.clone(),
            move |viewport: scrollable::Viewport| {
                let offset = viewport.absolute_offset();
                let bounds = viewport.bounds();
                Message::IconGridScrolled(pane_id, offset.y, bounds.width, bounds.height)
            },
        ));
    let grid_scroll = enhanced_scrollbar(
        grid_scroll,
        scrollbar_visibility,
        browser.scrollbar_viewport_for(&scrollbar_region),
        ScrollbarAxis::Vertical,
        8.0,
    );

    mouse_area(
        container(grid_scroll)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(list_panel_style),
    )
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

fn render_panel<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    panel: &IconGridPanelLayout<'a>,
    viewport: IconGridViewport,
    panel_top: f32,
    clip_bottom: f32,
    input: IconGridPanelInput,
    depth: usize,
) -> Element<'a, Message> {
    let mut content = Column::new().spacing(0).width(Length::Fill);
    let mut cursor = 0.0;

    if panel.status == IconGridPanelStatus::Loaded {
        for segment in &panel.flow {
            content = content.push(vertical_spacer(segment.top() - cursor));
            content = match segment {
                IconGridFlowSegment::Rows(rows) => content.push(render_rows(
                    browser,
                    pane,
                    rows,
                    viewport,
                    panel_top,
                    clip_bottom,
                    input,
                )),
                IconGridFlowSegment::Band(band) => content.push(render_band(
                    browser,
                    pane,
                    band,
                    viewport,
                    panel_top,
                    clip_bottom,
                    input,
                    depth + 1,
                )),
            };
            cursor = segment.top() + segment.height();
        }
    } else {
        content = content
            .push(vertical_spacer(GRID_PADDING))
            .push(panel_status(panel.status));
        cursor = GRID_PADDING + ICON_GRID_STATUS_HEIGHT;
    }

    content = content.push(vertical_spacer(panel.height - cursor));
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(panel.height))
        .into()
}

fn render_rows<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    rows: &IconGridRowsLayout<'a>,
    viewport: IconGridViewport,
    panel_top: f32,
    clip_bottom: f32,
    input: IconGridPanelInput,
) -> Element<'a, Message> {
    let icon_edge = browser.user_config().icons_icon_edge();
    let Some(visible) = rows.visible_rows(
        panel_top,
        clip_bottom,
        viewport,
        icon_edge,
        browser.main_window_height,
    ) else {
        return vertical_spacer(rows.height);
    };

    let mut content = Column::new()
        .spacing(0)
        .push(vertical_spacer(visible.before_height));
    let row_height = row_height(icon_edge);
    for row_index in visible.start_row..visible.end_row {
        let start = row_index
            .saturating_mul(rows.column_count)
            .min(rows.entries.len());
        let end = start
            .saturating_add(rows.column_count)
            .min(rows.entries.len());
        let mut row = Row::new()
            .spacing(grid_gap(icon_edge))
            .align_y(Alignment::Start)
            .height(Length::Fixed(row_height));
        for (entry_index, entry) in rows.entries[start..end].iter().enumerate() {
            row = row.push(icon_grid_entry(
                browser,
                pane,
                rows.directory,
                start + entry_index,
                entry,
                input,
            ));
        }
        content = content.push(row);
    }
    content = content.push(vertical_spacer(visible.after_height));
    container(content)
        .padding([0, GRID_PADDING as u16])
        .width(Length::Fill)
        .height(Length::Fixed(rows.height))
        .into()
}

fn render_band<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    band: &IconGridBandLayout<'a>,
    viewport: IconGridViewport,
    parent_top: f32,
    parent_clip_bottom: f32,
    parent_input: IconGridPanelInput,
    depth: usize,
) -> Element<'a, Message> {
    let input = if parent_input == IconGridPanelInput::Interactive && band.interactive {
        IconGridPanelInput::Interactive
    } else {
        IconGridPanelInput::VisualOnly
    };
    let band_top = parent_top + band.top;
    let band_clip_bottom = (band_top + band.height).min(parent_clip_bottom);
    let panel = render_panel(
        browser,
        pane,
        &band.panel,
        viewport,
        band_top,
        band_clip_bottom,
        input,
        depth,
    );
    let icon_edge = browser.user_config().icons_icon_edge();
    let indicator_offset = GRID_PADDING
        + band.anchor_column as f32 * (tile_width(icon_edge) + grid_gap(icon_edge))
        + (tile_width(icon_edge) - PANEL_INDICATOR_SIZE) / 2.0;
    let indicator = Row::new()
        .push(Space::new().width(Length::Fixed(indicator_offset.max(0.0))))
        .push(rotated_chevron_right_view(-90.0, PANEL_INDICATOR_SIZE).style(icon_svg_style()))
        .width(Length::Fill)
        .height(Length::Fixed(GRID_PADDING));
    let surface: Element<'a, Message> = Stack::with_children([panel, indicator.into()])
        .width(Length::Fill)
        .height(Length::Fixed(band.natural_height))
        .into();
    let animated = container(surface)
        .width(Length::Fill)
        .height(Length::Fixed(band.height))
        .clip(true)
        .style(icon_grid_expansion_panel_style(depth));

    if input == IconGridPanelInput::VisualOnly {
        let blocked_band = track_file_drag_hit_test_marker(
            animated,
            FileDragHitTestMarker::BlockedDirectoryTarget { pane_id: pane.id },
        );
        return Stack::with_children([
            blocked_band,
            input_blocking_space(Length::Fill, Length::Fixed(band.height)),
        ])
        .width(Length::Fill)
        .height(Length::Fixed(band.height))
        .into();
    }

    let directory = band.directory.to_path_buf();
    track_file_drag_hit_test_marker(
        mouse_area(animated)
            .on_enter(Message::DropTargetHovered(pane.id, directory.clone()))
            .on_exit(Message::DropTargetHoverCleared(pane.id, directory.clone()))
            .on_press(Message::IconGridPanelPressed(pane.id, directory.clone()))
            .on_release(Message::DropTargetReleased(pane.id, directory.clone()))
            .on_right_press(Message::BlankAreaRightClicked(pane.id, directory.clone())),
        FileDragHitTestMarker::DirectoryTarget {
            pane_id: pane.id,
            directory,
        },
    )
}

fn icon_grid_entry<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    parent_directory: &Path,
    entry_index: usize,
    entry: &DirectoryEntry,
    input: IconGridPanelInput,
) -> Element<'a, Message> {
    let visual_state = FileEntryVisualState::from_entry_context(pane, &entry.path, false);
    let content_modifier = browser.file_entry_content_modifier(&entry.path);
    let icon_tone = visual_state.icon_tone();
    let icon_edge = browser.user_config().icons_icon_edge();
    let tile_width = tile_width(icon_edge);
    let is_directory = entry.kind == FileKind::Directory && !pane.is_trash_view;
    let disclosure = is_directory
        .then(|| browser.icon_grid_disclosure(pane.id, pane.current_dir, &entry.path))
        .flatten();
    let shows_disclosure = is_directory
        && (pane.hovered_entry == Some(&entry.path)
            || pane.is_path_selected(&entry.path)
            || disclosure.is_some());

    let base_icon: Element<'a, Message> = container(entry_thumbnail_or_icon(
        browser,
        entry,
        icon_tone,
        FileEntryIconDensity::Grid(icon_edge),
    ))
    .width(Length::Fixed(icon_edge as f32))
    .height(Length::Fixed(icon_edge as f32))
    .center_x(Length::Fixed(icon_edge as f32))
    .center_y(Length::Fixed(icon_edge as f32))
    .into();
    let icon: Element<'a, Message> = if shows_disclosure {
        let (rotation, is_open) = disclosure.unwrap_or((0.0, false));
        let disclosure_button = button(
            rotated_chevron_right_view(rotation, DISCLOSURE_ICON_SIZE).style(icon_svg_style()),
        )
        .on_press(Message::IconGridDirectoryToggled(
            pane.id,
            IconGridExpansionAnchor {
                path: entry.path.clone(),
                parent_directory: parent_directory.to_path_buf(),
                index: entry_index,
            },
        ))
        .padding(0)
        .width(Length::Fixed(DISCLOSURE_BUTTON_SIZE))
        .height(Length::Fixed(DISCLOSURE_BUTTON_SIZE))
        .style(navigation_icon_button_style());
        let label = if is_open {
            "Collapse folder"
        } else {
            "Expand folder"
        };
        let disclosure_button = tooltip(
            disclosure_button,
            container(readable_text(label).size(11))
                .padding([5, 7])
                .style(context_menu_style),
            tooltip::Position::Bottom,
        );
        let disclosure_overlay = container(disclosure_button)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom);
        Stack::with_children([base_icon, disclosure_overlay.into()])
            .width(Length::Fixed(icon_edge as f32))
            .height(Length::Fixed(icon_edge as f32))
            .into()
    } else {
        base_icon
    };

    let label: Element<'a, Message> = if pane.renaming == Some(&entry.path) {
        container(
            text_input(
                &crate::localization::translate_current("File name"),
                pane.rename_input,
            )
            .id(rename_input_id())
            .on_input(Message::RenameInputChanged)
            .on_submit(Message::RenameSelected)
            .style(entry_text_input_style(content_modifier))
            .padding(4)
            .size(label_size(icon_edge))
            .width(Length::Fill),
        )
        .height(Length::Fixed(label_height(icon_edge)))
        .clip(true)
        .into()
    } else {
        let full_name = entry.name().to_string_lossy().into_owned();
        let full_name_tooltip = container(
            readable_text(full_name.clone())
                .size(12)
                .wrapping(text::Wrapping::WordOrGlyph)
                .width(Length::Fill),
        )
        .padding([5, 7])
        .width(Length::Fixed(ICON_GRID_NAME_TOOLTIP_WIDTH))
        .style(context_menu_style);
        container(measured_middle_ellipsized_wrapped_text_with_tooltip(
            full_name,
            label_size(icon_edge),
            label_line_height(icon_edge),
            full_name_tooltip,
            ICON_GRID_NAME_TOOLTIP_DELAY,
        ))
        .style(visual_state.content_style(content_modifier))
        .width(Length::Fill)
        .height(Length::Fixed(label_height(icon_edge)))
        .into()
    };

    let tile = container(
        Column::new()
            .align_x(Alignment::Center)
            .spacing(icon_label_spacing(icon_edge))
            .push(icon)
            .push(label),
    )
    .padding(iced::Padding {
        top: tile_padding_vertical(icon_edge),
        right: tile_padding_horizontal(icon_edge),
        bottom: tile_padding_vertical(icon_edge),
        left: tile_padding_horizontal(icon_edge),
    })
    .width(Length::Fixed(tile_width))
    .height(Length::Fixed(tile_visual_height(icon_edge)));
    let tile = match visual_state.row_style_for_selection_run(None) {
        Some(style) => tile.style(style),
        None => tile,
    };
    if input == IconGridPanelInput::VisualOnly {
        return tile.into();
    }

    let area = mouse_area(tile)
        .on_enter(Message::EntryHovered(pane.id, entry.path.clone()))
        .on_exit(Message::EntryHoverCleared(pane.id, entry.path.clone()))
        .on_press(Message::FlatEntryClicked(pane.id, entry.path.clone()))
        .on_release(Message::EntryReleased(pane.id, entry.path.clone()))
        .on_right_press(Message::EntryRightClicked(pane.id, entry.path.clone()))
        .interaction(iced::mouse::Interaction::Pointer);
    let area = if is_directory {
        area.on_middle_press(Message::OpenDirectoryFromMiddleClick(
            pane.id,
            entry.path.clone(),
        ))
    } else {
        area
    };
    track_column_entry_bounds(area, pane.id, entry.path.clone())
}

fn panel_status(status: IconGridPanelStatus) -> Element<'static, Message> {
    let label = match status {
        IconGridPanelStatus::Loading => "",
        IconGridPanelStatus::Empty => "No items",
        IconGridPanelStatus::Error => "Could not load folder",
        IconGridPanelStatus::Loaded => "",
    };
    container(readable_text(label).size(ICON_GRID_LABEL_SIZE))
        .width(Length::Fill)
        .height(Length::Fixed(ICON_GRID_STATUS_HEIGHT))
        .center_x(Length::Fill)
        .center_y(Length::Fixed(ICON_GRID_STATUS_HEIGHT))
        .into()
}

fn grid_message(message: &'static str) -> Element<'static, Message> {
    container(readable_text(message).size(ICON_GRID_LABEL_SIZE))
        .padding(GRID_PADDING)
        .width(Length::Fill)
        .into()
}

fn vertical_spacer(height: f32) -> Element<'static, Message> {
    Space::new().height(Length::Fixed(height.max(0.0))).into()
}
