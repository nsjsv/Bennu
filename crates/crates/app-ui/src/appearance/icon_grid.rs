use iced::{Background, Theme};

use super::container;
use crate::matugen_theme::ui_colors;

pub(crate) fn icon_grid_expansion_panel_style(
    depth: usize,
) -> impl Fn(&Theme) -> container::Appearance + Clone {
    move |theme| {
        let colors = ui_colors(theme);
        let alternate = depth % 2 == 1;
        let background = if alternate {
            colors.surface_container_low
        } else {
            colors.surface_container
        };
        container::Appearance {
            background: Some(Background::Color(background)),
            text_color: Some(colors.on_surface),
            ..container::Appearance::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_surfaces_use_borderless_distinct_theme_layers() {
        for theme in [Theme::Light, Theme::Dark] {
            let first = icon_grid_expansion_panel_style(1)(&theme);
            let second = icon_grid_expansion_panel_style(2)(&theme);

            assert_eq!(first.border.width, 0.0);
            assert_eq!(second.border.width, 0.0);
            assert_ne!(first.background, second.background);
            assert_eq!(first.text_color, Some(ui_colors(&theme).on_surface));
        }
    }
}
