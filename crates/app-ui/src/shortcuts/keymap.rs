use std::collections::BTreeMap;

use super::action::{ShortcutAction, ShortcutScope};
use super::key_press::KeyPress;

/// 键盘此刻归谁：终端有焦点时是终端，否则是文件区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyArea {
    FileBrowser,
    Terminal,
}

impl KeyArea {
    fn reaches(self, scope: ShortcutScope) -> bool {
        match self {
            Self::FileBrowser => scope != ShortcutScope::Terminal,
            Self::Terminal => scope != ShortcutScope::FileBrowser,
        }
    }
}

/// 快捷键设置：只存用户改过的动作（`overrides`，空列表 = 键被删光），其余跟随当前版本的
/// 默认键。`effective` 是两者合成的实际键位，每次改动后在这里重算，别处只读。
#[derive(Debug, Clone)]
pub(crate) struct ShortcutConfig {
    overrides: BTreeMap<ShortcutAction, Vec<KeyPress>>,
    effective: Vec<Vec<KeyPress>>,
}

impl PartialEq for ShortcutConfig {
    fn eq(&self, other: &Self) -> bool {
        self.overrides == other.overrides
    }
}

impl Eq for ShortcutConfig {}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self::from_overrides(BTreeMap::new())
    }
}

impl ShortcutConfig {
    pub(crate) fn from_overrides(overrides: BTreeMap<ShortcutAction, Vec<KeyPress>>) -> Self {
        let effective = effective_keys(&overrides);
        Self {
            overrides,
            effective,
        }
    }

    pub(crate) fn overrides(&self) -> &BTreeMap<ShortcutAction, Vec<KeyPress>> {
        &self.overrides
    }

    pub(crate) fn keys(&self, action: ShortcutAction) -> &[KeyPress] {
        &self.effective[action.index()]
    }

    pub(crate) fn is_customized(&self, action: ShortcutAction) -> bool {
        self.overrides.contains_key(&action)
    }

    pub(crate) fn action_for(&self, area: KeyArea, press: &KeyPress) -> Option<ShortcutAction> {
        ShortcutAction::all()
            .iter()
            .copied()
            .filter(|action| area.reaches(action.scope()))
            .find(|action| self.keys(*action).contains(press))
    }

    /// `candidate` 给 `action` 用时，和它作用域重叠、已经在用这个键的别的动作。
    pub(crate) fn conflicting_action(
        &self,
        action: ShortcutAction,
        candidate: &KeyPress,
    ) -> Option<ShortcutAction> {
        ShortcutAction::all().iter().copied().find(|other| {
            *other != action
                && other.scope().overlaps(action.scope())
                && self.keys(*other).contains(candidate)
        })
    }

    /// 和默认键一样就不算改过，存储里也就没有这一条。
    pub(crate) fn set_keys(&mut self, action: ShortcutAction, keys: Vec<KeyPress>) {
        if keys == action.default_keys() {
            self.overrides.remove(&action);
        } else {
            self.overrides.insert(action, keys);
        }
        self.effective = effective_keys(&self.overrides);
    }

    pub(crate) fn reset(&mut self, action: ShortcutAction) {
        self.overrides.remove(&action);
        self.effective = effective_keys(&self.overrides);
    }
}

/// 改过的动作用用户的键；没改过的用默认键，但用户已经拿去给别的动作（作用域重叠）的键
/// 让出来，按键只触发用户指定的那个动作。
fn effective_keys(overrides: &BTreeMap<ShortcutAction, Vec<KeyPress>>) -> Vec<Vec<KeyPress>> {
    ShortcutAction::all()
        .iter()
        .map(|action| {
            if let Some(keys) = overrides.get(action) {
                return keys.clone();
            }
            action
                .default_keys()
                .iter()
                .filter(|key| {
                    !overrides.iter().any(|(other, other_keys)| {
                        other.scope().overlaps(action.scope()) && other_keys.contains(key)
                    })
                })
                .copied()
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{self, key, Key};

    use super::*;
    use crate::shortcuts::key_press::NamedKey;

    #[test]
    fn default_keys_never_collide_within_overlapping_scopes() {
        let actions = ShortcutAction::all();
        for (index, action) in actions.iter().enumerate() {
            for other in &actions[index + 1..] {
                if !action.scope().overlaps(other.scope()) {
                    continue;
                }
                for key in action.default_keys() {
                    assert!(
                        !other.default_keys().contains(key),
                        "{action:?} and {other:?} both default to {}",
                        key.config_value()
                    );
                }
            }
        }
    }

    #[test]
    fn default_keys_are_never_letter_layer_keys() {
        for action in ShortcutAction::all() {
            for key in action.default_keys() {
                assert!(
                    !key.is_letter_layer_key(),
                    "{action:?}: {}",
                    key.config_value()
                );
            }
        }
    }

    #[test]
    fn setting_the_default_keys_removes_the_override() {
        let mut config = ShortcutConfig::default();
        config.set_keys(ShortcutAction::FocusPathInput, vec![KeyPress::ctrl('K')]);
        assert!(config.is_customized(ShortcutAction::FocusPathInput));

        config.set_keys(
            ShortcutAction::FocusPathInput,
            ShortcutAction::FocusPathInput.default_keys().to_vec(),
        );
        assert!(config.overrides().is_empty());
    }

    #[test]
    fn a_user_key_takes_over_another_actions_default_key() {
        let mut config = ShortcutConfig::default();
        config.set_keys(ShortcutAction::SelectAll, vec![KeyPress::ctrl('D')]);

        assert!(config.keys(ShortcutAction::DuplicateSelected).is_empty());
        assert_eq!(
            config.action_for(KeyArea::FileBrowser, &KeyPress::ctrl('D')),
            Some(ShortcutAction::SelectAll)
        );
    }

    #[test]
    fn terminal_tab_keys_only_count_while_the_terminal_has_focus() {
        let config = ShortcutConfig::default();
        let press = |key: Key, modifiers| KeyPress::from_iced(&key, modifiers).unwrap();
        let ctrl_shift_t = press(
            Key::Character("t".into()),
            keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
        );

        assert_eq!(
            config.action_for(KeyArea::Terminal, &ctrl_shift_t),
            Some(ShortcutAction::TerminalNewTab)
        );
        // 同一个键在文件区是恢复关闭的标签页。
        assert_eq!(
            config.action_for(KeyArea::FileBrowser, &ctrl_shift_t),
            Some(ShortcutAction::RestoreClosedTab)
        );
        assert_eq!(
            config.action_for(
                KeyArea::Terminal,
                &press(Key::Named(key::Named::F4), keyboard::Modifiers::default()),
            ),
            Some(ShortcutAction::ToggleTerminal)
        );
    }

    #[test]
    fn conflicts_respect_scopes() {
        let config = ShortcutConfig::default();
        assert_eq!(
            config.conflicting_action(ShortcutAction::Refresh, &KeyPress::ctrl('D')),
            Some(ShortcutAction::DuplicateSelected)
        );
        // 终端作用域的键不和文件区冲突。
        assert_eq!(
            config.conflicting_action(ShortcutAction::Refresh, &KeyPress::ctrl_shift('W')),
            None
        );
        assert_eq!(
            config.conflicting_action(
                ShortcutAction::TerminalNewTab,
                &KeyPress::named(NamedKey::Function(4)),
            ),
            Some(ShortcutAction::ToggleTerminal)
        );
    }
}
