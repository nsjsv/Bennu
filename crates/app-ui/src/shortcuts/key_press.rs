use iced::keyboard::{self, key, Key};

/// 一次按键：修饰键 + 键。字符键存基础层字符（iced 给的是不带修饰的逻辑键，
/// Shift+/ 送来的是 `/` 加 Shift），字母统一大写。Super 组合归窗口管理器，不在这里表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct KeyPress {
    pub(crate) control: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) key: ShortcutKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ShortcutKey {
    Character(char),
    Named(NamedKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum NamedKey {
    Enter,
    Escape,
    Space,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Copy,
    Paste,
    Cut,
    ContextMenu,
    Function(u8),
}

impl KeyPress {
    pub(crate) const fn plain(key: ShortcutKey) -> Self {
        Self {
            control: false,
            alt: false,
            shift: false,
            key,
        }
    }

    pub(crate) const fn named(key: NamedKey) -> Self {
        Self::plain(ShortcutKey::Named(key))
    }

    pub(crate) const fn ctrl(character: char) -> Self {
        Self {
            control: true,
            ..Self::plain(ShortcutKey::Character(character))
        }
    }

    pub(crate) const fn ctrl_shift(character: char) -> Self {
        Self {
            control: true,
            shift: true,
            ..Self::plain(ShortcutKey::Character(character))
        }
    }

    pub(crate) const fn alt(character: char) -> Self {
        Self {
            alt: true,
            ..Self::plain(ShortcutKey::Character(character))
        }
    }

    pub(crate) const fn alt_named(key: NamedKey) -> Self {
        Self {
            alt: true,
            ..Self::named(key)
        }
    }

    pub(crate) fn from_iced(key: &Key, modifiers: keyboard::Modifiers) -> Option<Self> {
        if modifiers.logo() {
            return None;
        }
        let key = match key.as_ref() {
            Key::Character(value) => ShortcutKey::Character(bindable_character(value)?),
            Key::Named(named) => ShortcutKey::Named(NamedKey::from_iced(named)?),
            Key::Unidentified => return None,
        };
        Some(Self {
            control: modifiers.control(),
            alt: modifiers.alt(),
            shift: modifiers.shift(),
            key,
        })
    }

    /// 配置写法 `Ctrl+Shift+N`。旧版本写的 `Primary`（Ctrl 或 Cmd）在 Linux 上就是 Ctrl。
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        let mut parts = value.split('+').map(str::trim);
        let key_part = parts.next_back()?;
        let mut press = Self::plain(ShortcutKey::from_config_part(key_part)?);
        for part in parts {
            if part.eq_ignore_ascii_case("ctrl")
                || part.eq_ignore_ascii_case("control")
                || part.eq_ignore_ascii_case("primary")
            {
                press.control = true;
            } else if part.eq_ignore_ascii_case("alt") {
                press.alt = true;
            } else if part.eq_ignore_ascii_case("shift") {
                press.shift = true;
            } else {
                return None;
            }
        }
        Some(press)
    }

    pub(crate) fn config_value(&self) -> String {
        self.joined(self.key.config_value())
    }

    /// 界面上显示的写法：方向键画箭头，翻页键缩写，其余同配置写法。
    pub(crate) fn display_label(&self) -> String {
        self.joined(self.key.display_label())
    }

    fn joined(&self, key: String) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.control {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        parts.push(key);
        parts.join("+")
    }

    /// 字母层的键：不带 Ctrl/Alt 的字符键（含 Shift+字符）。打字跳转和 vim 命令用它们，
    /// 常规快捷键不能占。
    pub(crate) fn is_letter_layer_key(&self) -> bool {
        !self.control && !self.alt && matches!(self.key, ShortcutKey::Character(_))
    }

    /// 文本框自己会用的键：聚焦的输入框在场时这些键归输入框，文件区只在确认没有
    /// 文本焦点后才用。
    pub(crate) fn is_editing_key(&self) -> bool {
        if self.control || self.alt {
            return false;
        }
        match self.key {
            ShortcutKey::Character(_) => true,
            ShortcutKey::Named(named) => matches!(
                named,
                NamedKey::Enter
                    | NamedKey::Space
                    | NamedKey::Tab
                    | NamedKey::Backspace
                    | NamedKey::Delete
                    | NamedKey::Home
                    | NamedKey::End
                    | NamedKey::PageUp
                    | NamedKey::PageDown
                    | NamedKey::ArrowLeft
                    | NamedKey::ArrowRight
                    | NamedKey::ArrowUp
                    | NamedKey::ArrowDown
            ),
        }
    }
}

impl ShortcutKey {
    fn from_config_part(part: &str) -> Option<Self> {
        if part.eq_ignore_ascii_case("plus") {
            return Some(Self::Character('+'));
        }
        bindable_character(part)
            .map(Self::Character)
            .or_else(|| NamedKey::from_config_part(part).map(Self::Named))
    }

    fn config_value(self) -> String {
        match self {
            Self::Character('+') => "Plus".to_owned(),
            Self::Character(character) => character.to_string(),
            Self::Named(named) => named.config_value().to_owned(),
        }
    }

    fn display_label(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Named(named) => named.display_label(),
        }
    }
}

