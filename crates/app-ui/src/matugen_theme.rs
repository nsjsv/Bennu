use iced::{
    theme::{
        palette::{Background, Danger, Extended, Pair, Primary, Secondary, Success, Warning},
        Palette,
    },
    Color, Theme,
};
use std::path::Path;
use toml::Table;

mod catalog;
mod custom;
mod presets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppearanceMode {
    Light,
    Dark,
}

impl AppearanceMode {
    fn theme_name(self, source: &str) -> String {
        let mode = match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        };
        format!("Bennu {source} {mode}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeMode {
    Light,
    Dark,
    Automatic,
}

impl ThemeMode {
    pub(crate) const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::Automatic];

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }

    pub(crate) const fn config_value(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Automatic => "automatic",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Automatic => "Automatic",
        }
    }

    fn resolve(self, detected: AppearanceMode) -> AppearanceMode {
        match self {
            Self::Light => AppearanceMode::Light,
            Self::Dark => AppearanceMode::Dark,
            Self::Automatic => detected,
        }
    }
}

pub(crate) use catalog::{ColorSchemeFamily, ColorSchemePreset};
pub(crate) use custom::{default_custom_color_scheme, ContrastWarnings, CustomColorScheme};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UiColorRoles {
    pub(crate) mode: AppearanceMode,
    pub(crate) background: Color,
    pub(crate) on_background: Color,
    pub(crate) surface: Color,
    pub(crate) surface_dim: Color,
    pub(crate) surface_bright: Color,
    pub(crate) surface_container_lowest: Color,
    pub(crate) surface_container_low: Color,
    pub(crate) surface_container: Color,
    pub(crate) surface_container_high: Color,
    pub(crate) surface_container_highest: Color,
    pub(crate) on_surface: Color,
    pub(crate) on_surface_variant: Color,
    pub(crate) outline: Color,
    pub(crate) outline_variant: Color,
    pub(crate) primary: Color,
    pub(crate) on_primary: Color,
    pub(crate) primary_container: Color,
    pub(crate) on_primary_container: Color,
    pub(crate) secondary: Color,
    pub(crate) on_secondary: Color,
    pub(crate) secondary_container: Color,
    pub(crate) on_secondary_container: Color,
    pub(crate) tertiary: Color,
    pub(crate) on_tertiary: Color,
    pub(crate) tertiary_container: Color,
    pub(crate) on_tertiary_container: Color,
    pub(crate) error: Color,
    pub(crate) on_error: Color,
    pub(crate) error_container: Color,
    pub(crate) on_error_container: Color,
}

impl UiColorRoles {
    fn into_theme(self, source: &str) -> Theme {
        let palette = Palette {
            background: self.background,
            text: self.on_background,
            primary: self.primary,
            success: self.secondary,
            warning: self.tertiary,
            danger: self.error,
        };
        let extended = Extended {
            background: Background {
                base: pair(self.surface, self.on_surface),
                weakest: pair(self.surface_container_lowest, self.on_surface),
                weaker: pair(self.surface_container_low, self.on_surface),
                weak: pair(self.surface_container, self.on_surface),
                neutral: pair(self.surface_container_high, self.on_surface),
                strong: pair(self.surface_container_highest, self.on_surface),
                stronger: pair(self.surface_bright, self.on_surface),
                strongest: pair(self.surface_dim, self.on_surface_variant),
            },
            primary: Primary {
                base: pair(self.primary, self.on_primary),
                weak: pair(self.primary_container, self.on_primary_container),
                strong: pair(self.primary, self.on_primary),
            },
            secondary: Secondary {
                base: pair(self.secondary, self.on_secondary),
                weak: pair(self.secondary_container, self.on_secondary_container),
                strong: pair(self.secondary, self.on_secondary),
            },
            success: Success {
                base: pair(self.outline, self.on_surface),
                weak: pair(self.outline_variant, self.on_surface),
                strong: pair(self.outline, self.on_surface),
            },
            warning: Warning {
                base: pair(self.tertiary, self.on_tertiary),
                weak: pair(self.tertiary_container, self.on_tertiary_container),
                strong: pair(self.tertiary, self.on_tertiary),
            },
            danger: Danger {
                base: pair(self.error, self.on_error),
                weak: pair(self.error_container, self.on_error_container),
                strong: pair(self.error, self.on_error),
            },
            is_dark: self.mode == AppearanceMode::Dark,
        };

        Theme::custom_with_fn(self.mode.theme_name(source), palette, move |_| extended)
    }

