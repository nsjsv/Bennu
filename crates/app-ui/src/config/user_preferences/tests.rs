use super::*;

const CUSTOM_JSON: &str = r##"{
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
fn custom_color_scheme_roundtrips_and_invalid_modes_fall_back_independently() {
    let default = default_user_config();
    let imported = CustomColorScheme::from_json(CUSTOM_JSON).expect("valid custom scheme");
    let mut config = default.clone();
    config.custom_color_scheme = imported.clone();
    config.color_scheme = ColorSchemePreset::Custom;

    let stored = config.user_preferences().to_stored();
    let restored = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(restored.custom_color_scheme, imported);
    assert_eq!(restored.color_scheme, ColorSchemePreset::Custom);

    let mut partially_invalid = config.user_preferences().to_stored();
    let custom = partially_invalid
        .custom_color_scheme
        .as_mut()
        .expect("stored custom scheme");
    custom.dark.as_mut().expect("stored dark set").text = "bad".to_owned();
    let restored = UserPreferences::from_stored(partially_invalid, &default).preferences;
    assert_eq!(restored.custom_color_scheme.light, imported.light);
    assert_eq!(
        restored.custom_color_scheme.dark,
        default.custom_color_scheme.dark
    );
}

#[test]
fn missing_custom_snapshot_uses_default_without_affecting_old_selection() {
    let mut stored = StoredUserPreferences::default();
    stored.color_scheme = "custom".to_owned();
    let restored = UserPreferences::from_stored(stored, &default_user_config()).preferences;
    assert_eq!(restored.color_scheme, ColorSchemePreset::Custom);
    assert_eq!(
        restored.custom_color_scheme,
        default_user_config().custom_color_scheme
    );
}

#[test]
fn file_grouping_roundtrips_and_legacy_missing_field_falls_back_to_none() {
    let default = default_user_config();
    let mut config = default.clone();
    config.file_grouping = FileGroupingMode::Kind;

    let stored = config.user_preferences().to_stored();
    assert_eq!(stored.file_grouping.as_deref(), Some("kind"));
    let restored = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(restored.file_grouping, FileGroupingMode::Kind);

    // 旧版本记录没有该字段：回退默认（无分组），而不是误开启分组。
    let legacy = StoredUserPreferences::default();
    assert_eq!(legacy.file_grouping, None);
    let restored = UserPreferences::from_stored(legacy, &default).preferences;
    assert_eq!(restored.file_grouping, FileGroupingMode::None);

    // 落盘值为 "none"（用户显式关闭）与缺字段回退结果一致。
    let mut explicit_none = default_user_config().user_preferences().to_stored();
    explicit_none.file_grouping = Some("none".to_owned());
    let restored = UserPreferences::from_stored(explicit_none, &default).preferences;
    assert_eq!(restored.file_grouping, FileGroupingMode::None);
}

#[test]
fn theme_selection_roundtrips_through_stored_preferences() {
    for color_scheme in ColorSchemePreset::ALL {
        let mut config = default_user_config();
        config.theme_mode = ThemeMode::Dark;
        config.color_scheme = color_scheme;

        let stored = config.user_preferences().to_stored();
        let restored = UserPreferences::from_stored(stored, &default_user_config()).preferences;

        assert_eq!(restored.theme_mode, ThemeMode::Dark);
        assert_eq!(restored.color_scheme, color_scheme);
    }
}

