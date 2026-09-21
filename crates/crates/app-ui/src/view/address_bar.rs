use std::path::{Path, PathBuf};

use iced::advanced::{layout, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::widget::{
    button, container, mouse_area, opaque, responsive, scrollable, stack, text_input, Column,
};
use iced::{mouse, Background, Color, Element, Event, Length, Point, Rectangle, Size, Theme};

use crate::anchored_popup::anchored_popup;
use crate::app::panes::BrowserPaneView;
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    address_bar_style, enhanced_horizontal_scrollbar_direction, enhanced_scrollbar_style,
    navigation_text_input_style, path_suggestion_item_style, path_suggestions_style,
    selected_path_suggestion_item_style, transparent_button_style,
};
use crate::breadcrumb_drop_target_bounds::{
    track_breadcrumb_drop_target, track_breadcrumb_viewport,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::measured_middle_ellipsized_text::{
    measured_middle_ellipsized_text, measured_text_natural_width,
};
use crate::model::{
    allocate_breadcrumb_widths, breadcrumb_segments, BreadcrumbSegment, BreadcrumbSegmentKind,
    BrowserPaneId, FileDropTarget, Message, ScrollbarRegion, ScrollbarViewport,
    ScrollbarVisibility, TRASH_LOCATION_LABEL,
};
use crate::typography::readable_text;
use crate::view::{icon_tone_style, themed_icon, IconTone};

const ADDRESS_BAR_HEIGHT: f32 = 34.0;
const ADDRESS_TEXT_SIZE: u32 = 14;
const BREADCRUMB_ICON_SIZE: f32 = 16.0;
const BREADCRUMB_SEPARATOR_SIZE: f32 = 13.0;
const BREADCRUMB_SEPARATOR_WIDTH: f32 = 17.0;
const BREADCRUMB_HOME_WIDTH: f32 = 30.0;
const BREADCRUMB_HORIZONTAL_PADDING: f32 = 7.0;
const BREADCRUMB_MINIMUM_TEXT_WIDTH: f32 = 58.0;
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

    Element::new(ElasticBreadcrumbs {
        children,
        measurements,
        viewport_width,
    })
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

fn faded_button_style(
    theme: &Theme,
    status: iced::widget::button::Status,
    opacity: f32,
    is_drop_target: bool,
) -> iced::widget::button::Style {
    let mut style = transparent_button_style()(theme, status);
    if is_drop_target {
        let target_style = selected_path_suggestion_item_style(theme);
        style.background = target_style.background;
        if let Some(text_color) = target_style.text_color {
            style.text_color = text_color;
        }
        style.border = target_style.border;
    }
    style.text_color = scale_color_alpha(style.text_color, opacity);
    style.border.color = scale_color_alpha(style.border.color, opacity);
    style.background = style
        .background
        .map(|background| scale_background_alpha(background, opacity));
    style
}

fn faded_text_input_style(
    theme: &Theme,
    status: iced::widget::text_input::Status,
    opacity: f32,
) -> iced::widget::text_input::Style {
    let mut style = navigation_text_input_style(theme, status);
    style.border.width = 0.0;
    style.border.color = Color::TRANSPARENT;
    style.icon = scale_color_alpha(style.icon, opacity);
    style.placeholder = scale_color_alpha(style.placeholder, opacity);
    style.value = scale_color_alpha(style.value, opacity);
    style.selection = scale_color_alpha(style.selection, opacity);
    style
}

fn scale_background_alpha(background: Background, opacity: f32) -> Background {
    match background {
        Background::Color(color) => Background::Color(scale_color_alpha(color, opacity)),
        Background::Gradient(gradient) => Background::Gradient(gradient),
    }
}

fn scale_color_alpha(color: iced::Color, opacity: f32) -> iced::Color {
    iced::Color {
        a: color.a * opacity.clamp(0.0, 1.0),
        ..color
    }
}

enum BreadcrumbMeasurement {
    Home,
    Text(String),
}

struct ElasticBreadcrumbs<'a> {
    children: Vec<Element<'a, Message>>,
    measurements: Vec<BreadcrumbMeasurement>,
    viewport_width: f32,
}

impl Widget<Message, Theme, iced::Renderer> for ElasticBreadcrumbs<'_> {
    fn children(&self) -> Vec<widget::Tree> {
        self.children.iter().map(widget::Tree::new).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Fixed(ADDRESS_BAR_HEIGHT))
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let natural_widths = self
            .measurements
            .iter()
            .map(|measurement| match measurement {
                BreadcrumbMeasurement::Home => BREADCRUMB_HOME_WIDTH,
                BreadcrumbMeasurement::Text(label) => {
                    measured_text_natural_width(renderer, label, ADDRESS_TEXT_SIZE)
                        + BREADCRUMB_HORIZONTAL_PADDING * 2.0
                }
            })
            .collect::<Vec<_>>();
        let minimum_widths = self
            .measurements
            .iter()
            .zip(&natural_widths)
            .map(|(measurement, natural_width)| match measurement {
                BreadcrumbMeasurement::Home => *natural_width,
                BreadcrumbMeasurement::Text(_) => natural_width.min(BREADCRUMB_MINIMUM_TEXT_WIDTH),
            })
            .collect::<Vec<_>>();
        let separator_total_width =
            BREADCRUMB_SEPARATOR_WIDTH * self.measurements.len().saturating_sub(1) as f32;
        let allocation = allocate_breadcrumb_widths(
            &natural_widths,
            &minimum_widths,
            separator_total_width,
            self.viewport_width,
        );

        let mut segment_index = 0usize;
        let mut child_offset_x = 0.0;
        let mut positioned_nodes = Vec::with_capacity(self.children.len());
        for (child_index, (child, child_tree)) in
            self.children.iter_mut().zip(&mut tree.children).enumerate()
        {
            let child_width = if child_index % 2 == 0 {
                let width = allocation.segment_widths[segment_index];
                segment_index += 1;
                width
            } else {
                BREADCRUMB_SEPARATOR_WIDTH
            };
            let child_limits = layout::Limits::new(
                Size::new(child_width, ADDRESS_BAR_HEIGHT),
                Size::new(child_width, ADDRESS_BAR_HEIGHT),
            );
            let child_node = child
                .as_widget_mut()
                .layout(child_tree, renderer, &child_limits)
                .move_to(Point::new(child_offset_x, 0.0));
            child_offset_x += child_width;
            positioned_nodes.push(child_node);
        }

        let resolved_height = limits
            .resolve(
                Length::Shrink,
                Length::Fixed(ADDRESS_BAR_HEIGHT),
                Size::ZERO,
            )
            .height;
        layout::Node::with_children(
            Size::new(allocation.content_width, resolved_height),
            positioned_nodes,
        )
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, child_tree), child_layout) in self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                child
                    .as_widget_mut()
                    .operate(child_tree, child_layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((child, child_tree), child_layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                child_tree,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, child_tree), child_layout)| {
                child.as_widget().mouse_interaction(
                    child_tree,
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, child_tree), child_layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            child.as_widget().draw(
                child_tree,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
