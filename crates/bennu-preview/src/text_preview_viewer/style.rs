use iced::{Color, Theme};

use bennu_theme::ui_colors;

pub(super) fn viewer_background_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_lowest
}

/// 行号栏比正文底色深一级，不再画分隔线。
pub(super) fn gutter_background_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_low
}

pub(super) fn text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface
}

pub(super) fn placeholder_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface_variant
}

pub(super) fn line_number_color(theme: &Theme) -> Color {
    Color {
        a: 0.6,
        ..ui_colors(theme).on_surface_variant
    }
}

pub(super) fn current_line_number_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface
}

pub(super) fn selection_color(theme: &Theme) -> Color {
    Color {
        a: 0.32,
        ..ui_colors(theme).primary
    }
}
