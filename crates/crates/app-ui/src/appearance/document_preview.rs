use iced::widget::container;
use iced::{Background, Border, Theme};

use super::subtle_border_color;
use crate::matugen_theme::ui_colors;

pub(crate) fn document_page_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container_lowest)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    }
}
