use iced::keyboard::{key, Key, Modifiers};

/// 键盘事件转成的 PTY 输入。
#[derive(Debug)]
pub(crate) enum TerminalInput {
    /// 编译期常量控制序列。
    Bytes(&'static [u8]),
    /// 运行期构造的文本(普通字符、CSI 序列、ESC 前缀)。
    Text(String),
    None,
}

impl TerminalInput {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Bytes(bytes) => bytes.to_vec(),
            Self::Text(text) => text.into_bytes(),
            Self::None => Vec::new(),
        }
    }
}

/// 终端聚焦时的按键翻译。`Ctrl+`` / `Ctrl+Shift+C/V` 等宿主拦截键由路由层先行
/// 处理,到达这里的键都属于终端本身。
pub(crate) fn input_for_key(key: &Key, modifiers: Modifiers) -> TerminalInput {
    match key {
        Key::Named(named) => input_for_named(*named, modifiers),
        Key::Character(text) => input_for_character(text, modifiers),
        Key::Unidentified => TerminalInput::None,
    }
}

fn control_byte(character: char) -> Option<u8> {
    // Ctrl+letter / Ctrl+@ / Ctrl+[ \ ] ^ _ => C0 控制字节。
    let code = match character.to_ascii_lowercase() {
        'a'..='z' => character.to_ascii_lowercase() as u8 - b'a' + 1,
        '@' | ' ' => 0,
        '[' => 27,
        '\\' => 28,
        ']' => 29,
        '^' => 30,
        '_' | '/' => 31,
        _ => return None,
    };
    Some(code)
}

fn input_for_character(text: &str, modifiers: Modifiers) -> TerminalInput {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return TerminalInput::None;
    };
    // 组合输入等多字符序列按普通文本处理。
    if characters.next().is_some() && !modifiers.control() {
        return TerminalInput::Text(text.to_owned());
    }

    if modifiers.control() {
        if let Some(code) = control_byte(first) {
            let byte = [code];
            return if modifiers.alt() {
                TerminalInput::Text(escape_prefixed(&byte))
            } else {
                TerminalInput::Bytes(static_control_byte(code))
            };
        }
    }

    let mut bytes = first.to_string().into_bytes();
    if modifiers.alt() {
        bytes.insert(0, 0x1b);
    }
    match String::from_utf8(bytes) {
        Ok(text) => TerminalInput::Text(text),
        Err(_) => TerminalInput::None,
    }
}

fn static_control_byte(code: u8) -> &'static [u8] {
    const CONTROL_BYTES: [&[u8]; 32] = [
        b"\x00", b"\x01", b"\x02", b"\x03", b"\x04", b"\x05", b"\x06", b"\x07", b"\x08", b"\x09",
        b"\x0a", b"\x0b", b"\x0c", b"\x0d", b"\x0e", b"\x0f", b"\x10", b"\x11", b"\x12", b"\x13",
        b"\x14", b"\x15", b"\x16", b"\x17", b"\x18", b"\x19", b"\x1a", b"\x1b", b"\x1c", b"\x1d",
        b"\x1e", b"\x1f",
    ];
    CONTROL_BYTES[code as usize]
}

fn escape_prefixed(bytes: &[u8]) -> String {
    let mut prefixed = vec![0x1b];
    prefixed.extend_from_slice(bytes);
    String::from_utf8_lossy(&prefixed).into_owned()
}

