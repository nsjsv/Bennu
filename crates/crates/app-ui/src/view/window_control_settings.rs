use iced::widget::{button, container, row, Column};
use iced::{Alignment, Border, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::{context_menu_button_style, subtle_border_color};
use crate::icons::IconSymbol;
use crate::model::{
    Message, WindowChromeLayout, WindowControlKind, WindowControlMoveDirection,
    WindowControlPlacement, WindowControlSide,
};
use crate::typography::readable_text;

use super::option_controls::{secondary_action_button, segmented_choice_row, SegmentedChoice};
use super::settings_group::{info_setting_row, muted_setting_text};
use super::toggle_switch::switch_control;
use super::{themed_icon, IconTone};

const WINDOW_CONTROL_ROW_HEIGHT: f32 = 46.0;
const WINDOW_CONTROL_SIDE_WIDTH: f32 = 150.0;
const WINDOW_CONTROL_VISIBILITY_WIDTH: f32 = 126.0;
// Two stacked 12px arrows + padding + spacing fill the 34px content box of a
// 46px row without stretching it.
const WINDOW_CONTROL_ARROW_ICON_SIZE: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowControlVisibilityControl {
    Toggle { visible: bool },
    AlwaysVisible,
}

pub(super) fn window_control_settings_row(browser: &FileBrowser) -> Element<'_, Message> {
    let mut content = Column::new()
        .spacing(8)
        .width(Length::Fill)
        .push(muted_setting_text("Window layout", 12))
        .push(window_layout_selector(browser));
    for side in WindowControlSide::ALL {
        if let Some(group) = side_control_group(browser, side) {
            content = content.push(group);
        }
    }
    info_setting_row(
        content
            .push(secondary_action_button(
                "Restore defaults",
                Message::WindowControlsReset,
            ))
            .into(),
    )
}

fn window_layout_selector(browser: &FileBrowser) -> Element<'static, Message> {
    let selected = browser.user_config().window_controls.layout();
    segmented_choice_row(
        WindowChromeLayout::ALL
            .into_iter()
            .map(|layout| SegmentedChoice {
                label: layout.label(),
                selected: layout == selected,
                message: Message::WindowChromeLayoutSelected(layout),
                tooltip: None,
            })
            .collect(),
    )
}

// A side with no placements renders nothing at all — not even its header.
// Rows on the other side keep the Left/Right selector, so an emptied side
// stays reachable and reappears as soon as a control moves back.
fn side_control_group(
    browser: &FileBrowser,
    side: WindowControlSide,
) -> Option<Element<'static, Message>> {
    let placements = browser
        .user_config()
        .window_controls
        .placements_on(side)
        .collect::<Vec<_>>();
    if placements.is_empty() {
        return None;
    }

    let last_index = placements.len() - 1;
    let mut controls = Column::new()
        .spacing(5)
        .width(Length::Fill)
        .push(muted_setting_text(
            match side {
                WindowControlSide::Left => "Left side",
                WindowControlSide::Right => "Right side",
            },
            12,
        ));
    for (index, placement) in placements.into_iter().enumerate() {
        controls = controls.push(window_control_row(
            placement,
            // Boundary rows hide their arrow instead of rendering a
            // click that could never change anything.
            index > 0,
            index < last_index,
        ));
    }
    Some(controls.into())
}

fn window_control_row(
    placement: WindowControlPlacement,
    can_move_up: bool,
    can_move_down: bool,
) -> Element<'static, Message> {
    let kind = placement.kind();
    let content = row![
        reorder_arrows(kind, can_move_up, can_move_down),
        readable_text(kind.label()).size(12).width(Length::Fill),
        visibility_control(placement),
        side_selector(placement),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    container(content)
        .padding([6, 8])
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_CONTROL_ROW_HEIGHT))
        .style(window_control_row_style)
        .into()
}

fn reorder_arrows(
    kind: WindowControlKind,
    can_move_up: bool,
    can_move_down: bool,
) -> Element<'static, Message> {
    let mut arrows = Column::new().spacing(1);
    if can_move_up {
        arrows = arrows.push(reorder_arrow_button(
            kind,
            WindowControlMoveDirection::Up,
            IconSymbol::ArrowUp,
        ));
    }
    if can_move_down {
        arrows = arrows.push(reorder_arrow_button(
            kind,
            WindowControlMoveDirection::Down,
            IconSymbol::ArrowDown,
        ));
    }
    arrows.into()
}

fn reorder_arrow_button(
    kind: WindowControlKind,
    direction: WindowControlMoveDirection,
    symbol: IconSymbol,
) -> Element<'static, Message> {
    button(
        container(themed_icon(
            symbol,
            IconTone::Normal,
            WINDOW_CONTROL_ARROW_ICON_SIZE,
        ))
        .padding(1)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink),
    )
    // Both arrows must fit the 34px content box of a 46px row: with the
    // 1px button border the pair is 2 * (12 + 2 + 2) + 1 spacing = 33px,
    // so the button's own 5px default padding has to go.
    .padding(0)
    .on_press(Message::WindowControlMoveRequested(kind, direction))
    .style(context_menu_button_style())
    .into()
}

fn visibility_control(placement: WindowControlPlacement) -> Element<'static, Message> {
    match window_control_visibility_control(placement) {
        WindowControlVisibilityControl::AlwaysVisible => {
            container(readable_text("Always visible").size(11))
                .width(Length::Fixed(WINDOW_CONTROL_VISIBILITY_WIDTH))
                .center_x(Length::Fill)
                .into()
        }
        WindowControlVisibilityControl::Toggle { visible } => {
            let content = row![
                readable_text("Show").size(11).width(Length::Fill),
                switch_control(visible),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            button(container(content).padding([3, 6]).width(Length::Fill))
                .on_press(Message::WindowControlVisibilityToggled(placement.kind()))
                .width(Length::Fixed(WINDOW_CONTROL_VISIBILITY_WIDTH))
                .style(context_menu_button_style())
                .into()
        }
    }
}

fn window_control_visibility_control(
    placement: WindowControlPlacement,
) -> WindowControlVisibilityControl {
    if placement.kind() == WindowControlKind::Close {
        WindowControlVisibilityControl::AlwaysVisible
    } else {
        WindowControlVisibilityControl::Toggle {
            visible: placement.visibility().is_visible(),
        }
    }
}

fn side_selector(placement: WindowControlPlacement) -> Element<'static, Message> {
    container(segmented_choice_row(
        WindowControlSide::ALL
            .into_iter()
            .map(|side| SegmentedChoice {
                label: side.label(),
                selected: side == placement.side(),
                message: Message::WindowControlSideSelected(placement.kind(), side),
                tooltip: None,
            })
            .collect(),
    ))
    .width(Length::Fixed(WINDOW_CONTROL_SIDE_WIDTH))
    .into()
}

fn window_control_row_style(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowControlsConfig;

    #[test]
    fn default_rows_use_two_toggles_and_an_always_visible_close_control() {
        let config = WindowControlsConfig::default();
        let presentations = config
            .placements()
            .iter()
            .copied()
            .map(window_control_visibility_control)
            .collect::<Vec<_>>();

        assert_eq!(
            presentations,
            vec![
                WindowControlVisibilityControl::Toggle { visible: true },
                WindowControlVisibilityControl::Toggle { visible: true },
                WindowControlVisibilityControl::AlwaysVisible,
            ]
        );
    }
}