    fn from_theme(theme: &Theme) -> Self {
        let palette = theme.palette();
        let extended = theme.extended_palette();
        Self {
            mode: if extended.is_dark {
                AppearanceMode::Dark
            } else {
                AppearanceMode::Light
            },
            background: palette.background,
            on_background: palette.text,
            surface: extended.background.base.color,
            surface_dim: extended.background.strongest.color,
            surface_bright: extended.background.stronger.color,
            surface_container_lowest: extended.background.weakest.color,
            surface_container_low: extended.background.weaker.color,
            surface_container: extended.background.weak.color,
            surface_container_high: extended.background.neutral.color,
            surface_container_highest: extended.background.strong.color,
            on_surface: extended.background.base.text,
            on_surface_variant: extended.background.strongest.text,
            outline: extended.success.base.color,
            outline_variant: extended.success.weak.color,
            primary: extended.primary.base.color,
            on_primary: extended.primary.base.text,
            primary_container: extended.primary.weak.color,
            on_primary_container: extended.primary.weak.text,
            secondary: extended.secondary.base.color,
            on_secondary: extended.secondary.base.text,
            secondary_container: extended.secondary.weak.color,
            on_secondary_container: extended.secondary.weak.text,
            tertiary: extended.warning.base.color,
            on_tertiary: extended.warning.base.text,
            tertiary_container: extended.warning.weak.color,
            on_tertiary_container: extended.warning.weak.text,
            error: extended.danger.base.color,
            on_error: extended.danger.base.text,
            error_container: extended.danger.weak.color,
            on_error_container: extended.danger.weak.text,
        }
    }

    fn from_fallback_palette(mode: AppearanceMode, palette: Palette) -> Self {
        let extended = Extended::generate(palette);
        let (surface_dim, surface_bright) = match mode {
            AppearanceMode::Light => (
                extended.background.strongest.color,
                extended.background.base.color,
            ),
            AppearanceMode::Dark => (
                extended.background.base.color,
                extended.background.strongest.color,
            ),
        };
        let on_surface_variant = Color {
            a: 0.68,
            ..extended.background.base.text
        };

        Self {
            mode,
            background: palette.background,
            on_background: palette.text,
            surface: extended.background.base.color,
            surface_dim,
            surface_bright,
            surface_container_lowest: extended.background.base.color,
            surface_container_low: extended.background.weakest.color,
            surface_container: extended.background.weak.color,
            surface_container_high: extended.background.neutral.color,
            surface_container_highest: extended.background.strong.color,
            on_surface: extended.background.base.text,
            on_surface_variant,
            outline: extended.secondary.strong.color,
            outline_variant: extended.secondary.weak.color,
            primary: extended.primary.base.color,
            on_primary: extended.primary.base.text,
            primary_container: extended.primary.weak.color,
            on_primary_container: extended.primary.weak.text,
            secondary: extended.secondary.base.color,
            on_secondary: extended.secondary.base.text,
            secondary_container: extended.secondary.weak.color,
            on_secondary_container: extended.secondary.weak.text,
            tertiary: extended.warning.base.color,
            on_tertiary: extended.warning.base.text,
            tertiary_container: extended.warning.weak.color,
            on_tertiary_container: extended.warning.weak.text,
            error: extended.danger.base.color,
            on_error: extended.danger.base.text,
            error_container: extended.danger.weak.color,
            on_error_container: extended.danger.weak.text,
        }
    }

