use std::collections::BTreeMap;

use super::action::{ShortcutAction, ShortcutScope};
use super::key_press::KeyPress;
use crate::actions::ActionId;

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

/// 一个能绑键的东西：内置的命令，或者动作文件夹里的一个动作。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ShortcutTarget {
    Builtin(ShortcutAction),
    Action(ActionId),
}

impl From<ShortcutAction> for ShortcutTarget {
    fn from(action: ShortcutAction) -> Self {
        Self::Builtin(action)
    }
}

impl ShortcutTarget {
    /// 动作作用在文件区的选中项上，终端里不触发。
    fn scope(&self) -> ShortcutScope {
        match self {
            Self::Builtin(action) => action.scope(),
            Self::Action(_) => ShortcutScope::FileBrowser,
        }
    }
}

/// 快捷键设置：只存用户改过的内置动作（`overrides`，空列表 = 键被删光）和给动作绑的键
/// （`action_keys`）；其余内置动作跟随当前版本的默认键。这里只有用户设的东西，实际键位要
/// 加上"哪些动作现在在"才算得出来，见 [`LiveKeymap`]。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShortcutConfig {
    overrides: BTreeMap<ShortcutAction, Vec<KeyPress>>,
    action_keys: BTreeMap<ActionId, Vec<KeyPress>>,
}

impl ShortcutConfig {
    pub(crate) fn from_overrides(
        overrides: BTreeMap<ShortcutAction, Vec<KeyPress>>,
        action_keys: BTreeMap<ActionId, Vec<KeyPress>>,
    ) -> Self {
        Self {
            overrides,
            action_keys,
        }
    }

    pub(crate) fn overrides(&self) -> &BTreeMap<ShortcutAction, Vec<KeyPress>> {
        &self.overrides
    }

    pub(crate) fn action_key_entries(&self) -> &BTreeMap<ActionId, Vec<KeyPress>> {
        &self.action_keys
    }

    pub(crate) fn is_customized(&self, target: impl Into<ShortcutTarget>) -> bool {
        match target.into() {
            ShortcutTarget::Builtin(action) => self.overrides.contains_key(&action),
            ShortcutTarget::Action(id) => self.action_keys.contains_key(&id),
        }
    }

    /// 内置动作和默认键一样就不算改过，存储里也就没有这一条；动作删光了键也没有这一条。
    pub(crate) fn set_keys(&mut self, target: impl Into<ShortcutTarget>, keys: Vec<KeyPress>) {
        match target.into() {
            ShortcutTarget::Builtin(action) if keys == action.default_keys() => {
                self.overrides.remove(&action);
            }
            ShortcutTarget::Builtin(action) => {
                self.overrides.insert(action, keys);
            }
            ShortcutTarget::Action(id) if keys.is_empty() => {
                self.action_keys.remove(&id);
            }
            ShortcutTarget::Action(id) => {
                self.action_keys.insert(id, keys);
            }
        }
    }

    pub(crate) fn reset(&mut self, target: impl Into<ShortcutTarget>) {
        match target.into() {
            ShortcutTarget::Builtin(action) => {
                self.overrides.remove(&action);
            }
            ShortcutTarget::Action(id) => {
                self.action_keys.remove(&id);
            }
        }
    }

    /// 这份设置在 `present` 说"在"的那些动作下的实际键位。动作文件可能暂时不在（改了名、
    /// 删了、还没读完）：它的键原样留在设置里，但不起作用、也不占用别的键。
    pub(crate) fn live<P: Fn(&ActionId) -> bool>(&self, present: P) -> LiveKeymap<'_, P> {
        LiveKeymap {
            config: self,
            present,
        }
    }
}

/// 实际键位：用户设的键加上哪些动作现在在。每次查的时候现算（几十个动作，每个一两个键），
/// 设置或动作文件夹怎么变都不会看到过期的键位。
pub(crate) struct LiveKeymap<'a, P> {
    config: &'a ShortcutConfig,
    present: P,
}

