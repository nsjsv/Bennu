use iced::widget::{button, Button, Column};
use iced::{Background, Border, Color, Element, Length, Theme};

use crate::appearance::{base_text_color, subtle_border_color};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

const ACTION_CHOICE_HEIGHT: f32 = 58.0;

pub(super) fn action_choice_row(
    title: String,
    description: String,
    message: Message,
) -> Element<'static, Message> {
    action_choice_button(title, description)
        .on_press(message)
        .into()
}

pub(super) fn action_choice_button(title: String, description: String) -> Button<'static, Message> {
    let labels = Column::new()
        .spacing(2)
        .push(readable_text(title).width(Length::Fill))
        .push(
            readable_text(description)
                .size(CAPTION_SIZE)
                .width(Length::Fill),
        );

    button(labels)
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Fixed(ACTION_CHOICE_HEIGHT))
        .style(selectable_choice_button_style(false))
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
