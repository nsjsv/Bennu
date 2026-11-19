use super::key_press::{KeyPress, NamedKey, ShortcutKey};

/// 一次键盘移动：往哪走，以及只移光标（单选目标）还是从锚点扩选到目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionMove {
    pub(crate) step: MoveStep,
    pub(crate) extent: MoveExtent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveStep {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveExtent {
    Cursor,
    FromAnchor,
}

impl SelectionMove {
    pub(crate) const fn cursor(step: MoveStep) -> Self {
        Self {
            step,
            extent: MoveExtent::Cursor,
        }
    }

    pub(crate) const fn extend(step: MoveStep) -> Self {
        Self {
            step,
            extent: MoveExtent::FromAnchor,
        }
    }
}

/// 不在设置里改的键：它们是一套规则（移动、关闭当前层、焦点切换、第 N 个标签页），
/// 拆开改会留下半套规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixedKey {
    Escape,
    Move(SelectionMove),
    /// Alt+1…9：第几个标签页（9 是最后一个）。
    TabNumber(u8),
}

/// 移动键不加修饰是移光标，加 Shift 是扩选；带 Ctrl/Alt 的不归这里（Alt+数字除外）。
pub(crate) fn fixed_key(press: &KeyPress) -> Option<FixedKey> {
    if let ShortcutKey::Character(digit @ '1'..='9') = press.key {
        return (press.alt && !press.control && !press.shift)
            .then(|| FixedKey::TabNumber(digit as u8 - b'0'));
    }
    if press.control || press.alt {
        return None;
    }
    let ShortcutKey::Named(named) = press.key else {
        return None;
    };
    let step = match named {
        NamedKey::Escape if !press.shift => return Some(FixedKey::Escape),
        NamedKey::ArrowUp => MoveStep::Up,
        NamedKey::ArrowDown => MoveStep::Down,
        NamedKey::ArrowLeft => MoveStep::Left,
        NamedKey::ArrowRight => MoveStep::Right,
        NamedKey::PageUp => MoveStep::PageUp,
        NamedKey::PageDown => MoveStep::PageDown,
        NamedKey::Home => MoveStep::First,
        NamedKey::End => MoveStep::Last,
        _ => return None,
    };
    Some(FixedKey::Move(if press.shift {
        SelectionMove::extend(step)
    } else {
        SelectionMove::cursor(step)
    }))
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
    fn movement_keys_move_the_cursor_and_extend_with_shift() {
        assert_eq!(
            fixed_key(&KeyPress::named(NamedKey::ArrowDown)),
            Some(FixedKey::Move(SelectionMove::cursor(MoveStep::Down)))
        );
        assert_eq!(
            fixed_key(&KeyPress {
                shift: true,
                ..KeyPress::named(NamedKey::End)
            }),
            Some(FixedKey::Move(SelectionMove::extend(MoveStep::Last)))
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