/// 能绑定的字符：ASCII 字母数字和标点。别的布局的字母不进快捷键（键位表按 ASCII 写）。
fn bindable_character(value: &str) -> Option<char> {
    let mut characters = value.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    (character.is_ascii_alphanumeric() || character.is_ascii_punctuation())
        .then(|| character.to_ascii_uppercase())
}

const NAMED_KEY_NAMES: [(NamedKey, &str); 18] = [
    (NamedKey::Enter, "Enter"),
    (NamedKey::Escape, "Escape"),
    (NamedKey::Space, "Space"),
    (NamedKey::Tab, "Tab"),
    (NamedKey::Backspace, "Backspace"),
    (NamedKey::Delete, "Delete"),
    (NamedKey::Home, "Home"),
    (NamedKey::End, "End"),
    (NamedKey::PageUp, "PageUp"),
    (NamedKey::PageDown, "PageDown"),
    (NamedKey::ArrowLeft, "Left"),
    (NamedKey::ArrowRight, "Right"),
    (NamedKey::ArrowUp, "Up"),
    (NamedKey::ArrowDown, "Down"),
    (NamedKey::Copy, "Copy"),
    (NamedKey::Paste, "Paste"),
    (NamedKey::Cut, "Cut"),
    (NamedKey::ContextMenu, "Menu"),
];

impl NamedKey {
    fn from_iced(named: key::Named) -> Option<Self> {
        Some(match named {
            key::Named::Enter => Self::Enter,
            key::Named::Escape => Self::Escape,
            key::Named::Space => Self::Space,
            key::Named::Tab => Self::Tab,
            key::Named::Backspace => Self::Backspace,
            key::Named::Delete => Self::Delete,
            key::Named::Home => Self::Home,
            key::Named::End => Self::End,
            key::Named::PageUp => Self::PageUp,
            key::Named::PageDown => Self::PageDown,
            key::Named::ArrowLeft => Self::ArrowLeft,
            key::Named::ArrowRight => Self::ArrowRight,
            key::Named::ArrowUp => Self::ArrowUp,
            key::Named::ArrowDown => Self::ArrowDown,
            key::Named::Copy => Self::Copy,
            key::Named::Paste => Self::Paste,
            key::Named::Cut => Self::Cut,
            key::Named::ContextMenu => Self::ContextMenu,
            key::Named::F1 => Self::Function(1),
            key::Named::F2 => Self::Function(2),
            key::Named::F3 => Self::Function(3),
            key::Named::F4 => Self::Function(4),
            key::Named::F5 => Self::Function(5),
            key::Named::F6 => Self::Function(6),
            key::Named::F7 => Self::Function(7),
            key::Named::F8 => Self::Function(8),
            key::Named::F9 => Self::Function(9),
            key::Named::F10 => Self::Function(10),
            key::Named::F11 => Self::Function(11),
            key::Named::F12 => Self::Function(12),
            _ => return None,
        })
    }

