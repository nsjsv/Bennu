use iced::Color;

use super::{AppearanceMode, ColorSchemePreset};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PresetPalette {
    pub(super) background: Color,
    pub(super) surface: Color,
    pub(super) text: Color,
    pub(super) muted_text: Color,
    pub(super) primary: Color,
    pub(super) success: Color,
    pub(super) warning: Color,
    pub(super) danger: Color,
}

pub(super) fn palette(preset: ColorSchemePreset, mode: AppearanceMode) -> PresetPalette {
    let preset = preset.effective_for_mode(mode);
    match (preset, mode) {
        (ColorSchemePreset::Claude, AppearanceMode::Light) => colors(
            0xfaf9f5, 0xe9e6dc, 0x3d3929, 0x83827d, 0xc96442, 0x16a34a, 0xd4a27f, 0x141413,
        ),
        (ColorSchemePreset::Claude, AppearanceMode::Dark) => colors(
            0x262624, 0x30302e, 0xc3c0b6, 0xb7b5a9, 0xd97757, 0x22c55e, 0xd4a27f, 0xef4444,
        ),
        (ColorSchemePreset::Catppuccin, AppearanceMode::Light) => colors(
            0xeff1f5, 0xe6e9ef, 0x4c4f69, 0x6c6f85, 0x8839ef, 0x40a02b, 0xdf8e1d, 0xd20f39,
        ),
        (ColorSchemePreset::Catppuccin, AppearanceMode::Dark) => colors(
            0x1e1e2e, 0x181825, 0xcdd6f4, 0xa6adc8, 0xcba6f7, 0xa6e3a1, 0xf9e2af, 0xf38ba8,
        ),
        (ColorSchemePreset::CatppuccinFrappe, AppearanceMode::Dark) => colors(
            0x303446, 0x292c3c, 0xc6d0f5, 0xa5adce, 0xca9ee6, 0xa6d189, 0xe5c890, 0xe78284,
        ),
        (ColorSchemePreset::CatppuccinMacchiato, AppearanceMode::Dark) => colors(
            0x24273a, 0x1e2030, 0xcad3f5, 0xa5adcb, 0xc6a0f6, 0xa6da95, 0xeed49f, 0xed8796,
        ),
        (ColorSchemePreset::Dracula, AppearanceMode::Light) => colors(
            0xfffbeb, 0xece9df, 0x1f1f1f, 0x6c664b, 0x644ac9, 0x14710a, 0x846e15, 0xcb3a2a,
        ),
        (ColorSchemePreset::Dracula, AppearanceMode::Dark) => colors(
            0x282a36, 0x44475a, 0xf8f8f2, 0x6272a4, 0xbd93f9, 0x50fa7b, 0xf1fa8c, 0xff5555,
        ),
        (ColorSchemePreset::EverforestHard, AppearanceMode::Light) => colors(
            0xfffbef, 0xf2efdf, 0x5c6a72, 0x939f91, 0x3a94c5, 0x8da101, 0xdfa000, 0xf85552,
        ),
        (ColorSchemePreset::Everforest, AppearanceMode::Light) => colors(
            0xfdf6e3, 0xf4f0d9, 0x5c6a72, 0x939f91, 0x3a94c5, 0x8da101, 0xdfa000, 0xf85552,
        ),
        (ColorSchemePreset::EverforestSoft, AppearanceMode::Light) => colors(
            0xf8f0dc, 0xeae4ca, 0x5c6a72, 0x939f91, 0x3a94c5, 0x8da101, 0xdfa000, 0xf85552,
        ),
        (ColorSchemePreset::EverforestHard, AppearanceMode::Dark) => colors(
            0x2b3339, 0x323c41, 0xd3c6aa, 0x859289, 0x7fbbb3, 0xa7c080, 0xdbbc7f, 0xe67e80,
        ),
        (ColorSchemePreset::Everforest, AppearanceMode::Dark) => colors(
            0x2d353b, 0x343f44, 0xd3c6aa, 0x859289, 0x7fbbb3, 0xa7c080, 0xdbbc7f, 0xe67e80,
        ),
        (ColorSchemePreset::EverforestSoft, AppearanceMode::Dark) => colors(
            0x323d43, 0x3a464c, 0xd3c6aa, 0x859289, 0x7fbbb3, 0xa7c080, 0xdbbc7f, 0xe67e80,
        ),
        (ColorSchemePreset::GitHub, AppearanceMode::Light) => colors(
            0xffffff, 0xf6f8fa, 0x1f2328, 0x59636e, 0x0969da, 0x1a7f37, 0x9a6700, 0xd1242f,
        ),
        (ColorSchemePreset::GitHub, AppearanceMode::Dark) => colors(
            0x0d1117, 0x151b23, 0xf0f6fc, 0x9198a1, 0x4493f8, 0x3fb950, 0xd29922, 0xf85149,
        ),
        (ColorSchemePreset::GitHubDimmed, AppearanceMode::Dark) => colors(
            0x212830, 0x262c36, 0xd1d7e0, 0x9198a1, 0x478be6, 0x57ab5a, 0xc69026, 0xe5534b,
        ),
        (ColorSchemePreset::GitHubHighContrast, AppearanceMode::Light) => colors(
            0xffffff, 0xe6eaef, 0x010409, 0x454c54, 0x023b95, 0x04591f, 0x603700, 0x960d1e,
        ),
        (ColorSchemePreset::GitHubHighContrast, AppearanceMode::Dark) => colors(
            0x010409, 0x151b23, 0xffffff, 0xb7bdc8, 0x74b9ff, 0x2bd853, 0xf0b72f, 0xff9492,
        ),
        (ColorSchemePreset::GitHubColorblind, AppearanceMode::Light) => colors(
            0xffffff, 0xf6f8fa, 0x1f2328, 0x59636e, 0x0969da, 0x0969da, 0x9a6700, 0xbc4c00,
        ),
        (ColorSchemePreset::GitHubColorblind, AppearanceMode::Dark) => colors(
            0x0d1117, 0x151b23, 0xf0f6fc, 0x9198a1, 0x4493f8, 0x58a6ff, 0xd29922, 0xf0883e,
        ),
        (ColorSchemePreset::GitHubTritanopia, AppearanceMode::Light) => colors(
            0xffffff, 0xf6f8fa, 0x1f2328, 0x59636e, 0x0969da, 0x0969da, 0x9a6700, 0xd1242f,
        ),
        (ColorSchemePreset::GitHubTritanopia, AppearanceMode::Dark) => colors(
            0x0d1117, 0x151b23, 0xf0f6fc, 0x9198a1, 0x4493f8, 0x58a6ff, 0xd29922, 0xf85149,
        ),
        (ColorSchemePreset::GruvboxHard, AppearanceMode::Light) => colors(
            0xf9f5d7, 0xebdbb2, 0x3c3836, 0x928374, 0x458588, 0x98971a, 0xd79921, 0xcc241d,
        ),
        (ColorSchemePreset::Gruvbox, AppearanceMode::Light) => colors(
            0xfbf1c7, 0xebdbb2, 0x3c3836, 0x928374, 0x458588, 0x98971a, 0xd79921, 0xcc241d,
        ),
        (ColorSchemePreset::GruvboxSoft, AppearanceMode::Light) => colors(
            0xf2e5bc, 0xebdbb2, 0x3c3836, 0x928374, 0x458588, 0x98971a, 0xd79921, 0xcc241d,
        ),
        (ColorSchemePreset::GruvboxHard, AppearanceMode::Dark) => colors(
            0x1d2021, 0x3c3836, 0xebdbb2, 0x928374, 0x83a598, 0xb8bb26, 0xfabd2f, 0xfb4934,
        ),
        (ColorSchemePreset::Gruvbox, AppearanceMode::Dark) => colors(
            0x282828, 0x3c3836, 0xebdbb2, 0x928374, 0x83a598, 0xb8bb26, 0xfabd2f, 0xfb4934,
        ),
        (ColorSchemePreset::GruvboxSoft, AppearanceMode::Dark) => colors(
            0x32302f, 0x3c3836, 0xebdbb2, 0x928374, 0x83a598, 0xb8bb26, 0xfabd2f, 0xfb4934,
        ),
        (ColorSchemePreset::Kanagawa, AppearanceMode::Light) => colors(
            0xf2ecbc, 0xe5ddb0, 0x545464, 0x8a8980, 0x4d699b, 0x6f894e, 0xe98a00, 0xc84053,
        ),
        (ColorSchemePreset::Kanagawa, AppearanceMode::Dark) => colors(
            0x1f1f28, 0x2a2a37, 0xdcd7ba, 0x727169, 0x7fb4ca, 0x98bb6c, 0xe6c384, 0xc34043,
        ),
        (ColorSchemePreset::KanagawaDragon, AppearanceMode::Dark) => colors(
            0x181616, 0x282727, 0xc5c9c5, 0x737c73, 0x8ba4b0, 0x8a9a7b, 0xc4b28a, 0xc4746e,
        ),
        (ColorSchemePreset::Nord, AppearanceMode::Light) => colors(
            0xeceff4, 0xe5e9f0, 0x2e3440, 0x4c566a, 0x5e81ac, 0xa3be8c, 0xebcb8b, 0xbf616a,
        ),
        (ColorSchemePreset::Nord, AppearanceMode::Dark) => colors(
            0x2e3440, 0x3b4252, 0xeceff4, 0xd8dee9, 0x88c0d0, 0xa3be8c, 0xebcb8b, 0xbf616a,
        ),
        (ColorSchemePreset::One, AppearanceMode::Light) => colors(
            0xfafafa, 0xf0f0f0, 0x383a42, 0xa0a1a7, 0x4078f2, 0x50a14f, 0xc18401, 0xe45649,
        ),
        (ColorSchemePreset::One, AppearanceMode::Dark) => colors(
            0x282c34, 0x21252b, 0xabb2bf, 0x5c6370, 0x61afef, 0x98c379, 0xe5c07b, 0xe06c75,
        ),
        (ColorSchemePreset::RosePine, AppearanceMode::Light) => colors(
            0xfaf4ed, 0xfffaf3, 0x575279, 0x9893a5, 0x907aa9, 0x286983, 0xea9d34, 0xb4637a,
        ),
        (ColorSchemePreset::RosePine, AppearanceMode::Dark) => colors(
            0x191724, 0x1f1d2e, 0xe0def4, 0x908caa, 0xc4a7e7, 0x31748f, 0xf6c177, 0xeb6f92,
        ),
        (ColorSchemePreset::RosePineMoon, AppearanceMode::Dark) => colors(
            0x232136, 0x2a273f, 0xe0def4, 0x908caa, 0xc4a7e7, 0x3e8fb0, 0xf6c177, 0xeb6f92,
        ),
        (ColorSchemePreset::Solarized, AppearanceMode::Light) => colors(
            0xfdf6e3, 0xeee8d5, 0x657b83, 0x93a1a1, 0x2aa198, 0x859900, 0xb58900, 0xdc322f,
        ),
        (ColorSchemePreset::Solarized, AppearanceMode::Dark) => colors(
            0x002b36, 0x073642, 0x839496, 0x586e75, 0x2aa198, 0x859900, 0xb58900, 0xdc322f,
        ),
        (ColorSchemePreset::TokyoNight, AppearanceMode::Light) => colors(
            0xd5d6db, 0xcbccd1, 0x565a6e, 0x8990b3, 0x2e7de9, 0x587539, 0x8c6c3e, 0x8c4351,
        ),
        (ColorSchemePreset::TokyoNight, AppearanceMode::Dark) => colors(
            0x1a1b26, 0x24283b, 0xc0caf5, 0x565f89, 0x7aa2f7, 0x9ece6a, 0xe0af68, 0xf7768e,
        ),
        (ColorSchemePreset::TokyoNightStorm, AppearanceMode::Dark) => colors(
            0x24283b, 0x292e42, 0xc0caf5, 0x565f89, 0x7aa2f7, 0x9ece6a, 0xe0af68, 0xf7768e,
        ),
        (ColorSchemePreset::TokyoNightMoon, AppearanceMode::Dark) => colors(
            0x222436, 0x2f334d, 0xc8d3f5, 0x636da6, 0x82aaff, 0xc3e88d, 0xffc777, 0xff757f,
        ),
        (
            ColorSchemePreset::Default | ColorSchemePreset::Matugen | ColorSchemePreset::Custom,
            _,
        ) => {
            unreachable!("dynamic themes do not use static preset palettes")
        }
        _ => unreachable!("the preset was resolved to a mode-compatible style"),
    }
}

