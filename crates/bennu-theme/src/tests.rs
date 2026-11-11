use super::scheme::rgb;
use super::*;

const MODES: [(ThemeMode, AppearanceMode); 2] = [
    (ThemeMode::Light, AppearanceMode::Light),
    (ThemeMode::Dark, AppearanceMode::Dark),
];

fn matugen() -> MatugenColors {
    parse_matugen_colors(include_str!("../test-data/matugen.toml")).expect("matugen fixture")
}

/// 每套配色 × 两种深浅，外加读到 Matugen 文件后的 Matugen。
fn every_scheme() -> Vec<(ColorSchemePreset, ThemeMode, UiColorRoles)> {
    let mut theme = ApplicationTheme::new(AppearanceMode::Light);
    theme.replace_matugen(Some(matugen()));
    ColorSchemePreset::ALL
        .into_iter()
        .flat_map(|preset| MODES.map(|(theme_mode, _)| (preset, theme_mode)))
        .map(|(preset, theme_mode)| (preset, theme_mode, theme.scheme_roles(theme_mode, preset)))
        .collect()
}

#[test]
fn every_scheme_keeps_text_readable_on_every_layer() {
    for (preset, mode, roles) in every_scheme() {
        let name = format!("{preset:?} {mode:?}");
        for (layer, surface) in [
            ("window", roles.background),
            ("sidebar", roles.surface_dim),
            ("menu", roles.surface_container_low),
            ("card", roles.surface_container),
            ("hover", roles.surface_container_high),
            ("sidebar selection", roles.surface_container_highest),
        ] {
            assert!(
                contrast_ratio(roles.on_surface, surface) >= 4.5,
                "{name}: text on {layer}"
            );
            assert!(
                contrast_ratio(roles.on_surface_variant, surface) >= 3.0,
                "{name}: muted text on {layer}"
            );
        }
        assert!(
            contrast_ratio(roles.on_primary_container, roles.primary_container) >= 4.5,
            "{name}: text on selection"
        );
        assert!(
            contrast_ratio(roles.on_surface_variant, roles.primary_container) >= 3.0,
            "{name}: muted text on selection"
        );
        assert!(
            contrast_ratio(roles.primary, roles.background) >= 3.0,
            "{name}: accent"
        );
        assert!(
            contrast_ratio(roles.on_primary, roles.primary) >= 3.0,
            "{name}: label on accent"
        );
        for (status, color) in [
            ("error", roles.error),
            ("warning", roles.tertiary),
            ("success", roles.secondary),
        ] {
            assert!(
                contrast_ratio(color, roles.background) >= 3.0,
                "{name}: {status}"
            );
        }
    }
}

#[test]
fn layers_step_away_from_the_window_in_order() {
    for (preset, mode, roles) in every_scheme() {
        let distance = |color| contrast_ratio(color, roles.background);
        let steps = [
            roles.background,
            roles.surface_container_low,
            roles.surface_container,
            roles.surface_container_high,
            roles.surface_container_highest,
        ]
        .map(distance);
        assert!(
            steps.windows(2).all(|pair| pair[0] < pair[1]),
            "{preset:?} {mode:?}: {steps:?}"
        );
        assert!(
            distance(roles.outline_variant) < distance(roles.outline),
            "{preset:?} {mode:?}: divider is softer than a border"
        );
        // 侧栏陷下去：比内容区暗，但看得出是另一块。
        assert!(
            relative_luminance(roles.surface_dim) < relative_luminance(roles.background)
                && distance(roles.surface_dim) > 1.04,
            "{preset:?} {mode:?}: sidebar"
        );
    }
}

fn relative_luminance(color: Color) -> f32 {
    // 和白色的对比度反推亮度，比较明暗用。
    1.05 / contrast_ratio(color, Color::WHITE) - 0.05
}

#[test]
fn roles_survive_the_round_trip_through_the_iced_theme() {
    let theme = ApplicationTheme::new(AppearanceMode::Light);
    for (theme_mode, _) in MODES {
        for preset in ColorSchemePreset::ALL {
            assert_eq!(
                ui_colors(&theme.active(theme_mode, preset)),
                theme.scheme_roles(theme_mode, preset)
            );
        }
    }
}

#[test]
fn default_scheme_is_black_white_and_gray_without_any_tint() {
    let theme = ApplicationTheme::new(AppearanceMode::Light);
    let gray =
        |color: Color| (color.r - color.g).abs() < 0.002 && (color.g - color.b).abs() < 0.002;
    for (theme_mode, _) in MODES {
        let roles = theme.scheme_roles(theme_mode, ColorSchemePreset::Default);
        for (role, color) in [
            ("window", roles.background),
            ("sidebar", roles.surface_dim),
            ("card", roles.surface_container),
            ("accent", roles.primary),
            ("switch knob", roles.on_primary),
            ("selected file", roles.primary_container),
        ] {
            assert!(gray(color), "{theme_mode:?} {role}: {color:?}");
        }
    }
    assert_eq!(
        theme
            .scheme_roles(ThemeMode::Light, ColorSchemePreset::Default)
            .background,
        rgb(0xffffff)
    );
    // 出名配色用自己的强调色。
    assert_eq!(
        theme
            .scheme_roles(ThemeMode::Light, ColorSchemePreset::Nord)
            .primary,
        rgb(0x5e81ac)
    );
}

#[test]
fn matugen_follows_the_mode_and_looks_like_the_default_until_generated() {
    let mut theme = ApplicationTheme::new(AppearanceMode::Light);
    for (theme_mode, _) in MODES {
        assert_eq!(
            theme.scheme_roles(theme_mode, ColorSchemePreset::Matugen),
            theme.scheme_roles(theme_mode, ColorSchemePreset::Default)
        );
    }

    theme.replace_matugen(Some(matugen()));
    assert!(theme.matugen_available());
    assert_eq!(
        theme
            .scheme_roles(ThemeMode::Light, ColorSchemePreset::Matugen)
            .background,
        rgb(0xfdf7ff)
    );
    assert_eq!(
        theme
            .scheme_roles(ThemeMode::Dark, ColorSchemePreset::Matugen)
            .background,
        rgb(0x141218)
    );
    // 生成文件只影响 Matugen，默认配色不会被它换色。
    assert_ne!(
        theme
            .scheme_roles(ThemeMode::Light, ColorSchemePreset::Default)
            .background,
        rgb(0xfdf7ff)
    );
}

#[test]
fn automatic_mode_follows_the_system_and_fixed_modes_ignore_it() {
    let mut theme = ApplicationTheme::new(AppearanceMode::Light);
    for system_mode in [AppearanceMode::Light, AppearanceMode::Dark] {
        theme.replace_system_mode(system_mode);
        assert_eq!(theme.effective_mode(ThemeMode::Automatic), system_mode);
        assert_eq!(
            theme.effective_mode(ThemeMode::Light),
            AppearanceMode::Light
        );
        assert_eq!(theme.effective_mode(ThemeMode::Dark), AppearanceMode::Dark);
        assert_eq!(
            theme
                .active(ThemeMode::Automatic, ColorSchemePreset::Nord)
                .extended_palette()
                .is_dark,
            system_mode == AppearanceMode::Dark
        );
    }
}

#[test]
fn theme_modes_keep_their_stable_config_values() {
    assert_eq!(
        ThemeMode::ALL.map(ThemeMode::config_value),
        ["light", "dark", "automatic"]
    );
}
