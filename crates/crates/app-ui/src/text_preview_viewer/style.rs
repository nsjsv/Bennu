use iced::{Color, Theme};

use crate::matugen_theme::ui_colors;

pub(super) fn viewer_background_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_lowest
}

pub(super) fn divider_color(theme: &Theme) -> Color {
    ui_colors(theme).outline_variant
}

pub(super) fn text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface
}

pub(super) fn placeholder_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface_variant
}

pub(super) fn selection_color(theme: &Theme) -> Color {
    Color {
        a: 0.48,
        ..ui_colors(theme).primary
    }
}