const fn colors(
    background: u32,
    surface: u32,
    text: u32,
    muted_text: u32,
    primary: u32,
    success: u32,
    warning: u32,
    danger: u32,
) -> PresetPalette {
    PresetPalette {
        background: rgb(background),
        surface: rgb(surface),
        text: rgb(text),
        muted_text: rgb(muted_text),
        primary: rgb(primary),
        success: rgb(success),
        warning: rgb(warning),
        danger: rgb(danger),
    }
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_palette_matches_credit_css_anchors() {
        let light = palette(ColorSchemePreset::Claude, AppearanceMode::Light);
        assert_eq!(light.background, rgb(0xfaf9f5));
        assert_eq!(light.surface, rgb(0xe9e6dc));
        assert_eq!(light.text, rgb(0x3d3929));
        assert_eq!(light.muted_text, rgb(0x83827d));
        assert_eq!(light.primary, rgb(0xc96442));
        assert_eq!(light.success, rgb(0x16a34a));
        assert_eq!(light.danger, rgb(0x141413));

        let dark = palette(ColorSchemePreset::Claude, AppearanceMode::Dark);
        assert_eq!(dark.background, rgb(0x262624));
        assert_eq!(dark.surface, rgb(0x30302e));
        assert_eq!(dark.text, rgb(0xc3c0b6));
        assert_eq!(dark.muted_text, rgb(0xb7b5a9));
        assert_eq!(dark.primary, rgb(0xd97757));
        assert_eq!(dark.success, rgb(0x22c55e));
        assert_eq!(dark.danger, rgb(0xef4444));
    }

    #[test]
    fn every_visible_static_style_matches_its_reference_anchors() {
        let cases = [
            (
                ColorSchemePreset::Claude,
                AppearanceMode::Light,
                [0xfaf9f5, 0xc96442, 0x3d3929],
            ),
            (
                ColorSchemePreset::Claude,
                AppearanceMode::Dark,
                [0x262624, 0xd97757, 0xc3c0b6],
            ),
            (
                ColorSchemePreset::Catppuccin,
                AppearanceMode::Light,
                [0xeff1f5, 0x8839ef, 0x4c4f69],
            ),
            (
                ColorSchemePreset::Catppuccin,
                AppearanceMode::Dark,
                [0x1e1e2e, 0xcba6f7, 0xcdd6f4],
            ),
            (
                ColorSchemePreset::CatppuccinFrappe,
                AppearanceMode::Dark,
                [0x303446, 0xca9ee6, 0xc6d0f5],
            ),
            (
                ColorSchemePreset::CatppuccinMacchiato,
                AppearanceMode::Dark,
                [0x24273a, 0xc6a0f6, 0xcad3f5],
            ),
            (
                ColorSchemePreset::Dracula,
                AppearanceMode::Light,
                [0xfffbeb, 0x644ac9, 0x1f1f1f],
            ),
            (
                ColorSchemePreset::Dracula,
                AppearanceMode::Dark,
                [0x282a36, 0xbd93f9, 0xf8f8f2],
            ),
            (
                ColorSchemePreset::EverforestHard,
                AppearanceMode::Light,
                [0xfffbef, 0x3a94c5, 0x5c6a72],
            ),
            (
                ColorSchemePreset::Everforest,
                AppearanceMode::Light,
                [0xfdf6e3, 0x3a94c5, 0x5c6a72],
            ),
            (
                ColorSchemePreset::EverforestSoft,
                AppearanceMode::Light,
                [0xf8f0dc, 0x3a94c5, 0x5c6a72],
            ),
            (
                ColorSchemePreset::EverforestHard,
                AppearanceMode::Dark,
                [0x2b3339, 0x7fbbb3, 0xd3c6aa],
            ),
            (
                ColorSchemePreset::Everforest,
                AppearanceMode::Dark,
                [0x2d353b, 0x7fbbb3, 0xd3c6aa],
            ),
            (
                ColorSchemePreset::EverforestSoft,
                AppearanceMode::Dark,
                [0x323d43, 0x7fbbb3, 0xd3c6aa],
            ),
            (
                ColorSchemePreset::GitHub,
                AppearanceMode::Light,
                [0xffffff, 0x0969da, 0x1f2328],
            ),
            (
                ColorSchemePreset::GitHub,
                AppearanceMode::Dark,
                [0x0d1117, 0x4493f8, 0xf0f6fc],
            ),
            (
                ColorSchemePreset::GitHubDimmed,
                AppearanceMode::Dark,
                [0x212830, 0x478be6, 0xd1d7e0],
            ),
            (
                ColorSchemePreset::GitHubHighContrast,
                AppearanceMode::Light,
                [0xffffff, 0x023b95, 0x010409],
            ),
            (
                ColorSchemePreset::GitHubHighContrast,
                AppearanceMode::Dark,
                [0x010409, 0x74b9ff, 0xffffff],
            ),
            (
                ColorSchemePreset::GitHubColorblind,
                AppearanceMode::Light,
                [0xffffff, 0x0969da, 0x1f2328],
            ),
            (
                ColorSchemePreset::GitHubColorblind,
                AppearanceMode::Dark,
                [0x0d1117, 0x4493f8, 0xf0f6fc],
            ),
            (
                ColorSchemePreset::GitHubTritanopia,
                AppearanceMode::Light,
                [0xffffff, 0x0969da, 0x1f2328],
            ),
            (
                ColorSchemePreset::GitHubTritanopia,
                AppearanceMode::Dark,
                [0x0d1117, 0x4493f8, 0xf0f6fc],
            ),
            (
                ColorSchemePreset::GruvboxHard,
                AppearanceMode::Light,
                [0xf9f5d7, 0x458588, 0x3c3836],
            ),
            (
                ColorSchemePreset::Gruvbox,
                AppearanceMode::Light,
                [0xfbf1c7, 0x458588, 0x3c3836],
            ),
            (
                ColorSchemePreset::GruvboxSoft,
                AppearanceMode::Light,
                [0xf2e5bc, 0x458588, 0x3c3836],
            ),
            (
                ColorSchemePreset::GruvboxHard,
                AppearanceMode::Dark,
                [0x1d2021, 0x83a598, 0xebdbb2],
            ),
            (
                ColorSchemePreset::Gruvbox,
                AppearanceMode::Dark,
                [0x282828, 0x83a598, 0xebdbb2],
            ),
            (
                ColorSchemePreset::GruvboxSoft,
                AppearanceMode::Dark,
                [0x32302f, 0x83a598, 0xebdbb2],
            ),
            (
                ColorSchemePreset::Kanagawa,
                AppearanceMode::Light,
                [0xf2ecbc, 0x4d699b, 0x545464],
            ),
            (
                ColorSchemePreset::Kanagawa,
                AppearanceMode::Dark,
                [0x1f1f28, 0x7fb4ca, 0xdcd7ba],
            ),
            (
                ColorSchemePreset::KanagawaDragon,
                AppearanceMode::Dark,
                [0x181616, 0x8ba4b0, 0xc5c9c5],
            ),
            (
                ColorSchemePreset::Nord,
                AppearanceMode::Light,
                [0xeceff4, 0x5e81ac, 0x2e3440],
            ),
            (
                ColorSchemePreset::Nord,
                AppearanceMode::Dark,
                [0x2e3440, 0x88c0d0, 0xeceff4],
            ),
            (
                ColorSchemePreset::One,
                AppearanceMode::Light,
                [0xfafafa, 0x4078f2, 0x383a42],
            ),
            (
                ColorSchemePreset::One,
                AppearanceMode::Dark,
                [0x282c34, 0x61afef, 0xabb2bf],
            ),
            (
                ColorSchemePreset::RosePine,
                AppearanceMode::Light,
                [0xfaf4ed, 0x907aa9, 0x575279],
            ),
            (
                ColorSchemePreset::RosePine,
                AppearanceMode::Dark,
                [0x191724, 0xc4a7e7, 0xe0def4],
            ),
            (
                ColorSchemePreset::RosePineMoon,
                AppearanceMode::Dark,
                [0x232136, 0xc4a7e7, 0xe0def4],
            ),
            (
                ColorSchemePreset::Solarized,
                AppearanceMode::Light,
                [0xfdf6e3, 0x2aa198, 0x657b83],
            ),
            (
                ColorSchemePreset::Solarized,
                AppearanceMode::Dark,
                [0x002b36, 0x2aa198, 0x839496],
            ),
            (
                ColorSchemePreset::TokyoNight,
                AppearanceMode::Light,
                [0xd5d6db, 0x2e7de9, 0x565a6e],
            ),
            (
                ColorSchemePreset::TokyoNight,
                AppearanceMode::Dark,
                [0x1a1b26, 0x7aa2f7, 0xc0caf5],
            ),
            (
                ColorSchemePreset::TokyoNightStorm,
                AppearanceMode::Dark,
                [0x24283b, 0x7aa2f7, 0xc0caf5],
            ),
            (
                ColorSchemePreset::TokyoNightMoon,
                AppearanceMode::Dark,
                [0x222436, 0x82aaff, 0xc8d3f5],
            ),
        ];

        for (preset, mode, expected) in cases {
            let palette = palette(preset, mode);
            assert_eq!(
                [palette.background, palette.primary, palette.text],
                expected.map(rgb),
                "{} {mode:?}",
                preset.config_value()
            );
        }
    }
}
