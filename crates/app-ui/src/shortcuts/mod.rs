mod action;
mod fixed_keys;
mod help_sheet;
mod key_press;
mod keymap;
mod letter_layer;
mod overrides;

#[cfg(test)]
pub(crate) use action::ShortcutGroup;
pub(crate) use action::{DeclaredContext, ShortcutAction};
pub(crate) use fixed_keys::{
    fixed_key, is_reserved, FixedKey, MoveExtent, MoveStep, SelectionMove,
};
pub(crate) use help_sheet::{shortcut_help_sections, HelpSection};
pub(crate) use key_press::{KeyPress, NamedKey, ShortcutKey};
pub(crate) use keymap::{KeyArea, LiveKeymap, ShortcutConfig, ShortcutTarget};
pub(crate) use letter_layer::{LetterMode, VimCommand, VimFeed, VimState};

/// 一次落到文件区的按键：可改键的命令、绑了键的动作、固定的移动键，或字母层的一个字符。
/// 经文本焦点探测后以 `Message::FileContentKeyRouted` 回到 update。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileContentKey {
    Action(ShortcutAction),
    UserAction(crate::actions::ActionId),
    Move(SelectionMove),
    Letter(TypedLetter),
}

/// 字母层收到的字符：不带 Ctrl/Alt 的字符键。`character` 是键盘布局给的不带修饰的字符
/// （Shift 另记），别的布局的字母也照样收。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypedLetter {
    pub(crate) character: char,
    pub(crate) shift: bool,
}

