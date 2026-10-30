use crate::UiLanguage;

/// 出问题的是哪一份数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSourceText {
    /// 设置（数据库里的一行）。
    Preferences,
    /// 配置文件 config.toml。
    AppConfig,
}

/// 被恢复成默认的部分：全部，或列出的这些项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetSettingsText<'a> {
    All,
    Keys(&'a [String]),
}

pub(super) fn restored(
    language: UiLanguage,
    source: SettingsSourceText,
    reset: ResetSettingsText<'_>,
    backup: &str,
) -> String {
    use SettingsSourceText::{AppConfig, Preferences};
    match (language, source, reset) {
        (UiLanguage::English, Preferences, ResetSettingsText::All) => format!(
            "Settings could not be read and were reset to defaults. The original settings were backed up to {backup}."
        ),
        (UiLanguage::English, Preferences, ResetSettingsText::Keys(keys)) => format!(
            "Some settings could not be read and were reset to defaults: {}. The original settings were backed up to {backup}.",
            keys.join(", ")
        ),
        (UiLanguage::English, AppConfig, ResetSettingsText::All) => format!(
            "The configuration file could not be read and was reset to defaults. The original file was backed up to {backup}."
        ),
        (UiLanguage::English, AppConfig, ResetSettingsText::Keys(keys)) => format!(
            "Some configuration entries could not be read and were reset to defaults: {}. The original file was backed up to {backup}.",
            keys.join(", ")
        ),
        (UiLanguage::Chinese, Preferences, ResetSettingsText::All) => {
            format!("设置无法读取，已全部恢复默认。原设置已备份到 {backup}。")
        }
        (UiLanguage::Chinese, Preferences, ResetSettingsText::Keys(keys)) => format!(
            "部分设置无法读取，已恢复默认：{}。原设置已备份到 {backup}。",
            keys.join("、")
        ),
        (UiLanguage::Chinese, AppConfig, ResetSettingsText::All) => {
            format!("配置文件无法读取，已全部恢复默认。原文件已备份到 {backup}。")
        }
        (UiLanguage::Chinese, AppConfig, ResetSettingsText::Keys(keys)) => format!(
            "配置文件中部分项无法读取，已恢复默认：{}。原文件已备份到 {backup}。",
            keys.join("、")
        ),
    }
}

pub(super) fn not_backed_up(
    language: UiLanguage,
    source: SettingsSourceText,
    error: &str,
) -> String {
    use SettingsSourceText::{AppConfig, Preferences};
    match (language, source) {
        (UiLanguage::English, Preferences) => format!(
            "Settings could not be read, and the original settings could not be backed up ({error}). Changes to settings will not be saved this time."
        ),
        (UiLanguage::English, AppConfig) => format!(
            "The configuration file could not be read, and the original file could not be backed up ({error}). Changes will not be written to it this time."
        ),
        (UiLanguage::Chinese, Preferences) => format!(
            "设置无法读取，原设置也没能备份（{error}）。本次对设置的修改不会保存。"
        ),
        (UiLanguage::Chinese, AppConfig) => format!(
            "配置文件无法读取，原文件也没能备份（{error}）。本次不会写入配置文件。"
        ),
    }
}

pub(super) fn unavailable(language: UiLanguage, source: SettingsSourceText, error: &str) -> String {
    use SettingsSourceText::{AppConfig, Preferences};
    match (language, source) {
        (UiLanguage::English, Preferences) => format!(
            "Settings could not be loaded ({error}). Defaults are in use and changes will not be saved this time."
        ),
        (UiLanguage::English, AppConfig) => format!(
            "The configuration file could not be loaded ({error}). Defaults are in use and changes will not be written to it this time."
        ),
        (UiLanguage::Chinese, Preferences) => {
            format!("无法读取设置（{error}）。本次使用默认设置，修改不会保存。")
        }
        (UiLanguage::Chinese, AppConfig) => {
            format!("无法读取配置文件（{error}）。本次使用默认配置，不会写入配置文件。")
        }
    }
}

pub(super) fn migration_not_saved(language: UiLanguage, error: &str) -> String {
    match language {
        UiLanguage::English => format!(
            "Settings from the old configuration file could not be saved ({error}). They are in use for this session."
        ),
        UiLanguage::Chinese => {
            format!("旧配置文件中的设置没能保存（{error}），本次仍然生效。")
        }
    }
}
