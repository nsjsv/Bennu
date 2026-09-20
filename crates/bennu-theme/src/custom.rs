use serde::Deserialize;

use super::{AppearanceMode, Color, UiColorRoles};
use file_operation_store::{StoredCustomColorScheme, StoredCustomColorSet};

const MINIMUM_CONTRAST_WARNING_RATIO: f32 = 2.4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CustomColorAnchors {
    pub background: Color,
    pub surface: Color,
    pub text: Color,
    pub muted_text: Color,
    pub primary: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomColorScheme {
    pub light: CustomColorAnchors,
    pub dark: CustomColorAnchors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContrastWarnings {
    pub background_text: bool,
    pub surface_muted_text: bool,
}

impl ContrastWarnings {
    pub fn is_empty(self) -> bool {
        !self.background_text && !self.surface_muted_text
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomColorDocument {
    version: u8,
    light: CustomColorSetDocument,
    dark: CustomColorSetDocument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomColorSetDocument {
    background: String,
    surface: String,
    text: String,
    muted_text: String,
    primary: String,
    success: String,
    warning: String,
    danger: String,
}

impl UiColorRoles {
    pub(super) fn from_custom_anchors(mode: AppearanceMode, anchors: CustomColorAnchors) -> Self {
        Self::from_preset_palette(
            mode,
            super::presets::PresetPalette {
                background: anchors.background,
                surface: anchors.surface,
                text: anchors.text,
                muted_text: anchors.muted_text,
                primary: anchors.primary,
                success: anchors.success,
                warning: anchors.warning,
                danger: anchors.danger,
            },
        )
    }
}

pub fn default_custom_color_scheme() -> CustomColorScheme {
    let light = super::ui_colors(&super::fallback_theme(AppearanceMode::Light));
    let dark = super::ui_colors(&super::fallback_theme(AppearanceMode::Dark));
    CustomColorScheme {
        light: CustomColorAnchors {
            background: light.background,
            surface: light.surface,
            text: light.on_background,
            muted_text: light.on_surface_variant,
            primary: light.primary,
            success: light.secondary,
            warning: light.tertiary,
            danger: light.error,
        },
        dark: CustomColorAnchors {
            background: dark.background,
            surface: dark.surface,
            text: dark.on_background,
            muted_text: dark.on_surface_variant,
            primary: dark.primary,
            success: dark.secondary,
            warning: dark.tertiary,
            danger: dark.error,
        },
    }
}

impl CustomColorScheme {
    pub fn from_json(document: &str) -> Result<Self, String> {
        let document = serde_json::from_str::<CustomColorDocument>(document)
            .map_err(|error| format!("invalid custom color scheme JSON: {error}"))?;
        if document.version != 1 {
            return Err("custom color scheme version must be 1".to_owned());
        }

        Ok(Self {
            light: document.light.into_anchors("light")?,
            dark: document.dark.into_anchors("dark")?,
        })
    }

    pub fn from_stored(stored: Option<&StoredCustomColorScheme>, fallback: &Self) -> Self {
        let Some(stored) = stored else {
            return fallback.clone();
        };
        Self {
            light: stored
                .light
                .as_ref()
                .and_then(|set| CustomColorAnchors::from_stored(set).ok())
                .unwrap_or(fallback.light),
            dark: stored
                .dark
                .as_ref()
                .and_then(|set| CustomColorAnchors::from_stored(set).ok())
                .unwrap_or(fallback.dark),
        }
    }

    pub fn to_stored(&self) -> StoredCustomColorScheme {
        StoredCustomColorScheme {
            light: Some(self.light.to_stored()),
            dark: Some(self.dark.to_stored()),
        }
    }

    pub fn anchors(&self, mode: AppearanceMode) -> CustomColorAnchors {
        match mode {
            AppearanceMode::Light => self.light,
            AppearanceMode::Dark => self.dark,
        }
    }

    pub fn contrast_warnings(&self, mode: AppearanceMode) -> ContrastWarnings {
        let anchors = self.anchors(mode);
        ContrastWarnings {
            background_text: contrast_ratio(anchors.background, anchors.text)
                < MINIMUM_CONTRAST_WARNING_RATIO,
            surface_muted_text: contrast_ratio(anchors.surface, anchors.muted_text)
                < MINIMUM_CONTRAST_WARNING_RATIO,
        }
    }
}

impl CustomColorSetDocument {
    fn into_anchors(self, side: &str) -> Result<CustomColorAnchors, String> {
        let color = |name: &'static str, value: String| {
            super::parse_hex_color(&value, &format!("{side}.{name}"))
        };
        Ok(CustomColorAnchors {
            background: color("background", self.background)?,
            surface: color("surface", self.surface)?,
            text: color("text", self.text)?,
            muted_text: color("muted_text", self.muted_text)?,
            primary: color("primary", self.primary)?,
            success: color("success", self.success)?,
            warning: color("warning", self.warning)?,
            danger: color("danger", self.danger)?,
        })
    }
}

impl CustomColorAnchors {
    fn from_stored(stored: &StoredCustomColorSet) -> Result<Self, String> {
        let color = |name: &'static str, value: &str| {
            super::parse_hex_color(value, &format!("custom.{name}"))
        };
        Ok(Self {
            background: color("background", &stored.background)?,
            surface: color("surface", &stored.surface)?,
            text: color("text", &stored.text)?,
            muted_text: color("muted_text", &stored.muted_text)?,
            primary: color("primary", &stored.primary)?,
            success: color("success", &stored.success)?,
            warning: color("warning", &stored.warning)?,
            danger: color("danger", &stored.danger)?,
        })
    }

    fn to_stored(self) -> StoredCustomColorSet {
        StoredCustomColorSet {
            background: color_hex(self.background),
            surface: color_hex(self.surface),
            text: color_hex(self.text),
            muted_text: color_hex(self.muted_text),
            primary: color_hex(self.primary),
            success: color_hex(self.success),
            warning: color_hex(self.warning),
            danger: color_hex(self.danger),
        }
    }
}

fn color_hex(color: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
    )
}

fn contrast_ratio(first: Color, second: Color) -> f32 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn relative_luminance(color: Color) -> f32 {
    let channel = |value: f32| {
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r##"{
        "version": 1,
        "light": {
            "background": "#ffffff",
            "surface": "#f6f8fa",
            "text": "#1f2328",
            "muted_text": "#59636e",
            "primary": "#0969da",
            "success": "#1a7f37",
            "warning": "#9a6700",
            "danger": "#d1242f"
        },
        "dark": {
            "background": "#0d1117",
            "surface": "#151b23",
            "text": "#f0f6fc",
            "muted_text": "#9198a1",
            "primary": "#4493f8",
            "success": "#3fb950",
            "warning": "#d29922",
            "danger": "#f85149"
        }
    }"##;

    #[test]
    fn valid_document_replaces_both_mode_sets_atomically() {
        let scheme = CustomColorScheme::from_json(VALID).expect("valid custom scheme");
        assert_eq!(scheme.light.background, Color::from_rgb8(255, 255, 255));
        assert_eq!(scheme.dark.primary, Color::from_rgb8(68, 147, 248));
    }

    #[test]
    fn malformed_document_is_rejected_without_partial_state() {
        for malformed in [
            VALID.replace("\"version\": 1", "\"version\": 2"),
            VALID.replace("\"danger\": \"#d1242f\"", "\"extra\": \"#d1242f\""),
            VALID.replace("\"#ffffff\"", "\"#fff\""),
            VALID.replace("\"dark\": {", "\"dark\": {\n            \"background\": \"#000000\",\n            \"background\": "),
        ] {
            assert!(CustomColorScheme::from_json(&malformed).is_err());
        }
    }

    #[test]
    fn duplicate_fields_are_rejected_at_the_json_boundary() {
        let duplicate = VALID.replace(
            "\"surface\": \"#f6f8fa\",",
            "\"surface\": \"#f6f8fa\",\n            \"surface\": \"#f6f8fa\",",
        );
        assert!(CustomColorScheme::from_json(&duplicate).is_err());
    }

    #[test]
    fn custom_theme_uses_mode_specific_anchors_and_keeps_matugen_priority() {
        use super::super::{
            fallback_theme, parse_matugen_theme, ApplicationTheme, ColorSchemePreset, ThemeMode,
        };

        let mut custom = default_custom_color_scheme();
        custom.light.background = Color::from_rgb8(1, 2, 3);
        custom.dark.background = Color::from_rgb8(4, 5, 6);
        let mut application_theme = ApplicationTheme::new(fallback_theme(AppearanceMode::Light));
        application_theme.replace_custom_color_scheme(custom);

        assert_eq!(
            application_theme
                .active(ThemeMode::Light, ColorSchemePreset::Custom)
                .palette()
                .background,
            Color::from_rgb8(1, 2, 3)
        );
        application_theme.replace_system_fallback(fallback_theme(AppearanceMode::Dark));
        assert_eq!(
            application_theme
                .active(ThemeMode::Automatic, ColorSchemePreset::Custom)
                .palette()
                .background,
            Color::from_rgb8(4, 5, 6)
        );

        let matugen = parse_matugen_theme(include_str!("../test-data/matugen-light.toml"))
            .expect("light matugen theme");
        let matugen_background = matugen.palette().background;
        application_theme.replace_matugen_override(Some(matugen));
        assert_eq!(
            application_theme
                .active(ThemeMode::Dark, ColorSchemePreset::Matugen)
                .palette()
                .background,
            matugen_background
        );
    }

    #[test]
    fn low_contrast_is_warning_only_signal() {
        let scheme = CustomColorScheme::from_json(
            &VALID
                .replace("#ffffff", "#111111")
                .replace("#1f2328", "#222222"),
        )
        .expect("valid low contrast scheme");
        let warnings = scheme.contrast_warnings(AppearanceMode::Light);
        assert!(warnings.background_text);
        assert!(!warnings.is_empty());
    }
}
