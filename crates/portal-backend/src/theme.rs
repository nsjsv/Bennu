//! FileChooser 窗口的启动主题解析。与主程序走同一条构造链路
//! （`bennu_theme::ApplicationTheme::active`），保证两种进程呈现同一套
//! 配色：显式预设/Custom 按存储值解析，matugen 仅在选中时生效，其余
//! 情况回退系统深浅检测。portal 进程常驻，主题在每次开窗时重新调用
//! 本解析（见 main.rs 的 open_picker_window），否则会锁死在登录时刻；
//! 窗口存续期间不做热跟随。

use std::path::Path;

use bennu_theme::{
    default_custom_color_scheme, fallback_theme, parse_matugen_theme, ApplicationTheme,
    ColorSchemePreset, CustomColorScheme, ThemeMode,
};
use file_operation_store::TaskQueueStore;
use iced::Theme;

use crate::location;

/// 从主程序状态库读出的外观偏好（解析失败视为未设置）。
pub(crate) struct StoredAppearance {
    pub(crate) theme_mode: ThemeMode,
    pub(crate) color_scheme: ColorSchemePreset,
    pub(crate) custom_color_scheme: CustomColorScheme,
}

pub(crate) fn resolve_startup_theme() -> Theme {
    let detected = detected_mode();
    let mut theme_state = ApplicationTheme::new(fallback_theme(detected));
    match read_stored_appearance(&state_database_path()) {
        Some(appearance) => {
            theme_state.replace_custom_color_scheme(appearance.custom_color_scheme.clone());
            // matugen 覆盖只在显式选中时加载；Default/预设绝不能被生成文件换色。
            if appearance.color_scheme == ColorSchemePreset::Matugen {
                theme_state.replace_matugen_override(load_matugen_override());
            }
            theme_state.active(appearance.theme_mode, appearance.color_scheme)
        }
        // 状态库缺失/损坏 = 未设置：跟随系统深浅 + 默认配色。
        None => theme_state.active(ThemeMode::Automatic, ColorSchemePreset::Default),
    }
}

fn detected_mode() -> bennu_theme::AppearanceMode {
    // dark-light 在 Linux 走 async-std 的 block_on，主线程可同步调用；
    // 检测失败（无 portal 会话等）回退浅色，与主程序启动检测同一兜底。
    match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => bennu_theme::AppearanceMode::Dark,
        Ok(dark_light::Mode::Light) | Ok(dark_light::Mode::Unspecified) | Err(_) => {
            bennu_theme::AppearanceMode::Light
        }
    }
}

pub(crate) fn state_database_path() -> std::path::PathBuf {
    // 与主程序 config::default_state_database_path 同一约定：
    // $XDG_DATA_HOME/bennu/state.sqlite。
    let fallback_base = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    dirs::data_dir()
        .unwrap_or(fallback_base)
        .join("bennu")
        .join("state.sqlite")
}

fn read_stored_appearance(db_path: &Path) -> Option<StoredAppearance> {
    let store = TaskQueueStore::new(db_path).ok()?;
    let stored = store.read_user_preferences().ok()??;
    let theme_mode = ThemeMode::from_config_value(&stored.theme_mode)?;
    let color_scheme = ColorSchemePreset::from_config_value(&stored.color_scheme)?;
    let custom_color_scheme = CustomColorScheme::from_stored(
        stored.custom_color_scheme.as_ref(),
        &default_custom_color_scheme(),
    );
    Some(StoredAppearance {
        theme_mode,
        color_scheme,
        custom_color_scheme,
    })
}

fn load_matugen_override() -> Option<Theme> {
    let path = location::app_config_dir()?.join("matugen.toml");
    let document = std::fs::read_to_string(path).ok()?;
    parse_matugen_theme(&document).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bennu_theme::{ui_colors, AppearanceMode};

    /// 组合规则与主程序 ApplicationTheme::active 一致；这里用注入的
    /// StoredAppearance 验证优先级，绕开 50 字段的存储结构构造。
    fn compose(
        fallback: Theme,
        appearance: Option<&StoredAppearance>,
        matugen: Option<Theme>,
    ) -> Theme {
        let mut theme_state = ApplicationTheme::new(fallback);
        if let Some(appearance) = appearance {
            theme_state.replace_custom_color_scheme(appearance.custom_color_scheme.clone());
            if appearance.color_scheme == ColorSchemePreset::Matugen {
                theme_state.replace_matugen_override(matugen);
            }
            return theme_state.active(appearance.theme_mode, appearance.color_scheme);
        }
        theme_state.active(ThemeMode::Automatic, ColorSchemePreset::Default)
    }

    fn stored(theme_mode: ThemeMode, color_scheme: ColorSchemePreset) -> StoredAppearance {
        StoredAppearance {
            theme_mode,
            color_scheme,
            custom_color_scheme: default_custom_color_scheme(),
        }
    }

    #[test]
    fn missing_preferences_follow_the_detected_system_mode() {
        let light = fallback_theme(AppearanceMode::Light);
        let dark = fallback_theme(AppearanceMode::Dark);

        assert_eq!(
            ui_colors(&compose(light.clone(), None, None)),
            ui_colors(&light)
        );
        // automatic + default = 系统回退原样，预设不会因 matugen 文件存在而换色。
        let with_unused_override = compose(dark.clone(), None, Some(light));
        assert_eq!(ui_colors(&with_unused_override), ui_colors(&dark));
    }

    #[test]
    fn explicit_preset_ignores_a_valid_matugen_file() {
        let fallback = fallback_theme(AppearanceMode::Dark);
        let generated = parse_matugen_theme(include_str!(
            "../../bennu-theme/test-data/matugen-light.toml"
        ))
        .expect("fixture must be valid");

        let theme = compose(
            fallback.clone(),
            Some(&stored(ThemeMode::Dark, ColorSchemePreset::Nord)),
            Some(generated),
        );
        assert!(theme.extended_palette().is_dark);
        // Nord 深色 ≠ 系统回退背景，也 ≠ matugen 生成背景。
        assert_ne!(ui_colors(&theme), ui_colors(&fallback));
    }

    #[test]
    fn selected_matugen_wins_and_missing_file_falls_back() {
        let fallback = fallback_theme(AppearanceMode::Light);
        let generated = parse_matugen_theme(include_str!(
            "../../bennu-theme/test-data/matugen-light.toml"
        ))
        .expect("fixture must be valid");
        let generated_background = generated.palette().background;

        let selected = stored(ThemeMode::Automatic, ColorSchemePreset::Matugen);
        let with_override = compose(fallback.clone(), Some(&selected), Some(generated));
        assert_eq!(with_override.palette().background, generated_background);

        let without_override = compose(fallback.clone(), Some(&selected), None);
        assert_eq!(ui_colors(&without_override), ui_colors(&fallback));
    }

    #[test]
    fn missing_state_database_reads_as_unset() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = directory.path().join("nested/absent.sqlite");
        assert!(read_stored_appearance(&missing).is_none());
    }
}
