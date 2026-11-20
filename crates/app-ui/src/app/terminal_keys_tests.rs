//! 终端聚焦时，键按出来的字原样进程序。2026-11-20：终端拿的是不算 Shift 的键，vim 里打不出 `:`，
//! 大写字母和 `|` 也一样。

use iced::keyboard::{key::Named, Key, Modifiers};

use super::FileBrowser;
use crate::config;
use crate::model::Message;
use crate::terminal_panel::emulator::TerminalDimensions;
use crate::terminal_panel::session::spawn_terminal_session;
use crate::terminal_panel::tabs::{TabProgram, TerminalTab};
use file_operations::CommandInvocation;

#[test]
fn a_focused_terminal_gets_what_the_keys_type() {
    let folder = tempfile::tempdir().unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let session = spawn_terminal_session(
        1,
        &CommandInvocation {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "read line; printf %s \"$line\" > typed".into()],
            working_directory: folder.path().to_path_buf(),
        },
        TerminalDimensions {
            columns: 80,
            screen_lines: 24,
        },
    )
    .expect("spawn shell");
    browser.terminal_panel.tabs.push(TerminalTab {
        session,
        directory: folder.path().to_path_buf(),
        has_unread_output: false,
        program: TabProgram::Command,
    });
    browser.terminal_panel.active_session_id = Some(1);
    browser.terminal_panel.expand_without_shell();
    browser.terminal_panel.focused = true;

    for (pressed, typed, modifiers) in [
        (
            Key::Character(";".into()),
            Key::Character(":".into()),
            Modifiers::SHIFT,
        ),
        (
            Key::Character("a".into()),
            Key::Character("A".into()),
            Modifiers::SHIFT,
        ),
        (
            Key::Character("\\".into()),
            Key::Character("|".into()),
            Modifiers::SHIFT,
        ),
        (
            Key::Named(Named::Enter),
            Key::Named(Named::Enter),
            Modifiers::empty(),
        ),
    ] {
        drop(browser.update(Message::KeyboardKeyPressed {
            key: pressed,
            modified_key: typed,
            modifiers,
            status: iced::event::Status::Ignored,
        }));
    }

    let typed = folder.path().join("typed");
    for _ in 0..200 {
        if std::fs::read_to_string(&typed).is_ok_and(|text| !text.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(std::fs::read_to_string(&typed).unwrap(), ":A|");
    for mut tab in browser.terminal_panel.tabs.drain(..) {
        tab.session.terminate();
    }
}
