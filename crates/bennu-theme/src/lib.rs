//! Bennu 共享主题库：主程序（app-ui）与 portal-backend（FileChooser）共用
//! 的唯一主题来源。持有 Material 角色模型（UiColorRoles）、主题优先级裁决
//! （ApplicationTheme::active）、内置配色、Matugen 文件解析，以及共享的
//! lucide 图标与样式词汇（styles）。消费者各自组合。

use iced::{
    theme::{
        palette::{Background, Danger, Extended, Pair, Primary, Secondary, Success, Warning},
        Palette,
    },
    Color, Theme,
};

mod catalog;
mod matugen;
mod presets;
mod scheme;

pub mod address_bar;
pub mod anchored_popup;
pub mod animation;
pub mod column_geometry;
pub mod controls;
pub mod icon_grid_geometry;
pub mod icons;
pub mod measured_text;
pub mod metrics;
pub mod preview_styles;
pub mod scrollbar;
pub mod segmented_buttons;
pub mod smooth_scroll;
pub mod styles;
pub mod translated_surface;
pub mod typography;
pub mod window_chrome_styles;
pub mod window_controls;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceMode {
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
pub enum ThemeMode {
    Light,
    Dark,
    Automatic,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::Automatic];

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }

    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Automatic => "automatic",
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Light => bennu_localization::tr!("Light"),
            Self::Dark => bennu_localization::tr!("Dark"),
            Self::Automatic => bennu_localization::tr!("Automatic"),
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

pub use catalog::ColorSchemePreset;
pub use matugen::{parse_matugen_colors, read_matugen_colors_file, MatugenColors};
pub use scheme::{contrast_ratio, SchemeColors};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiColorRoles {
    pub mode: AppearanceMode,
    pub background: Color,
    pub on_background: Color,
    pub surface: Color,
    pub surface_dim: Color,
    pub surface_bright: Color,
    pub surface_container_lowest: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub outline: Color,
    pub outline_variant: Color,
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub secondary: Color,
    pub on_secondary: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub tertiary: Color,
    pub on_tertiary: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,
    pub error: Color,
    pub on_error: Color,
    pub error_container: Color,
    pub on_error_container: Color,
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
}

#[derive(Debug, Clone)]
pub struct ApplicationTheme {
    /// 系统的深浅色（“自动”模式用）。读到系统设置之前、或桌面没有设置 portal 时是浅色。
    system_mode: AppearanceMode,
    matugen: Option<MatugenColors>,
}

impl ApplicationTheme {
    pub fn new(system_mode: AppearanceMode) -> Self {
        Self {
            system_mode,
            matugen: None,
        }
    }

    /// 所有窗口的主题都从这里来：选中的配色在当前深浅下的三色，经同一个层次计算。
    pub fn active(&self, theme_mode: ThemeMode, color_scheme: ColorSchemePreset) -> Theme {
        self.scheme_roles(theme_mode, color_scheme)
            .into_theme(color_scheme.label())
    }

    /// 设置页缩略图用：与 `active` 同源，不会和真正换上的颜色有出入。
    pub fn scheme_roles(
        &self,
        theme_mode: ThemeMode,
        color_scheme: ColorSchemePreset,
    ) -> UiColorRoles {
        let mode = self.effective_mode(theme_mode);
        let colors = match (color_scheme, &self.matugen) {
            (ColorSchemePreset::Matugen, Some(matugen)) => matugen.for_mode(mode),
            _ => presets::colors(color_scheme, mode),
        };
        UiColorRoles::derive(mode, colors)
    }

    pub fn effective_mode(&self, theme_mode: ThemeMode) -> AppearanceMode {
        theme_mode.resolve(self.system_mode)
    }

    pub fn matugen_available(&self) -> bool {
        self.matugen.is_some()
    }

    pub fn replace_system_mode(&mut self, system_mode: AppearanceMode) {
        self.system_mode = system_mode;
    }

    pub fn replace_matugen(&mut self, matugen: Option<MatugenColors>) {
        self.matugen = matugen;
    }
}

pub fn ui_colors(theme: &Theme) -> UiColorRoles {
    UiColorRoles::from_theme(theme)
}

pub fn parse_hex_color(value: &str, field: &str) -> Result<Color, String> {
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
mod tests;
