use std::path::Path;

use iced::widget::{button, container, mouse_area, row, Row, Space};
use iced::{Alignment, Element, Length};

use super::{
    tab_motion, tab_title_content, themed_icon, BrowserPaneView, FileBrowser, IconSymbol, IconTone,
    Message, TAB_BAR_EXPANDED_HEIGHT, TAB_CLOSE_ICON_SIZE, TAB_CLOSE_SLOT_WIDTH, TAB_FILL_PORTION,
};
use crate::appearance::{
    navigation_icon_button_style, selected_tab_item_style, tab_item_style, tab_strip_style,
};
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::model::{
    BrowserPaneId, FileDropSessionIdentity, FileDropTarget, TabDropDestination, TabFileDropTarget,
};

pub(super) fn tab_bar<'a>(
    browser: &'a FileBrowser,
    pane: BrowserPaneView<'a>,
) -> Element<'a, Message> {
    let reveal_fraction = pane.tab_bar_reveal_fraction;
    if reveal_fraction <= f32::EPSILON && pane.tabs.len() <= 1 {
        return Space::new().height(Length::Fixed(0.0)).into();
    }

    let mut tabs = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for tab in pane.tabs {
        tabs = tabs.push(tab_button(
            browser,
            pane.id,
            tab.id,
            tab.directory.as_path(),
            tab.is_trash_view,
            tab.file_drop_destination(),
            tab.id == pane.active_tab_id,
            pane.tab_width_fraction(tab.id),
            pane.tab_shift_offset(tab.id),
        ));
    }

    container(tabs)
        .height(Length::Fixed(TAB_BAR_EXPANDED_HEIGHT * reveal_fraction))
        .width(Length::Fill)
        .padding([3, 18])
        .style(tab_strip_style)
        .into()
}

fn tab_button<'a>(
    browser: &'a FileBrowser,
    pane_id: BrowserPaneId,
    tab_id: usize,
    directory: &'a Path,
    is_trash_view: bool,
    destination: TabDropDestination,
    is_active: bool,
    width_fraction: f32,
    shift_offset: f32,
) -> Element<'a, Message> {
    let target = TabFileDropTarget {
        pane_id,
        tab_id,
        destination,
    };
    let is_file_drop_target = browser.file_drop_session.as_ref().is_some_and(|session| {
        session.hovered_target.as_ref() == Some(&FileDropTarget::Tab(target.clone()))
    });
    let tone = if is_active || is_file_drop_target {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let label = row![
        Space::new().width(Length::Fixed(TAB_CLOSE_SLOT_WIDTH)),
        container(tab_title_content(directory, is_trash_view, tone)).center_x(Length::Fill),
        button(themed_icon(IconSymbol::Close, tone, TAB_CLOSE_ICON_SIZE))
            .on_press(Message::TabCloseRequested(pane_id, tab_id))
            .padding([2, 2])
            .width(Length::Fixed(TAB_CLOSE_SLOT_WIDTH))
            .style(navigation_icon_button_style()),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let tab = container(label)
        .padding([4, 8])
        .width(Length::Fill)
        .clip(true)
        .style(if is_active || is_file_drop_target {
            selected_tab_item_style
        } else {
            tab_item_style
        });
    let tab = tab_motion::translated(
        track_file_drag_hit_test_marker(
            tab,
            FileDragHitTestMarker::Tab {
                target: target.clone(),
            },
        ),
        shift_offset,
        0.0,
    );
    let iced_file_drop_gesture_id =
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| match session.identity {
                FileDropSessionIdentity::Iced(gesture_id) => Some(gesture_id),
                FileDropSessionIdentity::Wayland(_) | FileDropSessionIdentity::X11(_) => None,
            });
    let pointer_area = mouse_area(tab)
        .on_press(Message::TabPressed(pane_id, tab_id))
        .on_middle_press(Message::TabCloseRequested(pane_id, tab_id))
        .interaction(iced::mouse::Interaction::Pointer);
    let pointer_area = if let Some(gesture_id) = iced_file_drop_gesture_id {
        pointer_area
            .on_enter(Message::TabFileDropEntered(gesture_id, target.clone()))
            .on_exit(Message::TabFileDropExited(gesture_id, target.clone()))
            .on_release(Message::TabFileDropReleased(gesture_id, target))
    } else {
        pointer_area
            .on_enter(Message::TabDragEntered(pane_id, tab_id))
            .on_release(Message::TabDragFinished)
    };

    container(pointer_area)
        .width(Length::FillPortion(tab_width_portion(width_fraction)))
        .into()
}

fn tab_width_portion(intro_fraction: f32) -> u16 {
    ((TAB_FILL_PORTION as f32) * intro_fraction.clamp(0.0, 1.0))
        .round()
        .max(1.0) as u16
}
