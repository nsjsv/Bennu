use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
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

/// 终端聚焦时的按键翻译。`Ctrl+`` / `Ctrl+Shift+C/V`、Shift+翻页等宿主拦截键由路由层先行
/// 处理,到达这里的键都属于终端本身。`mode` 是程序当前要的键盘模式（DECCKM）。
pub(crate) fn input_for_key(key: &Key, modifiers: Modifiers, mode: TermMode) -> TerminalInput {
    match key {
        Key::Named(named) => input_for_named(*named, modifiers, mode),
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

fn input_for_named(named: key::Named, modifiers: Modifiers, mode: TermMode) -> TerminalInput {
    use key::Named as N;

    // xterm CSI 修饰编码:Shift=1 Alt=2 Ctrl=4,code = 1 + 和(1 表示无修饰,省略)。
    let modifier_code = 1
        + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control());
    // 程序开了光标键程序模式（DECCKM，vim、less 会开）时无修饰的方向键、Home、End 发 SS3。
    let csi_final = |final_byte: char| -> TerminalInput {
        if modifier_code > 1 {
            TerminalInput::Text(format!("\x1b[1;{modifier_code}{final_byte}"))
        } else if mode.contains(TermMode::APP_CURSOR) {
            TerminalInput::Text(format!("\x1bO{final_byte}"))
        } else {
            TerminalInput::Text(format!("\x1b[{final_byte}"))
        }
    };
    let tilde = |number: u8| -> TerminalInput {
        if modifier_code > 1 {
            TerminalInput::Text(format!("\x1b[{number};{modifier_code}~"))
        } else {
            TerminalInput::Text(format!("\x1b[{number}~"))
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
        N::Delete => tilde(3),
        N::Insert => TerminalInput::Bytes(b"\x1b[2~"),
        // Shift+翻页滚回看，路由层已经拿走；到这里的翻页都给程序（vim、less 翻页）。
        N::PageUp => tilde(5),
        N::PageDown => tilde(6),
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

/// Shift 加这些键滚回看（终端通行键位），不发给程序。
pub(crate) fn scrollback_for_key(key: &Key) -> Option<Scroll> {
    match key {
        Key::Named(key::Named::PageUp) => Some(Scroll::PageUp),
        Key::Named(key::Named::PageDown) => Some(Scroll::PageDown),
        Key::Named(key::Named::Home) => Some(Scroll::Top),
        Key::Named(key::Named::End) => Some(Scroll::Bottom),
        _ => None,
    }
}

/// 粘贴的字节。程序开了 bracketed paste：前后包上标记，文本里的 ESC 去掉——不然文本能自己
/// 写出结束标记，后面的字就当命令执行了。没开：换行都换成回车，跟一行行按回车一样。
/// 按字节收：拖进来的路径可以不是 UTF-8。
pub(crate) fn paste_bytes(text: &[u8], mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(text.iter().filter(|byte| **byte != 0x1b));
        bytes.extend_from_slice(b"\x1b[201~");
        return bytes;
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some((&byte, tail)) = rest.split_first() {
        match byte {
            b'\r' if tail.first() == Some(&b'\n') => {
                bytes.push(b'\r');
                rest = &tail[1..];
                continue;
            }
            b'\n' => bytes.push(b'\r'),
            byte => bytes.push(byte),
        }
        rest = tail;
    }
    bytes
}

/// 上报协议里的鼠标按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportedButton {
    Left,
    Middle,
    Right,
}

impl ReportedButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

/// 要上报给程序的一次鼠标动作。
#[derive(Debug, Clone, Copy)]
pub(crate) enum MouseAction {
    Press(ReportedButton),
    Release(ReportedButton),
    /// 移到了另一格；`held` 是按着的键。
    Move {
        held: Option<ReportedButton>,
    },
    WheelUp,
    WheelDown,
}

/// 程序开了鼠标上报时（vim、htop、mc），一次鼠标动作要发给它的字节；它没要这种动作（只要
/// 点击时的移动），或者格子超出旧编码能表示的范围时是 `None`。`line`、`column` 从 0 数。
pub(crate) fn mouse_report(
    action: MouseAction,
    line: usize,
    column: usize,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let (code, pressed) = match action {
        MouseAction::Press(button) => (button.code(), true),
        MouseAction::Release(button) => (button.code(), false),
        MouseAction::Move { held: Some(button) }
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) =>
        {
            (32 + button.code(), true)
        }
        MouseAction::Move { held: None } if mode.contains(TermMode::MOUSE_MOTION) => (35, true),
        MouseAction::Move { .. } => return None,
        MouseAction::WheelUp => (64, true),
        MouseAction::WheelDown => (65, true),
    };
    let modifier_bits = 4 * u8::from(modifiers.shift())
        + 8 * u8::from(modifiers.alt())
        + 16 * u8::from(modifiers.control());
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        return Some(
            format!(
                "\x1b[<{};{};{}{end}",
                code + modifier_bits,
                column + 1,
                line + 1
            )
            .into_bytes(),
        );
    }
    // 旧编码不分松开的是哪个键，一律是 3。
    let code = if pressed { code } else { 3 } + modifier_bits;
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let limit = if utf8 { 2015 } else { 223 };
    if line >= limit || column >= limit {
        return None;
    }
    let mut bytes = vec![0x1b, b'[', b'M', 32 + code];
    for position in [column, line] {
        let value = 33 + position as u32;
        match char::from_u32(value) {
            Some(encoded) if utf8 => {
                let mut buffer = [0; 4];
                bytes.extend_from_slice(encoded.encode_utf8(&mut buffer).as_bytes());
            }
            _ => bytes.push(value as u8),
        }
    }
    Some(bytes)
}

/// 备用屏幕里滚轮换成方向键（程序开了 alternate scroll 时）：less、man 里滚轮滚内容。
pub(crate) fn wheel_as_arrows(lines: i32) -> Vec<u8> {
    let arrow: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
    arrow.repeat(lines.unsigned_abs() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_becomes_carriage_return() {
        let input = input_for_key(
            &Key::Named(key::Named::Enter),
            Modifiers::empty(),
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Bytes(b"\r")));
    }

    #[test]
    fn ctrl_c_becomes_etx() {
        let modifiers = Modifiers::CTRL;
        let input = input_for_key(&Key::Character("c".into()), modifiers, TermMode::empty());
        assert!(matches!(input, TerminalInput::Bytes(b"\x03")));
    }

    #[test]
    fn ctrl_c_with_alt_gets_escape_prefix() {
        let modifiers = Modifiers::CTRL | Modifiers::ALT;
        let input = input_for_key(&Key::Character("c".into()), modifiers, TermMode::empty());
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b\x03"));
    }

    #[test]
    fn plain_character_passes_through() {
        let input = input_for_key(
            &Key::Character("x".into()),
            Modifiers::empty(),
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "x"));
    }

    #[test]
    fn alt_prefixes_character_with_escape() {
        let modifiers = Modifiers::ALT;
        let input = input_for_key(&Key::Character("b".into()), modifiers, TermMode::empty());
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\u{1b}b"));
    }

    #[test]
    fn arrows_produce_csi() {
        let input = input_for_key(
            &Key::Named(key::Named::ArrowUp),
            Modifiers::empty(),
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b[A"));

        let input = input_for_key(
            &Key::Named(key::Named::ArrowLeft),
            Modifiers::CTRL,
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Text(ref text) if text == "\x1b[1;5D"));
    }

    #[test]
    fn space_key_types_a_space() {
        // winit 交付 Named(Space) 而非 Character(" "),这里按实际形态断言。
        let input = input_for_key(
            &Key::Named(key::Named::Space),
            Modifiers::empty(),
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Bytes(b" ")));
    }

    #[test]
    fn shift_tab_produces_backtab() {
        let input = input_for_key(
            &Key::Named(key::Named::Tab),
            Modifiers::SHIFT,
            TermMode::empty(),
        );
        assert!(matches!(input, TerminalInput::Bytes(b"\x1b[Z")));
    }

    #[test]
    fn arrows_follow_the_cursor_key_mode_of_the_program() {
        let up = input_for_key(
            &Key::Named(key::Named::ArrowUp),
            Modifiers::empty(),
            TermMode::APP_CURSOR,
        );
        assert!(matches!(up, TerminalInput::Text(ref text) if text == "\x1bOA"));
        let home = input_for_key(
            &Key::Named(key::Named::Home),
            Modifiers::empty(),
            TermMode::APP_CURSOR,
        );
        assert!(matches!(home, TerminalInput::Text(ref text) if text == "\x1bOH"));
        // 带修饰时两种模式一样。
        let ctrl_up = input_for_key(
            &Key::Named(key::Named::ArrowUp),
            Modifiers::CTRL,
            TermMode::APP_CURSOR,
        );
        assert!(matches!(ctrl_up, TerminalInput::Text(ref text) if text == "\x1b[1;5A"));
    }

    #[test]
    fn paging_keys_go_to_the_program() {
        let page_up = input_for_key(
            &Key::Named(key::Named::PageUp),
            Modifiers::empty(),
            TermMode::empty(),
        );
        assert!(matches!(page_up, TerminalInput::Text(ref text) if text == "\x1b[5~"));
        let page_down = input_for_key(
            &Key::Named(key::Named::PageDown),
            Modifiers::CTRL,
            TermMode::empty(),
        );
        assert!(matches!(page_down, TerminalInput::Text(ref text) if text == "\x1b[6;5~"));
    }

    #[test]
    fn a_bracketed_paste_is_wrapped_and_cannot_end_itself_early() {
        let bytes = paste_bytes(b"ls\n\x1b[201~rm -rf x\n", TermMode::BRACKETED_PASTE);
        assert_eq!(bytes, b"\x1b[200~ls\n[201~rm -rf x\n\x1b[201~".to_vec());
    }

    #[test]
    fn a_plain_paste_turns_line_breaks_into_returns() {
        assert_eq!(
            paste_bytes(b"a\r\nb\nc", TermMode::empty()),
            b"a\rb\rc".to_vec()
        );
    }

    #[test]
    fn mouse_reports_follow_the_encoding_the_program_asked_for() {
        let click = TermMode::MOUSE_REPORT_CLICK;
        let press = MouseAction::Press(ReportedButton::Left);
        let release = MouseAction::Release(ReportedButton::Left);
        // 没开上报：不发。
        assert_eq!(
            mouse_report(press, 0, 0, Modifiers::empty(), TermMode::empty()),
            None
        );
        // 默认编码：32 + 键、33 + 列、33 + 行；松开一律 3。
        assert_eq!(
            mouse_report(press, 4, 9, Modifiers::empty(), click),
            Some(vec![0x1b, b'[', b'M', 32, 42, 37])
        );
        assert_eq!(
            mouse_report(release, 4, 9, Modifiers::empty(), click),
            Some(vec![0x1b, b'[', b'M', 35, 42, 37])
        );
        // SGR：松开用 m，带修饰位。
        let sgr = click | TermMode::SGR_MOUSE;
        assert_eq!(
            mouse_report(release, 4, 9, Modifiers::CTRL, sgr),
            Some(b"\x1b[<16;10;5m".to_vec())
        );
        assert_eq!(
            mouse_report(MouseAction::WheelDown, 0, 0, Modifiers::empty(), sgr),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
        // 默认编码放不下的格子不发；UTF-8 编码放得下。
        assert_eq!(mouse_report(press, 0, 300, Modifiers::empty(), click), None);
        let utf8 = mouse_report(
            press,
            0,
            300,
            Modifiers::empty(),
            click | TermMode::UTF8_MOUSE,
        )
        .unwrap();
        assert_eq!(&utf8[4..], "\u{14d}!".as_bytes());
    }

    #[test]
    fn movement_is_reported_only_in_the_modes_that_want_it() {
        let held = MouseAction::Move {
            held: Some(ReportedButton::Left),
        };
        let free = MouseAction::Move { held: None };
        let sgr = TermMode::SGR_MOUSE;
        assert_eq!(
            mouse_report(
                held,
                0,
                0,
                Modifiers::empty(),
                sgr | TermMode::MOUSE_REPORT_CLICK
            ),
            None
        );
        assert_eq!(
            mouse_report(held, 0, 0, Modifiers::empty(), sgr | TermMode::MOUSE_DRAG),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
        assert_eq!(
            mouse_report(free, 0, 0, Modifiers::empty(), sgr | TermMode::MOUSE_DRAG),
            None
        );
        assert_eq!(
            mouse_report(free, 0, 0, Modifiers::empty(), sgr | TermMode::MOUSE_MOTION),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
    }
}