    fn from_preset_palette(mode: AppearanceMode, anchors: presets::PresetPalette) -> Self {
        let palette = Palette {
            background: anchors.surface,
            text: anchors.text,
            primary: anchors.primary,
            success: anchors.success,
            warning: anchors.warning,
            danger: anchors.danger,
        };
        let extended = Extended::generate(palette);
        let mut roles = Self::from_fallback_palette(mode, palette);
        roles.background = anchors.background;
        roles.on_background = anchors.text;
        roles.surface = anchors.surface;
        roles.surface_container_lowest = anchors.background;
        roles.surface_container_low = anchors.surface;
        roles.on_surface = anchors.text;
        roles.on_surface_variant = anchors.muted_text;
        roles.outline = anchors.muted_text;
        roles.primary = anchors.primary;
        roles.on_primary = extended.primary.base.text;
        roles.primary_container = extended.primary.weak.color;
        roles.on_primary_container = extended.primary.weak.text;
        roles.secondary = anchors.success;
        roles.on_secondary = extended.success.base.text;
        roles.secondary_container = extended.success.weak.color;
        roles.on_secondary_container = extended.success.weak.text;
        roles.tertiary = anchors.warning;
        roles.on_tertiary = extended.warning.base.text;
        roles.tertiary_container = extended.warning.weak.color;
        roles.on_tertiary_container = extended.warning.weak.text;
        roles.error = anchors.danger;
        roles.on_error = extended.danger.base.text;
        roles.error_container = extended.danger.weak.color;
        roles.on_error_container = extended.danger.weak.text;
        roles
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ApplicationTheme {
    system_fallback: Theme,
    matugen_override: Option<Theme>,
    custom_color_scheme: CustomColorScheme,
}

impl ApplicationTheme {
    pub(crate) fn new(system_fallback: Theme) -> Self {
        Self {
            system_fallback,
            matugen_override: None,
            custom_color_scheme: default_custom_color_scheme(),
        }
    }

    pub(crate) fn active(&self, theme_mode: ThemeMode, color_scheme: ColorSchemePreset) -> Theme {
        if color_scheme == ColorSchemePreset::Matugen {
            return self
                .matugen_override
                .clone()
                .unwrap_or_else(|| self.system_fallback.clone());
        }
        if color_scheme == ColorSchemePreset::Default && theme_mode == ThemeMode::Automatic {
            return self.system_fallback.clone();
        }

        let mode = self.effective_mode(theme_mode);
        let color_scheme = color_scheme.effective_for_mode(mode);
        match color_scheme {
            ColorSchemePreset::Default => fallback_theme(mode),
            ColorSchemePreset::Custom => {
                UiColorRoles::from_custom_anchors(mode, self.custom_color_scheme.anchors(mode))
                    .into_theme("Custom")
            }
            ColorSchemePreset::Matugen => unreachable!("Matugen is handled above"),
            _ => preset_theme(color_scheme, mode),
        }
    }

    pub(crate) fn effective_mode(&self, theme_mode: ThemeMode) -> AppearanceMode {
        theme_mode.resolve(ui_colors(&self.system_fallback).mode)
    }

    pub(crate) fn preview_colors(
        &self,
        theme_mode: ThemeMode,
        color_scheme: ColorSchemePreset,
    ) -> [Color; 3] {
        let theme = self.active(theme_mode, color_scheme);
        let colors = ui_colors(&theme);
        [colors.background, colors.primary, colors.on_background]
    }

    pub(crate) fn replace_system_fallback(&mut self, theme: Theme) {
        self.system_fallback = theme;
    }

    pub(crate) fn replace_matugen_override(&mut self, theme: Option<Theme>) {
        self.matugen_override = theme;
    }

    pub(crate) fn replace_custom_color_scheme(&mut self, custom_color_scheme: CustomColorScheme) {
        self.custom_color_scheme = custom_color_scheme;
    }
}

pub(crate) fn fallback_theme(mode: AppearanceMode) -> Theme {
    let palette = match mode {
        AppearanceMode::Light => Palette {
            background: Color::from_rgb8(250, 252, 255),
            text: Color::from_rgb8(36, 43, 54),
            primary: Color::from_rgb8(74, 137, 220),
            success: Color::from_rgb8(69, 139, 101),
            warning: Color::from_rgb8(183, 126, 51),
            danger: Color::from_rgb8(195, 66, 63),
        },
        AppearanceMode::Dark => Palette {
            background: Color::from_rgb8(18, 24, 34),
            text: Color::from_rgb8(226, 233, 242),
            primary: Color::from_rgb8(82, 126, 190),
            success: Color::from_rgb8(95, 158, 120),
            warning: Color::from_rgb8(255, 193, 78),
            danger: Color::from_rgb8(211, 90, 90),
        },
    };

    UiColorRoles::from_fallback_palette(mode, palette).into_theme("System")
}

fn preset_theme(preset: ColorSchemePreset, mode: AppearanceMode) -> Theme {
    UiColorRoles::from_preset_palette(mode, presets::palette(preset, mode))
        .into_theme(preset.label())
}

pub(crate) fn ui_colors(theme: &Theme) -> UiColorRoles {
    UiColorRoles::from_theme(theme)
}

pub(crate) fn parse_matugen_theme(document: &str) -> Result<Theme, String> {
    let document_table = document.parse::<Table>().map_err(|error| {
        error
            .span()
            .map(|span| format!("TOML syntax error at bytes {}..{}", span.start, span.end))
            .unwrap_or_else(|| "TOML syntax error".to_owned())
    })?;
    let version = required_integer(&document_table, "version")?;
    if version != 1 {
        return Err("version must be 1".to_owned());
    }
    let mode = match required_string(&document_table, "mode")? {
        "light" => AppearanceMode::Light,
        "dark" => AppearanceMode::Dark,
        _ => return Err("mode must be light or dark".to_owned()),
    };
    let colors_table = required_table(&document_table, "colors")?;
    let color = |key| required_color(colors_table, key);
    let roles = UiColorRoles {
        mode,
        background: color("background")?,
        on_background: color("on_background")?,
        surface: color("surface")?,
        surface_dim: color("surface_dim")?,
        surface_bright: color("surface_bright")?,
        surface_container_lowest: color("surface_container_lowest")?,
        surface_container_low: color("surface_container_low")?,
        surface_container: color("surface_container")?,
        surface_container_high: color("surface_container_high")?,
        surface_container_highest: color("surface_container_highest")?,
        on_surface: color("on_surface")?,
        on_surface_variant: color("on_surface_variant")?,
        outline: color("outline")?,
        outline_variant: color("outline_variant")?,
        primary: color("primary")?,
        on_primary: color("on_primary")?,
        primary_container: color("primary_container")?,
        on_primary_container: color("on_primary_container")?,
        secondary: color("secondary")?,
        on_secondary: color("on_secondary")?,
        secondary_container: color("secondary_container")?,
        on_secondary_container: color("on_secondary_container")?,
        tertiary: color("tertiary")?,
        on_tertiary: color("on_tertiary")?,
        tertiary_container: color("tertiary_container")?,
        on_tertiary_container: color("on_tertiary_container")?,
        error: color("error")?,
        on_error: color("on_error")?,
        error_container: color("error_container")?,
        on_error_container: color("on_error_container")?,
    };

    Ok(roles.into_theme("Matugen"))
}

pub(crate) async fn read_matugen_theme_file(path: &Path) -> Result<Option<Theme>, String> {
    let document = match tokio::fs::read_to_string(path).await {
        Ok(document) => document,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not read matugen theme {}: {error}",
                path.display()
            ));
        }
    };

    parse_matugen_theme(&document)
        .map(Some)
        .map_err(|error| format!("invalid matugen theme {}: {error}", path.display()))
}