    fn from_config_part(part: &str) -> Option<Self> {
        if let Some((named, _)) = NAMED_KEY_NAMES
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(part))
        {
            return Some(*named);
        }
        // 旧版本也接受这些写法。
        let alias = match part.to_ascii_lowercase().as_str() {
            "return" => Some(Self::Enter),
            "esc" => Some(Self::Escape),
            "del" => Some(Self::Delete),
            "arrowleft" => Some(Self::ArrowLeft),
            "arrowright" => Some(Self::ArrowRight),
            "arrowup" => Some(Self::ArrowUp),
            "arrowdown" => Some(Self::ArrowDown),
            _ => None,
        };
        if alias.is_some() {
            return alias;
        }
        let number = part
            .strip_prefix(['F', 'f'])?
            .parse::<u8>()
            .ok()
            .filter(|number| (1..=12).contains(number))?;
        Some(Self::Function(number))
    }

    fn config_value(self) -> &'static str {
        match self {
            Self::Function(number) => FUNCTION_KEY_NAMES[usize::from(number - 1)],
            named => {
                NAMED_KEY_NAMES
                    .iter()
                    .find(|(candidate, _)| *candidate == named)
                    .expect("every named key except function keys has a config name")
                    .1
            }
        }
    }

    fn display_label(self) -> String {
        match self {
            Self::Escape => "Esc",
            Self::ArrowLeft => "←",
            Self::ArrowRight => "→",
            Self::ArrowUp => "↑",
            Self::ArrowDown => "↓",
            Self::PageUp => "PgUp",
            Self::PageDown => "PgDn",
            other => other.config_value(),
        }
        .to_owned()
    }
}

const FUNCTION_KEY_NAMES: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_round_trip() {
        for value in [
            "Ctrl+Alt+L",
            "Ctrl+Shift+N",
            "F2",
            "Alt+Up",
            "Ctrl+Plus",
            "Menu",
            "Ctrl+`",
        ] {
            let press = KeyPress::from_config_value(value).expect("parse");
            assert_eq!(press.config_value(), value);
        }
    }

    #[test]
    fn old_primary_and_alias_spellings_still_parse() {
        assert_eq!(
            KeyPress::from_config_value("Primary+a"),
            Some(KeyPress::ctrl('A'))
        );
        assert_eq!(
            KeyPress::from_config_value("Alt+Return"),
            Some(KeyPress::alt_named(NamedKey::Enter))
        );
        assert_eq!(KeyPress::from_config_value("Super+A"), None);
        assert_eq!(KeyPress::from_config_value("F13"), None);
    }

    fn pressed(key: &str, modifiers: keyboard::Modifiers) -> Option<KeyPress> {
        KeyPress::from_iced(&Key::Character(key.into()), modifiers)
    }

    #[test]
    fn letters_match_either_case_and_super_never_matches() {
        let ctrl_shift = keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
        for character in ["T", "t"] {
            assert_eq!(
                pressed(character, ctrl_shift),
                Some(KeyPress::ctrl_shift('T'))
            );
        }
        assert_eq!(pressed("t", ctrl_shift | keyboard::Modifiers::LOGO), None);
        assert_eq!(
            pressed("t", keyboard::Modifiers::CTRL),
            Some(KeyPress::ctrl('T'))
        );
    }

    #[test]
    fn punctuation_is_bindable() {
        // 回归：字符归一化曾拒绝标点，Ctrl+` 注册后永远匹配不上。
        assert_eq!(
            pressed("`", keyboard::Modifiers::CTRL),
            Some(KeyPress::ctrl('`'))
        );
    }

    #[test]
    fn letter_layer_and_editing_keys() {
        assert!(KeyPress::plain(ShortcutKey::Character('J')).is_letter_layer_key());
        assert!(KeyPress {
            shift: true,
            ..KeyPress::plain(ShortcutKey::Character('G'))
        }
        .is_letter_layer_key());
        assert!(!KeyPress::ctrl('J').is_letter_layer_key());
        assert!(!KeyPress::named(NamedKey::Backspace).is_letter_layer_key());

        assert!(KeyPress::named(NamedKey::Backspace).is_editing_key());
        assert!(KeyPress::named(NamedKey::Delete).is_editing_key());
        assert!(!KeyPress::alt_named(NamedKey::ArrowUp).is_editing_key());
        assert!(!KeyPress::named(NamedKey::Function(2)).is_editing_key());
    }

    #[test]
    fn display_labels_use_arrows_and_short_page_keys() {
        assert_eq!(
            KeyPress::alt_named(NamedKey::ArrowLeft).display_label(),
            "Alt+←"
        );
        assert_eq!(
            KeyPress {
                control: true,
                ..KeyPress::named(NamedKey::PageDown)
            }
            .display_label(),
            "Ctrl+PgDn"
        );
        assert_eq!(KeyPress::ctrl('+').display_label(), "Ctrl++");
    }
}
