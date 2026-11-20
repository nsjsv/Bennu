//! 快捷键设置和存储之间的转换。存储里只有改过的动作；0.4.6 及以前存的是全部键，
//! 读到旧格式时按当时的默认值认出哪些没改过。

use std::collections::BTreeMap;

use super::action::ShortcutAction;
use super::fixed_keys::is_reserved;
use super::key_press::KeyPress;
use super::keymap::ShortcutConfig;
use crate::actions::ActionId;

/// 0.4.6 写进存储的全部条目和当时的默认键。冻结不改：只用来认出旧数据里哪些是默认值。
const LEGACY_DEFAULTS: [(&str, &str); 30] = [
    ("open_selected", "Enter"),
    ("rename_selected", "F2"),
    ("focus_path_input", "Ctrl+L"),
    ("navigate_back", "Alt+Left"),
    ("navigate_forward", "Alt+Right"),
    ("navigate_up", "Alt+Up"),
    ("move_selection_up", "Up"),
    ("move_selection_down", "Down"),
    ("move_selection_left", "Left"),
    ("move_selection_right", "Right"),
    ("file_properties", "Alt+Enter"),
    ("refresh", "F5"),
    ("escape", "Escape"),
    ("preview", "Space"),
    ("toggle_right_preview_panel", "Alt+P"),
    ("toggle_terminal", "Ctrl+`"),
    ("terminal_new_tab", "Ctrl+Shift+T"),
    ("terminal_close_tab", "Ctrl+Shift+W"),
    ("select_all", "Primary+A"),
    ("copy", "Primary+C"),
    ("duplicate_selected", "Primary+D"),
    ("new_folder_from_selection", "Ctrl+Shift+N"),
    ("copy_named", "Copy"),
    ("paste", "Primary+V"),
    ("paste_named", "Paste"),
    ("cut", "Primary+X"),
    ("cut_named", "Cut"),
    ("delete", "Delete"),
    ("undo", "Primary+Z"),
    ("redo", "Primary+Y"),
];

/// 读进来的设置，和读不懂、丢掉的条目数（调用方据此提示“部分设置已恢复默认”）。
pub(crate) struct LoadedShortcuts {
    pub(crate) config: ShortcutConfig,
    pub(crate) skipped: usize,
}

/// 存储边界的校验：单个字符键留给字母层，固定键不能改。过了这里，设置里的键都合法。
fn bindable(press: &KeyPress) -> bool {
    !press.is_letter_layer_key() && !is_reserved(press)
}