fn required_table<'a>(table: &'a Table, key: &str) -> Result<&'a Table, String> {
    table
        .get(key)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{key} must be a table"))
}

fn required_integer(table: &Table, key: &str) -> Result<i64, String> {
    table
        .get(key)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| format!("{key} must be an integer"))
}

fn required_string<'a>(table: &'a Table, key: &str) -> Result<&'a str, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn required_color(table: &Table, key: &str) -> Result<Color, String> {
    let hex = required_string(table, key)?;
    parse_hex_color(hex, &format!("colors.{key}"))
}

pub(crate) fn parse_hex_color(value: &str, field: &str) -> Result<Color, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return Err(format!("{field} must use #RRGGBB"));
    }

    let mut channels = [0_u8; 3];
    for (channel, pair) in channels.iter_mut().zip(bytes[1..].chunks_exact(2)) {
        *channel = (hex_nibble(pair[0]).ok_or_else(|| format!("{field} must use #RRGGBB"))? << 4)
            | hex_nibble(pair[1]).ok_or_else(|| format!("{field} must use #RRGGBB"))?;
    }

    Ok(Color::from_rgb8(channels[0], channels[1], channels[2]))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn pair(color: Color, text: Color) -> Pair {
    Pair { color, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK_THEME: &str = include_str!("../test-data/matugen-dark.toml");
    const LIGHT_THEME: &str = include_str!("../test-data/matugen-light.toml");

    #[test]
    fn generated_dark_and_light_themes_preserve_matugen_roles() {
        let dark = parse_matugen_theme(DARK_THEME).expect("dark matugen theme");
        let light = parse_matugen_theme(LIGHT_THEME).expect("light matugen theme");

        assert!(dark.extended_palette().is_dark);
        assert!(!light.extended_palette().is_dark);
        assert_eq!(
            dark.palette().background,
            Color::from_rgb8(0x14, 0x12, 0x18)
        );
        assert_eq!(
            light.palette().background,
            Color::from_rgb8(0xfd, 0xf7, 0xff)
        );
        assert_eq!(
            dark.extended_palette().primary.weak.color,
            Color::from_rgb8(0x4d, 0x3d, 0x75)
        );
        assert_eq!(
            light.extended_palette().danger.weak.text,
            Color::from_rgb8(0x41, 0x00, 0x02)
        );
        assert_eq!(
            ui_colors(&dark).outline_variant,
            Color::from_rgb8(0x49, 0x45, 0x4e)
        );
        assert_eq!(
            ui_colors(&light).on_surface_variant,
            Color::from_rgb8(0x49, 0x45, 0x4e)
        );
    }

    #[test]
    fn malformed_theme_documents_are_rejected_at_the_boundary() {
        for malformed in [
            LIGHT_THEME.replacen("version = 1", "version = 2", 1),
            LIGHT_THEME.replacen("mode = \"light\"", "mode = \"sepia\"", 1),
            LIGHT_THEME.replacen("background = \"#fdf7ff\"", "background = \"#fff\"", 1),
            LIGHT_THEME.replacen("primary = ", "missing_primary = ", 1),
        ] {
            assert!(parse_matugen_theme(&malformed).is_err());
        }
    }

    #[test]
    fn matugen_override_wins_when_selected_even_if_system_theme_arrives_late() {
        let light = fallback_theme(AppearanceMode::Light);
        let dark = fallback_theme(AppearanceMode::Dark);
        let generated = parse_matugen_theme(LIGHT_THEME).expect("light matugen theme");
        let generated_background = generated.palette().background;
        let mut application_theme = ApplicationTheme::new(light);

        application_theme.replace_matugen_override(Some(generated));
        application_theme.replace_system_fallback(dark.clone());
        assert_eq!(
            application_theme
                .active(ThemeMode::Dark, ColorSchemePreset::Matugen)
                .palette()
                .background,
            generated_background
        );

        application_theme.replace_matugen_override(None);
        assert_eq!(
            application_theme
                .active(ThemeMode::Dark, ColorSchemePreset::Matugen)
                .palette(),
            dark.palette()
        );
    }

    #[test]
    fn settings_options_keep_their_stable_order_and_values() {
        assert_eq!(
            ThemeMode::ALL.map(ThemeMode::config_value),
            ["light", "dark", "automatic"]
        );
        assert_eq!(
            ColorSchemeFamily::ALL.map(ColorSchemeFamily::label),
            [
                "Default",
                "Claude",
                "Catppuccin",
                "Dracula",
                "Everforest",
                "GitHub",
                "Gruvbox",
                "Kanagawa",
                "Nord",
                "One",
                "Rosé Pine",
                "Solarized",
                "Tokyo Night",
                "Matugen",
                "Custom",
            ]
        );
        assert_eq!(
            ColorSchemePreset::ALL.map(ColorSchemePreset::config_value),
            [
                "default",
                "claude",
                "catppuccin",
                "catppuccin-frappe",
                "catppuccin-macchiato",
                "dracula",
                "everforest-hard",
                "everforest",
                "everforest-soft",
                "github",
                "github-dimmed",
                "github-high-contrast",
                "github-colorblind",
                "github-tritanopia",
                "gruvbox-hard",
                "gruvbox",
                "gruvbox-soft",
                "kanagawa",
                "kanagawa-dragon",
                "nord",
                "one",
                "rose-pine",
                "rose-pine-moon",
                "solarized",
                "tokyo-night",
                "tokyo-night-storm",
                "tokyo-night-moon",
                "matugen",
                "custom",
            ]
        );
        for preset in ColorSchemePreset::ALL {
            assert_eq!(
                ColorSchemePreset::from_config_value(preset.config_value()),
                Some(preset)
            );
        }
    }

    #[test]
    fn built_in_presets_provide_complete_roles_and_ignore_matugen_updates() {
        let light = fallback_theme(AppearanceMode::Light);
        let generated = parse_matugen_theme(DARK_THEME).expect("dark matugen theme");
        let generated_background = generated.palette().background;
        let mut application_theme = ApplicationTheme::new(light);
        application_theme.replace_matugen_override(Some(generated));

        for (theme_mode, expected_mode) in [
            (ThemeMode::Light, AppearanceMode::Light),
            (ThemeMode::Dark, AppearanceMode::Dark),
        ] {
            for preset in ColorSchemePreset::ALL {
                if preset == ColorSchemePreset::Matugen {
                    continue;
                }
                let theme = application_theme.active(theme_mode, preset);
                let roles = ui_colors(&theme);
                assert_eq!(roles.mode, expected_mode);
                assert_ne!(roles.background, generated_background);
                for color in [
                    roles.background,
                    roles.on_background,
                    roles.primary,
                    roles.outline,
                    roles.tertiary,
                    roles.error,
                ] {
                    assert!(color.a > 0.0);
                }
            }
        }
    }
    #[test]
    fn revised_presets_use_reference_palette_anchors() {
        let rgb =
            |value: u32| Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8);
        for (preset, mode, expected) in [
            (
                ColorSchemePreset::Everforest,
                AppearanceMode::Light,
                [0xfdf6e3, 0x5c6a72, 0x3a94c5, 0xdfa000, 0xf85552],
            ),
            (
                ColorSchemePreset::Everforest,
                AppearanceMode::Dark,
                [0x2d353b, 0xd3c6aa, 0x7fbbb3, 0xdbbc7f, 0xe67e80],
            ),
            (
                ColorSchemePreset::Catppuccin,
                AppearanceMode::Light,
                [0xeff1f5, 0x4c4f69, 0x8839ef, 0xdf8e1d, 0xd20f39],
            ),
            (
                ColorSchemePreset::Catppuccin,
                AppearanceMode::Dark,
                [0x1e1e2e, 0xcdd6f4, 0xcba6f7, 0xf9e2af, 0xf38ba8],
            ),
            (
                ColorSchemePreset::Claude,
                AppearanceMode::Light,
                [0xfaf9f5, 0x3d3929, 0xc96442, 0xd4a27f, 0x141413],
            ),
            (
                ColorSchemePreset::Claude,
                AppearanceMode::Dark,
                [0x262624, 0xc3c0b6, 0xd97757, 0xd4a27f, 0xef4444],
            ),
        ] {
            let roles = ui_colors(&preset_theme(preset, mode));
            assert_eq!(
                [
                    roles.background,
                    roles.on_background,
                    roles.primary,
                    roles.tertiary,
                    roles.error
                ],
                expected.map(rgb)
            );
        }
    }

    #[test]
    fn automatic_mode_uses_the_detected_system_mode() {
        let mut application_theme = ApplicationTheme::new(fallback_theme(AppearanceMode::Light));
        for (system_theme, expected_dark) in [
            (fallback_theme(AppearanceMode::Light), false),
            (fallback_theme(AppearanceMode::Dark), true),
        ] {
            application_theme.replace_system_fallback(system_theme.clone());
            assert_eq!(
                ui_colors(
                    &application_theme.active(ThemeMode::Automatic, ColorSchemePreset::Default)
                ),
                ui_colors(&system_theme)
            );
            assert_eq!(
                application_theme
                    .active(ThemeMode::Automatic, ColorSchemePreset::Nord)
                    .extended_palette()
                    .is_dark,
                expected_dark
            );
        }
    }

    #[tokio::test]
    async fn missing_file_is_distinct_from_an_invalid_document() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("matugen.toml");

        assert!(read_matugen_theme_file(&path)
            .await
            .expect("missing theme is optional")
            .is_none());

        tokio::fs::write(&path, "version = 1\nmode = ???")
            .await
            .expect("write invalid theme");
        assert!(read_matugen_theme_file(&path).await.is_err());
    }
}
