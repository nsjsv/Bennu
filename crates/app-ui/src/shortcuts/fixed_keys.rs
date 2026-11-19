use super::key_press::{KeyPress, NamedKey, ShortcutKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileSelectionDirection {
    Up,
    Down,
    Left,
    Right,
}

/// 不在设置里改的键：它们是一套规则（移动、关闭当前层、焦点切换、第 N 个标签页），
/// 拆开改会留下半套规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixedKey {
    Escape,
    Move(FileSelectionDirection),
}

pub(crate) fn fixed_key(press: &KeyPress) -> Option<FixedKey> {
    if press.control || press.alt || press.shift {
        return None;
    }
    let ShortcutKey::Named(named) = press.key else {
        return None;
    };
    Some(match named {
        NamedKey::Escape => FixedKey::Escape,
        NamedKey::ArrowUp => FixedKey::Move(FileSelectionDirection::Up),
        NamedKey::ArrowDown => FixedKey::Move(FileSelectionDirection::Down),
        NamedKey::ArrowLeft => FixedKey::Move(FileSelectionDirection::Left),
        NamedKey::ArrowRight => FixedKey::Move(FileSelectionDirection::Right),
        _ => return None,
    })
}

/// 设置里不能录的键：固定键本身，以及它们加 Shift 的扩选形式、Tab 焦点切换、Alt+1…9。
pub(crate) fn is_reserved(press: &KeyPress) -> bool {
    match press.key {
        ShortcutKey::Named(NamedKey::Escape) => true,
        ShortcutKey::Named(NamedKey::Tab) => !press.control && !press.alt,
        ShortcutKey::Named(
            NamedKey::ArrowUp
            | NamedKey::ArrowDown
            | NamedKey::ArrowLeft
            | NamedKey::ArrowRight
            | NamedKey::Home
            | NamedKey::End
            | NamedKey::PageUp
            | NamedKey::PageDown,
        ) => !press.control && !press.alt,
        ShortcutKey::Character(digit) => {
            press.alt && !press.control && !press.shift && ('1'..='9').contains(&digit)
        }
        ShortcutKey::Named(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arrows_and_escape_are_fixed() {
        assert_eq!(
            fixed_key(&KeyPress::named(NamedKey::ArrowDown)),
            Some(FixedKey::Move(FileSelectionDirection::Down))
        );
        assert_eq!(
            fixed_key(&KeyPress::named(NamedKey::Escape)),
            Some(FixedKey::Escape)
        );
        assert_eq!(fixed_key(&KeyPress::alt_named(NamedKey::ArrowUp)), None);
    }

    #[test]
    fn reserved_keys_cover_movement_with_shift_and_alt_digits() {
        assert!(is_reserved(&KeyPress {
            shift: true,
            ..KeyPress::named(NamedKey::PageDown)
        }));
        assert!(is_reserved(&KeyPress::alt('3')));
        assert!(!is_reserved(&KeyPress::alt_named(NamedKey::ArrowLeft)));
        assert!(!is_reserved(&KeyPress::ctrl('1')));
    }
}