impl TypedLetter {
    pub(crate) fn from_iced(
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Self> {
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let iced::keyboard::Key::Character(value) = key.as_ref() else {
            return None;
        };
        let character = value
            .chars()
            .next()
            .filter(|character| !character.is_control())?;
        Some(Self {
            character,
            shift: modifiers.shift(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileBrowserShortcutOwnership {
    CapturedDeleteEvent,
    FocusedTextInputProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutRoutingContext {
    Application,
    FileBrowserContent(FileBrowserShortcutOwnership),
}

/// 按键归文本框还是归文件区，看键不看动作：文本框自己会用的编辑键一律先让给文本框；
/// 其余组合键按动作声明。实际的 Delete 被可编辑输入框原生捕获，ignored 本身就证明没有
/// 输入框在用它，不必再探测。
pub(crate) fn routing_context(
    press: &KeyPress,
    declared: DeclaredContext,
) -> ShortcutRoutingContext {
    if press.is_editing_key() {
        let ownership = if press.key == ShortcutKey::Named(NamedKey::Delete) {
            FileBrowserShortcutOwnership::CapturedDeleteEvent
        } else {
            FileBrowserShortcutOwnership::FocusedTextInputProbe
        };
        return ShortcutRoutingContext::FileBrowserContent(ownership);
    }
    match declared {
        DeclaredContext::Application => ShortcutRoutingContext::Application,
        DeclaredContext::FileBrowserContent => ShortcutRoutingContext::FileBrowserContent(
            FileBrowserShortcutOwnership::FocusedTextInputProbe,
        ),
    }
}

/// 设置页正在给哪个动作录键：替换第几个键，或加一个键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureSlot {
    Replace(usize),
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureRejection {
    Unsupported,
    LetterKey,
    Reserved,
    Conflict {
        key: KeyPress,
        target: ShortcutTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutCaptureState {
    pub(crate) target: ShortcutTarget,
    pub(crate) slot: CaptureSlot,
    pub(crate) rejection: Option<CaptureRejection>,
}

impl ShortcutCaptureState {
    pub(crate) fn new(target: ShortcutTarget, slot: CaptureSlot) -> Self {
        Self {
            target,
            slot,
            rejection: None,
        }
    }
}

impl<P: Fn(&crate::actions::ActionId) -> bool> LiveKeymap<'_, P> {
    /// 录到 `press` 之后这个目标的键列表；不能用时给出原因。
    pub(crate) fn keys_after_capture(
        &self,
        target: &ShortcutTarget,
        slot: CaptureSlot,
        press: KeyPress,
    ) -> Result<Vec<KeyPress>, CaptureRejection> {
        if press.is_letter_layer_key() {
            return Err(CaptureRejection::LetterKey);
        }
        if is_reserved(&press) {
            return Err(CaptureRejection::Reserved);
        }
        if let Some(other) = self.conflicting_target(target, &press) {
            return Err(CaptureRejection::Conflict {
                key: press,
                target: other,
            });
        }
        let mut keys = self.keys(target.clone());
        match slot {
            CaptureSlot::Replace(index) if index < keys.len() => {
                keys[index] = press;
                let mut seen = Vec::with_capacity(keys.len());
                keys.retain(|key| {
                    let first = !seen.contains(key);
                    seen.push(*key);
                    first
                });
            }
            CaptureSlot::Replace(_) | CaptureSlot::Add => {
                if !keys.contains(&press) {
                    keys.push(press);
                }
            }
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_keys_yield_to_text_input_whatever_the_action() {
        let backspace = KeyPress::named(NamedKey::Backspace);
        assert_eq!(
            routing_context(&backspace, ShortcutAction::NavigateUp.declared_context()),
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::FocusedTextInputProbe
            )
        );
        let alt_up = KeyPress::alt_named(NamedKey::ArrowUp);
        assert_eq!(
            routing_context(&alt_up, ShortcutAction::NavigateUp.declared_context()),
            ShortcutRoutingContext::Application
        );
    }

    #[test]
    fn native_delete_relies_on_widget_capture() {
        assert_eq!(
            routing_context(
                &KeyPress::named(NamedKey::Delete),
                ShortcutAction::Delete.declared_context()
            ),
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::CapturedDeleteEvent
            )
        );
        // 删除改绑到别的键时不能沿用 Delete 的捕获证明。
        assert_eq!(
            routing_context(
                &KeyPress::ctrl('K'),
                ShortcutAction::Delete.declared_context()
            ),
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::FocusedTextInputProbe
            )
        );
    }

    #[test]
    fn file_editing_combinations_probe_focused_text_input() {
        for action in [
            ShortcutAction::SelectAll,
            ShortcutAction::Copy,
            ShortcutAction::Paste,
            ShortcutAction::Cut,
            ShortcutAction::Undo,
            ShortcutAction::Redo,
        ] {
            assert_eq!(
                routing_context(&action.default_keys()[0], action.declared_context()),
                ShortcutRoutingContext::FileBrowserContent(
                    FileBrowserShortcutOwnership::FocusedTextInputProbe
                ),
                "{action:?}"
            );
        }
    }

    #[test]
    fn navigation_combinations_stay_application_level() {
        for action in [
            ShortcutAction::Refresh,
            ShortcutAction::FocusPathInput,
            ShortcutAction::NavigateBack,
            ShortcutAction::NavigateForward,
        ] {
            assert_eq!(
                routing_context(&action.default_keys()[0], action.declared_context()),
                ShortcutRoutingContext::Application,
                "{action:?}"
            );
        }
    }

    #[test]
    fn capture_rejects_letter_layer_reserved_and_conflicting_keys() {
        let config = ShortcutConfig::default();
        let rename = ShortcutAction::RenameSelected;
        let slot = CaptureSlot::Replace(0);

        assert_eq!(
            config.live(|_| false).keys_after_capture(
                &rename.into(),
                slot,
                KeyPress::plain(ShortcutKey::Character('R'))
            ),
            Err(CaptureRejection::LetterKey)
        );
        assert_eq!(
            config.live(|_| false).keys_after_capture(
                &rename.into(),
                slot,
                KeyPress::named(NamedKey::Home)
            ),
            Err(CaptureRejection::Reserved)
        );
        assert_eq!(
            config
                .live(|_| false)
                .keys_after_capture(&rename.into(), slot, KeyPress::ctrl('D')),
            Err(CaptureRejection::Conflict {
                key: KeyPress::ctrl('D'),
                target: ShortcutAction::DuplicateSelected.into(),
            })
        );
    }

    #[test]
    fn capture_replaces_or_adds_without_duplicates() {
        let config = ShortcutConfig::default();
        let redo = ShortcutAction::Redo;
        let f12 = KeyPress::named(NamedKey::Function(12));

        assert_eq!(
            config
                .live(|_| false)
                .keys_after_capture(&redo.into(), CaptureSlot::Replace(1), f12),
            Ok(vec![KeyPress::ctrl_shift('Z'), f12])
        );
        assert_eq!(
            config
                .live(|_| false)
                .keys_after_capture(&redo.into(), CaptureSlot::Add, f12),
            Ok(vec![KeyPress::ctrl_shift('Z'), KeyPress::ctrl('Y'), f12])
        );
        assert_eq!(
            config.live(|_| false).keys_after_capture(
                &redo.into(),
                CaptureSlot::Replace(1),
                KeyPress::ctrl_shift('Z')
            ),
            Ok(vec![KeyPress::ctrl_shift('Z')])
        );
    }
}