impl<P: Fn(&ActionId) -> bool> LiveKeymap<'_, P> {
    /// 改过的内置动作用用户的键；没改过的用默认键，但用户已经拿去给别的目标（作用域重叠）
    /// 的键让出来，按键只触发用户指定的那个。动作的键就是给它设的键。
    pub(crate) fn keys(&self, target: impl Into<ShortcutTarget>) -> Vec<KeyPress> {
        let action = match target.into() {
            ShortcutTarget::Action(id) => {
                return self
                    .config
                    .action_keys
                    .get(&id)
                    .cloned()
                    .unwrap_or_default()
            }
            ShortcutTarget::Builtin(action) => action,
        };
        if let Some(keys) = self.config.overrides.get(&action) {
            return keys.clone();
        }
        action
            .default_keys()
            .iter()
            .filter(|key| !self.taken_from(action, key))
            .copied()
            .collect()
    }

    /// 这个键在 `area` 里触发什么。
    pub(crate) fn target_for(&self, area: KeyArea, press: &KeyPress) -> Option<ShortcutTarget> {
        self.live_targets()
            .filter(|target| area.reaches(target.scope()))
            .find(|target| self.keys(target.clone()).contains(press))
    }

    /// `candidate` 给 `target` 用时，和它作用域重叠、已经在用这个键的别的目标。
    pub(crate) fn conflicting_target(
        &self,
        target: &ShortcutTarget,
        candidate: &KeyPress,
    ) -> Option<ShortcutTarget> {
        self.live_targets().find(|other| {
            other != target
                && other.scope().overlaps(target.scope())
                && self.keys(other.clone()).contains(candidate)
        })
    }

    /// 用户把 `key` 给了作用域和 `action` 重叠的别的目标（在的动作才算）。
    fn taken_from(&self, action: ShortcutAction, key: &KeyPress) -> bool {
        let scope = action.scope();
        self.config.overrides.iter().any(|(other, keys)| {
            *other != action && other.scope().overlaps(scope) && keys.contains(key)
        }) || self.config.action_keys.iter().any(|(id, keys)| {
            (self.present)(id)
                && ShortcutTarget::Action(id.clone()).scope().overlaps(scope)
                && keys.contains(key)
        })
    }

    /// 起作用的目标：所有内置动作，加上在的动作。
    fn live_targets(&self) -> impl Iterator<Item = ShortcutTarget> + '_ {
        ShortcutAction::all()
            .iter()
            .copied()
            .map(ShortcutTarget::Builtin)
            .chain(
                self.config
                    .action_keys
                    .keys()
                    .filter(|id| (self.present)(id))
                    .cloned()
                    .map(ShortcutTarget::Action),
            )
    }
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

        assert!(config
            .live(no_actions)
            .keys(ShortcutAction::DuplicateSelected)
            .is_empty());
        assert_eq!(
            config
                .live(no_actions)
                .target_for(KeyArea::FileBrowser, &KeyPress::ctrl('D')),
            Some(ShortcutAction::SelectAll.into())
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
            config
                .live(no_actions)
                .target_for(KeyArea::Terminal, &ctrl_shift_t),
            Some(ShortcutAction::TerminalNewTab.into())
        );
        // 同一个键在文件区是恢复关闭的标签页。
        assert_eq!(
            config
                .live(no_actions)
                .target_for(KeyArea::FileBrowser, &ctrl_shift_t),
            Some(ShortcutAction::RestoreClosedTab.into())
        );
        assert_eq!(
            config.live(no_actions).target_for(
                KeyArea::Terminal,
                &press(Key::Named(key::Named::F4), keyboard::Modifiers::default()),
            ),
            Some(ShortcutAction::ToggleTerminal.into())
        );
    }

    #[test]
    fn conflicts_respect_scopes() {
        let config = ShortcutConfig::default();
        assert_eq!(
            config
                .live(no_actions)
                .conflicting_target(&ShortcutAction::Refresh.into(), &KeyPress::ctrl('D')),
            Some(ShortcutAction::DuplicateSelected.into())
        );
        // 终端作用域的键不和文件区冲突。
        assert_eq!(
            config
                .live(no_actions)
                .conflicting_target(&ShortcutAction::Refresh.into(), &KeyPress::ctrl_shift('W')),
            None
        );
        assert_eq!(
            config.live(no_actions).conflicting_target(
                &ShortcutAction::TerminalNewTab.into(),
                &KeyPress::named(NamedKey::Function(4)),
            ),
            Some(ShortcutAction::ToggleTerminal.into())
        );
    }

    fn no_actions(_: &ActionId) -> bool {
        false
    }

    #[test]
    fn an_action_key_works_and_takes_over_a_default_only_while_the_action_is_there() {
        let mut config = ShortcutConfig::default();
        let audio_id = ActionId::from_file_stem("extract-audio");
        let audio = ShortcutTarget::Action(audio_id.clone());
        config.set_keys(audio.clone(), vec![KeyPress::ctrl('D')]);
        let present = config.live(|id: &ActionId| *id == audio_id);
        let absent = config.live(no_actions);

        // 动作文件不在：键不起作用，也不占用创建副本的默认键。
        assert_eq!(
            absent.target_for(KeyArea::FileBrowser, &KeyPress::ctrl('D')),
            Some(ShortcutAction::DuplicateSelected.into())
        );
        assert_eq!(
            absent.keys(audio.clone()),
            [KeyPress::ctrl('D')],
            "the keys are kept"
        );

        assert_eq!(
            present.target_for(KeyArea::FileBrowser, &KeyPress::ctrl('D')),
            Some(audio.clone())
        );
        assert!(present.keys(ShortcutAction::DuplicateSelected).is_empty());
        // 终端里不触发动作。
        assert_eq!(
            present.target_for(KeyArea::Terminal, &KeyPress::ctrl('D')),
            None
        );
        assert_eq!(
            present.conflicting_target(&ShortcutAction::Refresh.into(), &KeyPress::ctrl('D')),
            Some(audio.clone())
        );

        config.set_keys(audio, Vec::new());
        assert!(config.action_key_entries().is_empty());
    }
}