#[test]
fn right_preview_panel_roundtrips_and_missing_width_falls_back() {
    let default = default_user_config();
    let mut config = default.clone();
    config.right_preview_panel_open = true;
    config.right_preview_panel_width = 480.0;
    config.right_preview_preview_ratio = 0.8;

    let stored = config.user_preferences().to_stored();
    assert_eq!(stored.right_preview_panel_open, true);
    assert_eq!(stored.right_preview_panel_width, Some(480.0));
    // f32 比例升 f64 存储,只保留精度容差内相等。
    assert!((stored.right_preview_preview_ratio.unwrap() - 0.8).abs() < 1e-6);
    let restored = UserPreferences::from_stored(stored, &default).preferences;
    assert!(restored.right_preview_panel_open);
    assert_eq!(restored.right_preview_panel_width, 480.0);
    assert_eq!(restored.right_preview_preview_ratio, 0.8);

    // 旧版本数据缺宽度字段:回退默认宽度,开关回退关闭。
    let mut legacy = StoredUserPreferences::default();
    legacy.right_preview_panel_width = None;
    legacy.right_preview_preview_ratio = None;
    let restored = UserPreferences::from_stored(legacy, &default).preferences;
    assert!(!restored.right_preview_panel_open);
    assert_eq!(
        restored.right_preview_panel_width,
        default.right_preview_panel_width
    );
    assert_eq!(
        restored.right_preview_preview_ratio,
        default.right_preview_preview_ratio
    );
}

#[test]
fn invalid_theme_selection_falls_back_to_current_defaults() {
    for (theme_mode, color_scheme) in [("sepia", "claude"), ("dark", "unknown")] {
        let mut stored = StoredUserPreferences::default();
        stored.theme_mode = theme_mode.to_owned();
        stored.color_scheme = color_scheme.to_owned();

        let restored = UserPreferences::from_stored(stored, &default_user_config()).preferences;

        assert_eq!(restored.theme_mode, ThemeMode::Automatic);
        assert_eq!(restored.color_scheme, ColorSchemePreset::Default);
    }
}

#[test]
fn density_levels_roundtrip_independently_with_icon_mirror() {
    let default = default_user_config();
    let mut config = default.clone();
    config.columns_view_density = ViewDensityLevel::from_index(1);
    config.list_view_density = ViewDensityLevel::from_index(4);
    config.icons_view_density = ViewDensityLevel::from_index(6);
    config.icon_grid_size = config.icons_view_density.icon_grid_size();

    let stored = config.user_preferences().to_stored();
    assert_eq!(
        (
            stored.columns_view_density,
            stored.list_view_density,
            stored.icons_view_density,
            stored.icon_grid_size,
        ),
        (Some(1), Some(4), Some(6), 160)
    );

    let restored = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(restored.columns_view_density.index(), 1);
    assert_eq!(restored.list_view_density.index(), 4);
    assert_eq!(restored.icons_view_density.index(), 6);
    assert_eq!(restored.icon_grid_size, 160);
}

#[test]
fn density_boundary_migrates_legacy_values_and_clamps_new_indexes() {
    let default = default_user_config();
    let mut legacy_config = default.clone();
    legacy_config.icon_grid_size = 160;
    assert_eq!(
        legacy_config.user_preferences().icons_view_density.index(),
        6
    );

    let mut stored = default.user_preferences().to_stored();
    stored.columns_view_density = None;
    stored.list_view_density = None;
    stored.icons_view_density = None;
    stored.icon_grid_size = 120;
    let migrated = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(migrated.columns_view_density, ViewDensityLevel::DEFAULT);
    assert_eq!(migrated.list_view_density, ViewDensityLevel::DEFAULT);
    assert_eq!(migrated.icons_view_density.index(), 4);
    assert_eq!(migrated.icon_grid_size, 128);

    let mut stored = migrated.to_stored();
    stored.columns_view_density = Some(u8::MAX);
    stored.list_view_density = Some(0);
    stored.icons_view_density = Some(u8::MAX);
    stored.icon_grid_size = 64;
    let normalized = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(normalized.columns_view_density.index(), 8);
    assert_eq!(normalized.list_view_density.index(), 0);
    assert_eq!(normalized.icons_view_density.index(), 8);
    assert_eq!(normalized.to_stored().icon_grid_size, 192);

    let mut stored = normalized.to_stored();
    stored.icons_view_density = Some(1);
    stored.icon_grid_size = 192;
    let preferred = UserPreferences::from_stored(stored, &default).preferences;
    assert_eq!(preferred.icons_view_density.index(), 1);
    assert_eq!(preferred.icon_grid_size, 80);
}
