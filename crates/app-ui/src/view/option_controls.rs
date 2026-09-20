use iced::widget::{button, container, row, tooltip, Button, Column};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::appearance::{base_text_color, muted_text_color, subtle_border_color};
use bennu_theme::styles::primary_action_button_style;
// 破坏性确认按钮样式迁至共享库；以重导出保持 option_controls 的既有入口。
use crate::matugen_theme::ui_colors;
use crate::model::Message;
use crate::typography::readable_text;
pub(super) use bennu_theme::styles::destructive_confirmation_button_style;

const SEGMENTED_CHOICE_HEIGHT: f32 = 30.0;
const ACTION_CHOICE_HEIGHT: f32 = 58.0;

pub(super) struct SegmentedChoice {
    pub(super) label: &'static str,
    pub(super) selected: bool,
    pub(super) message: Message,
    /// 悬停补充说明（如"上次位置"档的完整路径）；None = 无。
    pub(super) tooltip: Option<String>,
}

pub(super) fn segmented_choice_row(choices: Vec<SegmentedChoice>) -> Element<'static, Message> {
    let mut options = row![].spacing(0).align_y(Alignment::Center);
    for choice in choices {
        options = options.push(segmented_choice_button(choice));
    }

    container(options)
        .padding(2)
        .width(Length::Fill)
        .style(segmented_choice_group_style)
        .into()
}

pub(super) fn inactive_segmented_choice_row(
    choices: Vec<SegmentedChoice>,
) -> Element<'static, Message> {
    let mut options = row![].spacing(0).align_y(Alignment::Center);
    for choice in choices {
        let label = container(readable_text(choice.label).size(12))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);
        options = options.push(
            button(label)
                .width(Length::FillPortion(1))
                .height(Length::Fixed(SEGMENTED_CHOICE_HEIGHT))
                .padding([4, 8])
                .style(segmented_choice_button_style(choice.selected)),
        );
    }

    container(options)
        .padding(2)
        .width(Length::Fill)
        .style(segmented_choice_group_style)
        .into()
}
pub(super) fn action_choice_row(
    title: &'static str,
    description: &'static str,
    message: Message,
) -> Element<'static, Message> {
    action_choice_button(title, description)
        .on_press(message)
        .into()
}

pub(super) fn action_choice_button(
    title: &'static str,
    description: &'static str,
) -> Button<'static, Message> {
    let labels = Column::new()
        .spacing(2)
        .push(readable_text(title).size(12).width(Length::Fill))
        .push(readable_text(description).size(11).width(Length::Fill));

    button(labels)
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Fixed(ACTION_CHOICE_HEIGHT))
        .style(selectable_choice_button_style(false))
}

pub(super) fn primary_action_button(
    label: &'static str,
    message: Message,
) -> Button<'static, Message> {
    button(readable_text(label).size(12))
        .on_press(message)
        .padding([6, 12])
        .style(primary_action_button_style())
}

pub(super) fn inactive_primary_action_button(label: &'static str) -> Button<'static, Message> {
    button(readable_text(label).size(12))
        .padding([6, 12])
        .style(primary_action_button_style())
}

pub(super) fn secondary_action_button(
    label: &'static str,
    message: Message,
) -> Button<'static, Message> {
    button(readable_text(label).size(12))
        .on_press(message)
        .padding([6, 12])
        .style(secondary_action_button_style())
}

fn segmented_choice_button(choice: SegmentedChoice) -> Element<'static, Message> {
    let label = container(readable_text(choice.label).size(12))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    let button = button(label)
        .on_press(choice.message)
        .width(Length::FillPortion(1))
        .height(Length::Fixed(SEGMENTED_CHOICE_HEIGHT))
        .padding([4, 8])
        .style(segmented_choice_button_style(choice.selected));
    match choice.tooltip {
        Some(text) => tooltip(
            button,
            container(readable_text(text).size(11))
                .padding([5, 7])
                .style(segmented_choice_tooltip_style),
            tooltip::Position::Bottom,
        )
        .into(),
        None => button.into(),
    }
}

fn segmented_choice_tooltip_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

fn segmented_choice_group_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub(super) fn segmented_choice_button_style(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        let background = segmented_choice_background(theme, status, selected);
        let border_color = if selected {
            accent_border_color(theme)
        } else {
            subtle_border_color(theme)
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color: if selected {
                selected_text_color(theme)
            } else {
                base_text_color(theme)
            },
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn selectable_choice_button_style(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        let background = if selected {
            match status {
                button::Status::Hovered => selected_hover_background_color(theme),
                button::Status::Pressed => selected_pressed_background_color(theme),
                _ => selected_background_color(theme),
            }
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => hover_background_color(theme),
                button::Status::Active | button::Status::Disabled => panel_background_color(theme),
            }
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color: if selected {
                selected_text_color(theme)
            } else {
                base_text_color(theme)
            },
            border: Border {
                color: if selected {
                    accent_border_color(theme)
                } else {
                    subtle_border_color(theme)
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub(super) fn secondary_action_button_style() -> fn(&Theme, button::Status) -> button::Style {
    secondary_action_button_appearance
}

fn secondary_action_button_appearance(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => hover_background_color(theme),
        button::Status::Active | button::Status::Disabled => panel_background_color(theme),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 7.0.into(),
        },
        ..button::Style::default()
    }
}

fn segmented_choice_background(theme: &Theme, status: button::Status, selected: bool) -> Color {
    match (selected, status) {
        (true, button::Status::Hovered) => selected_hover_background_color(theme),
        (true, button::Status::Pressed) => selected_pressed_background_color(theme),
        (true, _) => selected_background_color(theme),
        (false, button::Status::Hovered) | (false, button::Status::Pressed) => {
            hover_background_color(theme)
        }
        (false, _) => panel_background_color(theme),
    }
}

fn accent_border_color(theme: &Theme) -> Color {
    ui_colors(theme).primary
}

pub(super) fn selected_background_color(theme: &Theme) -> Color {
    ui_colors(theme).primary_container
}

pub(super) fn selected_hover_background_color(theme: &Theme) -> Color {
    Color {
        a: 0.9,
        ..ui_colors(theme).primary_container
    }
}

pub(super) fn selected_pressed_background_color(theme: &Theme) -> Color {
    Color {
        a: 0.8,
        ..ui_colors(theme).primary_container
    }
}

pub(super) fn selected_text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_primary_container
}

pub(super) fn panel_background_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container
}

pub(super) fn hover_background_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_high
}