fn input_for_named(named: key::Named, modifiers: Modifiers) -> TerminalInput {
    use key::Named as N;

    // xterm CSI 修饰编码:Shift=1 Alt=2 Ctrl=4,code = 1 + 和(1 表示无修饰,省略)。
    let modifier_code = 1
        + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control());
    let csi_final = |final_byte: char| -> TerminalInput {
        if modifier_code > 1 {
            TerminalInput::Text(format!("\x1b[1;{modifier_code}{final_byte}"))
        } else {
            TerminalInput::Text(format!("\x1b[{final_byte}"))
        }
    };

    match named {
        N::Enter => TerminalInput::Bytes(b"\r"),
        // winit 把空格交付为命名键而非 Character(" "),漏掉会直接吞键。
        N::Space => TerminalInput::Bytes(b" "),
        N::Backspace => TerminalInput::Bytes(b"\x7f"),
        N::Tab => {
            if modifiers.shift() {
                TerminalInput::Bytes(b"\x1b[Z")
            } else {
                TerminalInput::Bytes(b"\t")
            }
        }
        N::Escape => TerminalInput::Bytes(b"\x1b"),
        N::ArrowUp => csi_final('A'),
        N::ArrowDown => csi_final('B'),
        N::ArrowRight => csi_final('C'),
        N::ArrowLeft => csi_final('D'),
        N::Home => csi_final('H'),
        N::End => csi_final('F'),
        N::Delete => {
            if modifier_code > 1 {
                TerminalInput::Text(format!("\x1b[3;{modifier_code}~"))
            } else {
                TerminalInput::Text("\x1b[3~".to_owned())
            }
        }
        N::Insert => TerminalInput::Bytes(b"\x1b[2~"),
        // 无修饰的翻页交给宿主滚动回看缓冲,不转发给应用。
        N::PageUp => {
            if modifier_code > 1 {
                TerminalInput::Text(format!("\x1b[5;{modifier_code}~"))
            } else {
                TerminalInput::None
            }
        }
        N::PageDown => {
            if modifier_code > 1 {
                TerminalInput::Text(format!("\x1b[6;{modifier_code}~"))
            } else {
                TerminalInput::None
            }
        }
        N::F1 => TerminalInput::Bytes(b"\x1bOP"),
        N::F2 => TerminalInput::Bytes(b"\x1bOQ"),
        N::F3 => TerminalInput::Bytes(b"\x1bOR"),
        N::F4 => TerminalInput::Bytes(b"\x1bOS"),
        N::F5 => TerminalInput::Bytes(b"\x1b[15~"),
        N::F6 => TerminalInput::Bytes(b"\x1b[17~"),
        N::F7 => TerminalInput::Bytes(b"\x1b[18~"),
        N::F8 => TerminalInput::Bytes(b"\x1b[19~"),
        N::F9 => TerminalInput::Bytes(b"\x1b[20~"),
        N::F10 => TerminalInput::Bytes(b"\x1b[21~"),
        N::F11 => TerminalInput::Bytes(b"\x1b[23~"),
        N::F12 => TerminalInput::Bytes(b"\x1b[24~"),
        _ => TerminalInput::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_becomes_carriage_return() {
        let input = input_for_key(&Key::Named(key::Named::Enter), Modifiers::empty());
        assert!(matches!(input, TerminalInput::Bytes(b"\r")));
    }

    #[test]
    fn ctrl_c_becomes_etx() {
        let modifiers = Modifiers::CTRL;
        let input = input_for_key(&Key::Character("c".into()), modifiers);
        assert!(matches!(input, TerminalInput::Bytes(b"\x03")));
    }

    #[test]
    fn ctrl_c_with_alt_gets_escape_prefix() {
        let modifiers = Modifiers::CTRL | Modifiers::ALT;
        let input = input_for_key(&Key::Character("c".into()), modifiers);
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b\x03"));
    }

    #[test]
    fn plain_character_passes_through() {
        let input = input_for_key(&Key::Character("x".into()), Modifiers::empty());
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "x"));
    }

    #[test]
    fn alt_prefixes_character_with_escape() {
        let modifiers = Modifiers::ALT;
        let input = input_for_key(&Key::Character("b".into()), modifiers);
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\u{1b}b"));
    }

    #[test]
    fn arrows_produce_csi() {
        let input = input_for_key(&Key::Named(key::Named::ArrowUp), Modifiers::empty());
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b[A"));

        let input = input_for_key(&Key::Named(key::Named::ArrowLeft), Modifiers::CTRL);
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b[1;5D"));
    }

    #[test]
    fn space_key_types_a_space() {
        // winit 交付 Named(Space) 而非 Character(" "),这里按实际形态断言。
        let input = input_for_key(&Key::Named(key::Named::Space), Modifiers::empty());
        assert!(matches!(input, TerminalInput::Bytes(b" ")));
    }

    #[test]
    fn shift_tab_produces_backtab() {
        let input = input_for_key(&Key::Named(key::Named::Tab), Modifiers::SHIFT);
        assert!(matches!(input, TerminalInput::Bytes(b"\x1b[Z")));
    }
}
