use std::path::{Path, PathBuf};

use iced::widget::{
    button, container, mouse_area, opaque, responsive, scrollable, stack, text_input, Column,
};
use iced::{mouse, Element, Length};

use bennu_theme::address_bar::{
    BreadcrumbMeasurement, ElasticBreadcrumbs, ADDRESS_BAR_HEIGHT, ADDRESS_TEXT_SIZE,
    BREADCRUMB_HORIZONTAL_PADDING, BREADCRUMB_ICON_SIZE, BREADCRUMB_SEPARATOR_SIZE,
};

use crate::anchored_popup::anchored_popup;
use crate::app::panes::BrowserPaneView;
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    address_bar_style, enhanced_horizontal_scrollbar_direction, enhanced_scrollbar_style,
    faded_button_style, faded_text_input_style, path_suggestion_item_style, path_suggestions_style,
    scale_color_alpha, selected_path_suggestion_item_style,
};
use crate::breadcrumb_drop_target_bounds::{
    track_breadcrumb_drop_target, track_breadcrumb_viewport,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::measured_middle_ellipsized_text::measured_middle_ellipsized_text;
use crate::model::{
    breadcrumb_segments, BreadcrumbSegment, BreadcrumbSegmentKind, BrowserPaneId, FileDropTarget,
    Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, TRASH_LOCATION_LABEL,
};
use crate::typography::readable_text;
use crate::view::{icon_tone_style, themed_icon, IconTone};

const PATH_SUGGESTION_MAX_CHARS: usize = 72;

pub(crate) fn address_input_id(pane_id: BrowserPaneId) -> iced::widget::Id {
    iced::widget::Id::from(format!("address-input-{}", pane_id.key()))
}

pub(crate) fn address_bar<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    if pane.is_trash_view {
        let content = container(
            iced::widget::row![
                themed_icon(IconSymbol::Trash, IconTone::Normal, BREADCRUMB_ICON_SIZE),
                readable_text(TRASH_LOCATION_LABEL).size(16),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT));

        return address_bar_surface(content.into(), pane.id);
    }

    let editing_fraction = pane.address_transition_fraction.clamp(0.0, 1.0);
    let breadcrumb_fraction = 1.0 - editing_fraction;
    let breadcrumbs = breadcrumb_layer(
        browser,
        pane,
        breadcrumb_fraction,
        pane.address_editing.is_none(),
    );

    let anchor: Element<'a, Message> = if let Some(session) = pane.address_editing {
        let input = text_input(
            &crate::localization::translate_current("Path"),
            &session.draft,
        )
        .id(address_input_id(pane.id))
        .on_input(move |value| Message::AddressDraftChanged(pane.id, value))
        .on_submit(Message::AddressEditingSubmitted(pane.id))
        .padding([7, 10])
        .size(16)
        .style(move |theme, status| faded_text_input_style(theme, status, editing_fraction))
        .width(Length::Fill);

        stack([breadcrumbs, opaque(input)])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .into()
    } else if let Some(snapshot) = pane.address_exit_snapshot {
        let snapshot = address_exit_snapshot(snapshot, editing_fraction);
        stack([breadcrumbs, snapshot])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .into()
    } else {
        breadcrumbs
    };

    let popup = pane.address_editing.and_then(|session| {
        (!session.suggestions.is_empty()).then(|| path_suggestions_panel(pane.id, session))
    });

    address_bar_surface(anchored_popup(anchor, popup), pane.id)
}

fn address_bar_surface<'a>(
    content: Element<'a, Message>,
    pane_id: BrowserPaneId,
) -> Element<'a, Message> {
    mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .style(address_bar_style),
    )
    .on_press(Message::AddressEditingRequested(pane_id))
    .interaction(mouse::Interaction::Text)
    .into()
}

fn breadcrumb_layer<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    opacity: f32,
    registers_drop_targets: bool,
) -> Element<'a, Message> {
    let pane_id = pane.id;
    let segments = breadcrumb_segments(pane.address_bar_directory(), &browser.home_dir);
    let active_drop_target = browser.file_drop_session.as_ref().and_then(|session| {
        match session.hovered_target.as_ref() {
            Some(FileDropTarget::Directory(directory)) => Some(directory.clone()),
            Some(
                FileDropTarget::Trash
                | FileDropTarget::SidebarBookmarkSlot(_)
                | FileDropTarget::Tab(_),
            )
            | None => None,
        }
    });
    let region = ScrollbarRegion::AddressBar(pane_id);
    let scrollbar_visibility = browser.scrollbar_visibility_for(&region);

    responsive(move |viewport_size| {
        breadcrumb_scrollable(
            pane_id,
            segments.clone(),
            opacity,
            viewport_size.width,
            scrollbar_visibility,
            browser.scrollbar_viewport_for(&region),
            active_drop_target.clone(),
            registers_drop_targets,
        )
    })
    .width(Length::Fill)
    .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
    .into()
}

fn breadcrumb_scrollable<'a>(
    pane_id: BrowserPaneId,
    segments: Vec<BreadcrumbSegment>,
    opacity: f32,
    viewport_width: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    active_drop_target: Option<PathBuf>,
    registers_drop_targets: bool,
) -> Element<'a, Message> {
    let region = ScrollbarRegion::AddressBar(pane_id);
    let breadcrumbs = elastic_breadcrumbs(
        pane_id,
        segments,
        opacity,
        viewport_width,
        active_drop_target,
        registers_drop_targets,
    );
    let scroller = scrollable(smooth_scroll_content(breadcrumbs, region.clone()))
        .id(smooth_scroll_id(&region))
        .direction(enhanced_horizontal_scrollbar_direction(
            scrollbar_visibility,
            8.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .on_scroll(scrollbar_on_scroll(region.clone(), move |_| {
            Message::AddressBarScrolled(pane_id)
        }));
    let scroller: Element<'a, Message> = if registers_drop_targets {
        track_breadcrumb_viewport(scroller, pane_id)
    } else {
        scroller.into()
    };

    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Horizontal,
        8.0,
    )
}

