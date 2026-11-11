//! FileChooser 窗口的外观解析：配色和侧栏样式。与主程序走同一条构造链路
//! （`bennu_theme::ApplicationTheme::active`），保证两种进程呈现同一套配色：
//! 外观偏好从主程序的状态库读，Matugen 仅在选中时读，系统深浅色由
//! daemon 订阅系统设置后传进来。portal 进程常驻，每次开窗和系统设置变化时都重新
//! 解析，否则会锁死在登录时刻。

use std::path::Path;

use bennu_state::UserSettingsStore;
use bennu_theme::window_controls::SidebarStyle;
use bennu_theme::{
    parse_matugen_colors, AppearanceMode, ApplicationTheme, ColorSchemePreset, MatugenColors,
    ThemeMode,
};
use iced::Theme;
use serde::{Deserialize, Serialize};

/// 从主程序状态库读出的外观偏好（解析失败视为未设置）。
pub(crate) struct StoredAppearance {
    pub(crate) theme_mode: ThemeMode,
    pub(crate) color_scheme: ColorSchemePreset,
    pub(crate) sidebar_style: SidebarStyle,
}

/// 选择窗、预览窗画出来的样子：一次读设置得到的配色和侧栏样式。
pub(crate) struct PickerLook {
    pub(crate) theme: Theme,
    pub(crate) sidebar_style: SidebarStyle,
}

pub(crate) fn resolve_look(system_mode: AppearanceMode) -> PickerLook {
    let appearance = read_stored_appearance(&state_database_path());
    // Matugen 文件只在显式选中时读；默认和出名配色绝不会被生成文件换色。
    let matugen = appearance
        .as_ref()
        .filter(|appearance| appearance.color_scheme == ColorSchemePreset::Matugen)
        .and_then(|_| load_matugen_colors());
    PickerLook {
        theme: compose(system_mode, appearance.as_ref(), matugen),
        sidebar_style: appearance
            .as_ref()
            .map_or(SidebarStyle::default(), |appearance| {
                appearance.sidebar_style
            }),
    }
}

/// 状态库缺失/损坏 = 未设置：跟随系统深浅 + 默认配色。
fn compose(
    system_mode: AppearanceMode,
    appearance: Option<&StoredAppearance>,
    matugen: Option<MatugenColors>,
) -> Theme {
    let mut theme = ApplicationTheme::new(system_mode);
    theme.replace_matugen(matugen);
    match appearance {
        Some(appearance) => theme.active(appearance.theme_mode, appearance.color_scheme),
        None => theme.active(ThemeMode::Automatic, ColorSchemePreset::Default),
    }
}

pub(crate) fn state_database_path() -> std::path::PathBuf {
    // 与主程序 config::default_state_database_path 同一个文件。
    file_core::app_dirs::app_data_dir().join("state.sqlite")
}

/// 设置文档里外观用到的键，缺省值与主程序的设置文档一致（旧数据没有这些键）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppearanceSettings {
    #[serde(default = "default_theme_mode")]
    theme_mode: String,
    #[serde(default = "default_color_scheme")]
    color_scheme: String,
    #[serde(default = "default_sidebar_style")]
    sidebar_style: String,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_mode: default_theme_mode(),
            color_scheme: default_color_scheme(),
            sidebar_style: default_sidebar_style(),
        }
    }
}

fn default_sidebar_style() -> String {
    SidebarStyle::default().config_value().to_owned()
}

fn default_theme_mode() -> String {
    "automatic".to_owned()
}

fn default_color_scheme() -> String {
    "default".to_owned()
}

/// 只读：读不懂的项已由存储层换成默认值；读出错时记日志、按未设置处理，不备份、不写
/// （主程序启动时会备份并提示）。
fn read_stored_appearance(db_path: &Path) -> Option<StoredAppearance> {
    let read = UserSettingsStore::new(db_path)
        .and_then(|store| store.read_user_preferences(&AppearanceSettings::default()));
    let stored = match read {
        Ok(read) => read?.preferences,
        Err(error) => {
            tracing::warn!(%error, "Bennu settings could not be read; following the system theme");
            return None;
        }
    };
    let theme_mode = ThemeMode::from_config_value(&stored.theme_mode)?;
    // 退役的配色值按主程序同一规则读成后继配色。
    let color_scheme = ColorSchemePreset::from_stored_value(&stored.color_scheme)?;
    Some(StoredAppearance {
        theme_mode,
        color_scheme,
        // 侧栏样式读不懂不连累配色：主程序那边会提示并改回默认。
        sidebar_style: SidebarStyle::from_config_value(&stored.sidebar_style).unwrap_or_default(),
    })
}

