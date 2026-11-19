//! 快捷键设置和存储形态之间的转换：新格式只存改过的动作，旧格式（0.4.6 及以前的全部键）
//! 读到时迁移。

use super::super::unrecognized_settings::UnrecognizedSettings;
use super::super::{StoredShortcutOverride, StoredUserPreferences};
use crate::shortcuts::ShortcutConfig;

pub(super) fn stored_shortcut_overrides(shortcuts: &ShortcutConfig) -> Vec<StoredShortcutOverride> {
    shortcuts
        .to_stored()
        .into_iter()
        .map(|(action_key, keys)| StoredShortcutOverride { action_key, keys })
        .collect()
}

pub(super) fn shortcut_config_from_stored(
    stored: &StoredUserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> ShortcutConfig {
    let loaded = match &stored.shortcut_overrides {
        Some(overrides) => ShortcutConfig::from_stored(
            overrides
                .iter()
                .map(|entry| (entry.action_key.as_str(), entry.keys.as_slice())),
        ),
        None => ShortcutConfig::from_legacy(
            stored
                .shortcuts
                .iter()
                .map(|entry| (entry.action_key.as_str(), entry.binding.as_str())),
        ),
    };
    if loaded.skipped > 0 {
        unrecognized.note("shortcuts");
    }
    loaded.config
}