fn elastic_breadcrumbs<'a>(
    pane_id: BrowserPaneId,
    segments: Vec<BreadcrumbSegment>,
    opacity: f32,
    viewport_width: f32,
    active_drop_target: Option<PathBuf>,
    registers_drop_targets: bool,
) -> Element<'a, Message> {
    let mut children = Vec::with_capacity(segments.len().saturating_mul(2).saturating_sub(1));
    let mut measurements = Vec::with_capacity(segments.len());

    for (index, segment) in segments.into_iter().enumerate() {
        if index > 0 {
            children.push(faded_icon(
                IconSymbol::ChevronRight,
                BREADCRUMB_SEPARATOR_SIZE,
                opacity,
            ));
        }

        let display_text = segment.display_text();
        let content: Element<'a, Message> = match segment.kind {
            BreadcrumbSegmentKind::Home => {
                faded_icon(IconSymbol::House, BREADCRUMB_ICON_SIZE, opacity)
            }
            BreadcrumbSegmentKind::Root | BreadcrumbSegmentKind::Name(_) => {
                measured_middle_ellipsized_text(display_text.clone(), ADDRESS_TEXT_SIZE)
            }
        };
        let target = segment.target;
        let is_drop_target = active_drop_target.as_ref() == Some(&target);
        let segment_button = button(content)
            .padding([7, BREADCRUMB_HORIZONTAL_PADDING as u16])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .style(move |theme, status| faded_button_style(theme, status, opacity, is_drop_target))
            .on_press(Message::BreadcrumbSegmentPressed(pane_id, target.clone()));
        let segment_target = mouse_area(segment_button)
            .on_enter(Message::BreadcrumbDropTargetHovered(
                pane_id,
                target.clone(),
            ))
            .on_exit(Message::BreadcrumbDropTargetHoverCleared(
                pane_id,
                target.clone(),
            ))
            .on_release(Message::DropTargetReleased(pane_id, target.clone()));
        let segment_target: Element<'a, Message> = if registers_drop_targets {
            track_breadcrumb_drop_target(segment_target, pane_id, target)
        } else {
            segment_target.into()
        };

        measurements.push(match segment.kind {
            BreadcrumbSegmentKind::Home => BreadcrumbMeasurement::Home,
            BreadcrumbSegmentKind::Root | BreadcrumbSegmentKind::Name(_) => {
                BreadcrumbMeasurement::Text(display_text)
            }
        });
        children.push(segment_target);
    }

    Element::new(ElasticBreadcrumbs::new(
        children,
        measurements,
        viewport_width,
    ))
}

fn path_suggestions_panel<'a>(
    pane_id: BrowserPaneId,
    session: &'a crate::model::AddressEditingSession,
) -> Element<'a, Message> {
    let mut suggestions = Column::new().spacing(3).padding(4);
    for (index, suggestion) in session.suggestions.iter().enumerate() {
        suggestions = suggestions.push(path_suggestion_row(
            pane_id,
            suggestion,
            session.suggestion_selection == Some(index),
        ));
    }

    container(suggestions)
        .width(Length::Fill)
        .style(path_suggestions_style)
        .into()
}

fn path_suggestion_row(
    pane_id: BrowserPaneId,
    path: &Path,
    is_selected: bool,
) -> Element<'_, Message> {
    let label = path.to_string_lossy();
    let label = format_middle_ellipsized_text(label.as_ref(), PATH_SUGGESTION_MAX_CHARS);
    let item = container(readable_text(label).size(13).width(Length::Fill))
        .padding([5, 8])
        .width(Length::Fill);
    let item = if is_selected {
        item.style(selected_path_suggestion_item_style)
    } else {
        item.style(path_suggestion_item_style)
    };

    mouse_area(item)
        .on_press(Message::AddressSuggestionSelected(
            pane_id,
            path.to_path_buf(),
        ))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn address_exit_snapshot(snapshot: &str, opacity: f32) -> Element<'_, Message> {
    container(measured_middle_ellipsized_text(snapshot, 16))
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .style(move |theme| {
            let input_style =
                faded_text_input_style(theme, iced::widget::text_input::Status::Active, opacity);
            iced::widget::container::Style {
                background: Some(input_style.background),
                text_color: Some(input_style.value),
                border: input_style.border,
                ..iced::widget::container::Style::default()
            }
        })
        .into()
}

fn faded_icon<'a>(symbol: IconSymbol, size: f32, opacity: f32) -> Element<'a, Message> {
    symbol
        .view(size)
        .style(move |theme, status| {
            let mut style = icon_tone_style(IconTone::Normal)(theme, status);
            style.color = style.color.map(|color| scale_color_alpha(color, opacity));
            style
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Color, Theme};

    #[test]
    fn address_input_overlay_masks_breadcrumbs_without_drawing_a_second_border() {
        for theme in [Theme::Light, Theme::Dark] {
            let frame_style = address_bar_style(&theme);
            let input_style =
                faded_text_input_style(&theme, iced::widget::text_input::Status::Active, 1.0);

            assert_eq!(Some(input_style.background), frame_style.background);
            assert_eq!(input_style.border.radius, frame_style.border.radius);
            assert_eq!(input_style.border.width, 0.0);
            assert_eq!(input_style.border.color, Color::TRANSPARENT);
        }
    }
}