fn load_matugen_colors() -> Option<MatugenColors> {
    let path = file_core::app_dirs::app_config_dir().join("matugen.toml");
    let document = std::fs::read_to_string(path).ok()?;
    parse_matugen_colors(&document).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bennu_theme::ui_colors;

    const GENERATED: &str = include_str!("../../bennu-theme/test-data/matugen.toml");

    fn stored(theme_mode: ThemeMode, color_scheme: ColorSchemePreset) -> StoredAppearance {
        StoredAppearance {
            theme_mode,
            color_scheme,
            sidebar_style: SidebarStyle::Embedded,
        }
    }

    #[test]
    fn missing_preferences_follow_the_system_mode() {
        let dark = compose(AppearanceMode::Dark, None, None);
        assert!(dark.extended_palette().is_dark);
        assert!(
            !compose(AppearanceMode::Light, None, None)
                .extended_palette()
                .is_dark
        );
    }

    #[test]
    fn the_same_preferences_give_the_main_window_and_the_chooser_the_same_colors() {
        let preferences = stored(ThemeMode::Dark, ColorSchemePreset::Nord);
        let chooser = compose(AppearanceMode::Light, Some(&preferences), None);
        let main_window = ApplicationTheme::new(AppearanceMode::Light)
            .active(ThemeMode::Dark, ColorSchemePreset::Nord);
        assert_eq!(ui_colors(&chooser), ui_colors(&main_window));
    }

    #[test]
    fn matugen_colors_apply_only_when_selected() {
        let generated = parse_matugen_colors(GENERATED).expect("fixture must be valid");
        let selected = stored(ThemeMode::Light, ColorSchemePreset::Matugen);
        let with_file = compose(AppearanceMode::Light, Some(&selected), Some(generated));
        assert_eq!(
            with_file.palette().background,
            iced::Color::from_rgb8(0xfd, 0xf7, 0xff)
        );

        let default = stored(ThemeMode::Light, ColorSchemePreset::Default);
        assert_eq!(
            ui_colors(&compose(AppearanceMode::Light, Some(&selected), None)),
            ui_colors(&compose(AppearanceMode::Light, Some(&default), None))
        );
    }

    #[test]
    fn a_retired_color_scheme_is_read_as_its_successor() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("state.sqlite");
        UserSettingsStore::new(&database).expect("create store");
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute(
                "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
                 VALUES (?1, ?2, 1)",
                [
                    bennu_state::USER_PREFERENCES_KEY,
                    r#"{"theme_mode":"dark","color_scheme":"catppuccin-frappe"}"#,
                ],
            )
            .unwrap();

        let appearance = read_stored_appearance(&database).expect("stored appearance");

        assert_eq!(appearance.theme_mode, ThemeMode::Dark);
        assert_eq!(appearance.color_scheme, ColorSchemePreset::Catppuccin);
    }

    #[test]
    fn the_chooser_reads_the_sidebar_style_and_ignores_an_unknown_one() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("state.sqlite");
        UserSettingsStore::new(&database).expect("create store");
        let write = |payload: &str| {
            rusqlite::Connection::open(&database)
                .unwrap()
                .execute(
                    "INSERT OR REPLACE INTO user_preferences (preference_key, payload_json, updated_at_ms)
                     VALUES (?1, ?2, 1)",
                    [bennu_state::USER_PREFERENCES_KEY, payload],
                )
                .unwrap();
        };

        write(r#"{"theme_mode":"dark","color_scheme":"nord","sidebar_style":"floating"}"#);
        let appearance = read_stored_appearance(&database).expect("stored appearance");
        assert_eq!(appearance.sidebar_style, SidebarStyle::Floating);

        write(r#"{"theme_mode":"dark","color_scheme":"nord","sidebar_style":"glass"}"#);
        let appearance = read_stored_appearance(&database).expect("stored appearance");
        assert_eq!(appearance.sidebar_style, SidebarStyle::Embedded);
        assert_eq!(appearance.color_scheme, ColorSchemePreset::Nord);
    }

    #[test]
    fn missing_state_database_reads_as_unset() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = directory.path().join("nested/absent.sqlite");
        assert!(read_stored_appearance(&missing).is_none());
    }

    #[test]
    fn a_damaged_setting_keeps_the_stored_theme_and_leaves_the_database_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("state.sqlite");
        UserSettingsStore::new(&database).expect("create store");
        let payload = r#"{"theme_mode":"dark","color_scheme":"nord","show_hidden_files":"yes"}"#;
        let stored_payload = || -> String {
            rusqlite::Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT payload_json FROM user_preferences WHERE preference_key = ?1",
                    [bennu_state::USER_PREFERENCES_KEY],
                    |row| row.get(0),
                )
                .unwrap()
        };
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute(
                "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
                 VALUES (?1, ?2, 1)",
                [bennu_state::USER_PREFERENCES_KEY, payload],
            )
            .unwrap();

        let appearance = read_stored_appearance(&database).expect("stored appearance");

        assert_eq!(appearance.theme_mode, ThemeMode::Dark);
        assert_eq!(appearance.color_scheme, ColorSchemePreset::Nord);
        assert_eq!(stored_payload(), payload);
        let backups = std::fs::read_dir(directory.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("user-preferences-")
            })
            .count();
        assert_eq!(backups, 0);
    }
}
