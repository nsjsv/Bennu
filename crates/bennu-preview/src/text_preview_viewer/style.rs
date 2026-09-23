use iced::{Color, Theme};

// app-ui 的 matugen_theme 是 bennu-theme 的纯 re-export 垫片，
// 这里直接取单源，视觉取值不变。
use bennu_theme::ui_colors;

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
