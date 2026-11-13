use std::path::{Path, PathBuf};

use iced::widget::{button, container, mouse_area, opaque, responsive, scrollable, stack, Column};
use iced::{mouse, Element, Length};

use bennu_theme::address_bar::{
    address_bar_overlay, BreadcrumbMeasurement, ElasticBreadcrumbs, ADDRESS_BAR_HEIGHT,
    ADDRESS_TEXT_SIZE, BREADCRUMB_HORIZONTAL_PADDING, BREADCRUMB_ICON_SIZE,
    BREADCRUMB_SEPARATOR_SIZE,
};

use crate::app::panes::BrowserPaneView;
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    address_bar_style, faded_button_style, faded_text_input_style, path_suggestion_item_style,
    path_suggestions_style, scale_color_alpha, selected_path_suggestion_item_style,
};
use crate::breadcrumb_drop_target_bounds::{
    track_breadcrumb_drop_target, track_breadcrumb_viewport,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::model::{
    breadcrumb_segments, displayed_address_directory, BreadcrumbSegment, BreadcrumbSegmentKind,
    BrowserLocation, BrowserPaneId, FileDropTarget, Message, ScrollbarRegion, SearchDirectoryScope,
    SearchWorkspaceState,
};
use crate::view::{icon_tone_style, themed_icon, IconTone};
use bennu_preview::panel_text::readable_text;
use bennu_theme::anchored_popup::anchored_popup;
use bennu_theme::controls::text_field;
use bennu_theme::icons::IconSymbol;
use bennu_theme::measured_text::measured_middle_ellipsized_text;
use bennu_theme::metrics::{CONTROL_PADDING_Y, FIELD_PADDING_X};

const PATH_SUGGESTION_MAX_CHARS: usize = 72;

pub(crate) fn address_input_id(pane_id: BrowserPaneId) -> iced::widget::Id {
    iced::widget::Id::from(format!("address-input-{}", pane_id.key()))
}

pub(crate) fn address_bar<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    let address_bar_directory = match pane.location {
        BrowserLocation::Directory(current_directory) => displayed_address_directory(
            current_directory,
            pane.view_mode,
            pane.deepest_open_column_directory,
        ),
        BrowserLocation::Trash => return trash_address_bar(pane.id),
        BrowserLocation::Search(search) => return search_address_bar(browser, pane, search),
    };

    let editing_fraction = pane.address_transition_fraction.clamp(0.0, 1.0);
    let breadcrumb_fraction = 1.0 - editing_fraction;
    let breadcrumbs = breadcrumb_layer(
        browser,
        pane,
        address_bar_directory,
        breadcrumb_fraction,
        pane.address_editing.is_none(),
    );

    let anchor: Element<'a, Message> = if let Some(session) = pane.address_editing {
        let input = text_field(&tr!("Path"), &session.draft)
            .id(address_input_id(pane.id))
            .on_input(move |value| Message::AddressDraftChanged(pane.id, value))
            .on_submit(Message::AddressEditingSubmitted(pane.id))
            .style(move |theme, status| faded_text_input_style(theme, status, editing_fraction))
            .width(Length::Fill);

        stack([breadcrumbs, opaque(address_bar_overlay(input))])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .into()
    } else if let Some(snapshot) = pane.address_exit_snapshot {
        let snapshot = address_bar_overlay(address_exit_snapshot(snapshot, editing_fraction));
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

/// 搜索结果：搜索图标 + 搜的文件夹路径（点任一层退出搜索去那里）；搜所有已索引位置时
/// 只显示文字。
fn search_address_bar<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
    search: &'a SearchWorkspaceState,
) -> Element<'a, Message> {
    let icon = container(themed_icon(
        IconSymbol::Search,
        IconTone::Normal,
        BREADCRUMB_ICON_SIZE,
    ))
    .padding(iced::padding::left(10));
    let place: Element<'a, Message> =
        if search.root.selected_scope() == SearchDirectoryScope::AllIndexedLocations {
            container(readable_text(tr!("All indexed locations")))
                .padding([7, 8])
                .width(Length::Fill)
                .into()
        } else {
            breadcrumb_layer(browser, pane, search.root.path(), 1.0, true)
        };
    address_bar_surface(
        iced::widget::row![icon, place]
            .align_y(iced::Alignment::Center)
            .width(Length::Fill)
            .into(),
        pane.id,
    )
}

fn trash_address_bar<'a>(pane_id: BrowserPaneId) -> Element<'a, Message> {
    let content = container(
        iced::widget::row![
            themed_icon(IconSymbol::Trash, IconTone::Normal, BREADCRUMB_ICON_SIZE),
            readable_text(tr!("Trash")),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .padding([7, 10])
    .width(Length::Fill)
    .height(Length::Fixed(ADDRESS_BAR_HEIGHT));

    address_bar_surface(content.into(), pane_id)
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
    address_bar_directory: &Path,
    opacity: f32,
    registers_drop_targets: bool,
) -> Element<'a, Message> {
    let pane_id = pane.id;
    let segments = breadcrumb_segments(address_bar_directory, &browser.home_dir);
    let active_drop_target = browser.file_drop_session.as_ref().and_then(|session| {
        match session.hovered_target.as_ref() {
            Some(FileDropTarget::Directory(directory)) => Some(directory.clone()),
            Some(
                FileDropTarget::Trash
                | FileDropTarget::LocalSend
                | FileDropTarget::SidebarBookmarkSlot(_)
                | FileDropTarget::Tab(_),
            )
            | None => None,
        }
    });
    responsive(move |viewport_size| {
        breadcrumb_scrollable(
            pane_id,
            segments.clone(),
            opacity,
            viewport_size.width,
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
        // 面包屑不画滚动条：路径下面多一根线只是噪音，滚轮照样横向滚动。
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::hidden(),
        ))
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .on_scroll(|_| Message::AddressBarScrolled);
    if registers_drop_targets {
        track_breadcrumb_viewport(scroller, pane_id)
    } else {
        scroller.into()
    }
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
    let item = container(readable_text(label).width(Length::Fill))
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
    container(measured_middle_ellipsized_text(snapshot, ADDRESS_TEXT_SIZE))
        .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
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
    use bennu_theme::controls::text_field_style;
    use bennu_theme::styles::address_bar_mask_style;
    use iced::widget::text_input;
    use iced::Theme;

    /// 外框、编辑层、搜索框是同一个半透明底色，由渲染器叠出同一个颜色；
    /// 编辑层下面垫的是不透明窗口底色，面包屑透不过来。
    #[test]
    fn address_layers_share_the_field_tint_over_an_opaque_mask() {
        for theme in [Theme::Light, Theme::Dark] {
            let search = text_field_style(&theme, text_input::Status::Active);
            let frame = address_bar_style(&theme);
            let editing = faded_text_input_style(&theme, text_input::Status::Active, 1.0);

            assert_eq!(frame.background, Some(search.background));
            assert_eq!(editing.background, search.background);

            let Some(iced::Background::Color(mask)) = address_bar_mask_style(&theme).background
            else {
                panic!("the mask under the editing layer must be a solid color");
            };
            assert_eq!(mask.a, 1.0, "a translucent mask shows the breadcrumbs");
        }
    }
}