impl ShortcutConfig {
    pub(crate) fn from_stored<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a [String])>,
    ) -> LoadedShortcuts {
        let mut overrides = BTreeMap::new();
        let mut action_keys = BTreeMap::new();
        let mut skipped = 0;
        for (action_key, values) in entries {
            let mut keys = Vec::with_capacity(values.len());
            for value in values {
                match KeyPress::from_config_value(value).filter(bindable) {
                    Some(press) if !keys.contains(&press) => keys.push(press),
                    Some(_) => {}
                    None => skipped += 1,
                }
            }
            if let Some(action) = ShortcutAction::from_config_key(action_key) {
                overrides.insert(action, keys);
            } else if let Some(id) = ActionId::from_config_key(action_key) {
                action_keys.insert(id, keys);
            } else {
                skipped += 1;
            }
        }
        LoadedShortcuts {
            config: Self::from_overrides(overrides, action_keys),
            skipped,
        }
    }

    /// 动作文件暂时不在时它的键也照样存着，文件回来键就回来。
    pub(crate) fn to_stored(&self) -> Vec<(String, Vec<String>)> {
        let stored_keys = |keys: &[KeyPress]| keys.iter().map(KeyPress::config_value).collect();
        self.overrides()
            .iter()
            .map(|(action, keys)| (action.config_key().to_owned(), stored_keys(keys)))
            .chain(
                self.action_key_entries()
                    .iter()
                    .map(|(id, keys)| (id.config_key(), stored_keys(keys))),
            )
            .collect()
    }

    /// 旧格式（每个条目一个键，全部都存）。等于当时默认值的视为没改过；改过的成为这个动作
    /// 唯一的键；已经没有的条目（移动键、Esc、三个专用键）改过的话算读不懂。
    pub(crate) fn from_legacy<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> LoadedShortcuts {
        let mut overrides = BTreeMap::new();
        let mut skipped = 0;
        for (action_key, value) in entries {
            let press = KeyPress::from_config_value(value);
            let legacy_default = LEGACY_DEFAULTS
                .iter()
                .find(|(key, _)| *key == action_key)
                .and_then(|(_, default)| KeyPress::from_config_value(default));
            if press.is_some() && press == legacy_default {
                continue;
            }
            match (
                ShortcutAction::from_config_key(action_key),
                press.filter(bindable),
            ) {
                (Some(action), Some(press)) => {
                    overrides.insert(action, vec![press]);
                }
                _ => skipped += 1,
            }
        }
        LoadedShortcuts {
            config: Self::from_overrides(overrides, BTreeMap::new()),
            skipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_defaults_still_parse() {
        for (key, value) in LEGACY_DEFAULTS {
            assert!(
                KeyPress::from_config_value(value).is_some(),
                "{key} = {value}"
            );
        }
    }

    #[test]
    fn a_full_legacy_table_of_defaults_migrates_to_no_overrides() {
        let loaded = ShortcutConfig::from_legacy(LEGACY_DEFAULTS);

        assert!(loaded.config.overrides().is_empty());
        assert_eq!(loaded.skipped, 0);
        // 旧默认 Ctrl+Shift+N 不再跟着“用选中项新建文件夹”。
        assert_eq!(
            loaded
                .config
                .live(|_| false)
                .keys(ShortcutAction::NewFolderFromSelection),
            [KeyPress::ctrl('G')]
        );
    }

    #[test]
    fn customized_legacy_entries_become_overrides() {
        let loaded =
            ShortcutConfig::from_legacy([("focus_path_input", "Ctrl+Alt+L"), ("refresh", "F5")]);

        assert_eq!(loaded.skipped, 0);
        assert_eq!(
            loaded
                .config
                .live(|_| false)
                .keys(ShortcutAction::FocusPathInput),
            [KeyPress::from_config_value("Ctrl+Alt+L").unwrap()]
        );
        assert!(!loaded.config.is_customized(ShortcutAction::Refresh));
    }

    #[test]
    fn customized_removed_or_unbindable_legacy_entries_are_reported() {
        let loaded = ShortcutConfig::from_legacy([
            ("move_selection_up", "K"),
            ("copy_named", "Ctrl+Insert"),
            ("rename_selected", "R"),
            ("move_selection_down", "Down"),
        ]);

        assert!(loaded.config.overrides().is_empty());
        assert_eq!(loaded.skipped, 3);
    }

    #[test]
    fn stored_overrides_round_trip_and_hold_only_changed_actions() {
        let mut config = ShortcutConfig::default();
        config.set_keys(
            ShortcutAction::Redo,
            vec![
                KeyPress::ctrl('Y'),
                KeyPress::from_config_value("F12").unwrap(),
            ],
        );
        config.set_keys(ShortcutAction::RenameSelected, Vec::new());

        let stored = config.to_stored();
        assert_eq!(stored.len(), 2);

        let loaded = ShortcutConfig::from_stored(
            stored
                .iter()
                .map(|(action, keys)| (action.as_str(), keys.as_slice())),
        );
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.config, config);
        assert!(loaded
            .config
            .live(|_| false)
            .keys(ShortcutAction::RenameSelected)
            .is_empty());
    }

    #[test]
    fn unbindable_stored_keys_are_dropped() {
        let keys = ["J".to_owned(), "Shift+Down".to_owned(), "Ctrl+J".to_owned()];
        let loaded = ShortcutConfig::from_stored([("rename_selected", keys.as_slice())]);

        assert_eq!(loaded.skipped, 2);
        assert_eq!(
            loaded
                .config
                .live(|_| false)
                .keys(ShortcutAction::RenameSelected),
            [KeyPress::ctrl('J')]
        );
    }

    #[test]
    fn action_keys_are_stored_with_a_prefix_and_read_back() {
        let mut config = ShortcutConfig::default();
        let audio =
            crate::shortcuts::ShortcutTarget::Action(ActionId::from_file_stem("extract-audio"));
        config.set_keys(audio.clone(), vec![KeyPress::ctrl_shift('E')]);

        let stored = config.to_stored();
        assert_eq!(
            stored,
            [(
                "action:extract-audio".to_owned(),
                vec!["Ctrl+Shift+E".to_owned()]
            )]
        );

        let loaded = ShortcutConfig::from_stored(
            stored
                .iter()
                .map(|(key, values)| (key.as_str(), values.as_slice())),
        );
        assert_eq!(loaded.skipped, 0);
        assert_eq!(
            loaded.config.live(|_| true).keys(audio),
            [KeyPress::ctrl_shift('E')]
        );
        // 只有前缀、没有名字的条目读不懂。
        let unknown = [("action:".to_owned(), vec!["Ctrl+E".to_owned()])];
        let loaded = ShortcutConfig::from_stored(
            unknown
                .iter()
                .map(|(key, values)| (key.as_str(), values.as_slice())),
        );
        assert_eq!(loaded.skipped, 1);
    }
}
