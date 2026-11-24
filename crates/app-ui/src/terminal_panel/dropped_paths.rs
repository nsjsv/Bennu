//! 文件拖到终端网格上：往那个标签打带引号的路径，不回车，键盘给终端。按粘贴打进去（程序开了
//! bracketed paste 就包起来），文件名里有换行也不会被当成回车执行。

use std::path::PathBuf;

use iced::Task;

use super::follow::ShellQuoting;
use super::session::SessionId;
use super::tabs::TabProgram;
use crate::app::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    /// `session` 是拖进来时指针下那个标签；松手前它被关掉了就什么都不做。
    pub(crate) fn type_dropped_paths_into_terminal(
        &mut self,
        session: SessionId,
        paths: &[PathBuf],
    ) -> Task<Message> {
        let Some(tab) = self
            .terminal_panel
            .tabs
            .iter()
            .find(|tab| tab.session.id == session)
        else {
            return Task::none();
        };
        // 命令标签里不知道跑的是什么，按最常见的 POSIX 写法。
        let quoting = match tab.program {
            TabProgram::Shell(quoting) => quoting,
            TabProgram::Command => ShellQuoting::Posix,
        };
        tab.session.paste(&quoting.quoted_paths(paths));
        self.give_keyboard_to_terminal();
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_panel::tabs::TerminalTab;

    #[test]
    fn dropped_paths_are_typed_quoted_without_a_return_and_the_terminal_gets_the_keys() {
        let folder = tempfile::tempdir().unwrap();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let session = crate::terminal_panel::session::spawn_terminal_session(
            5,
            &file_operations::CommandInvocation {
                program: "/bin/sh".into(),
                arguments: vec!["-c".into(), "printf 'ready\\n'; read line".into()],
                working_directory: folder.path().to_path_buf(),
            },
            crate::terminal_panel::emulator_dimensions(60, 5),
            browser.terminal_panel.events.sender(),
        )
        .unwrap();
        browser.terminal_panel.tabs.push(TerminalTab {
            session,
            directory: folder.path().to_path_buf(),
            has_unread_output: false,
            program: TabProgram::Shell(ShellQuoting::Posix),
        });
        browser.terminal_panel.active_session_id = Some(5);
        browser.terminal_panel.expand_without_shell();
        let emulator = &browser.terminal_panel.tabs[0].session.emulator;
        let wait_for = |expected: &str| {
            for _ in 0..800 {
                if emulator.last_lines(1) == expected {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("the screen shows {:?}", emulator.last_lines(1));
        };
        wait_for("ready");

        drop(browser.type_dropped_paths_into_terminal(
            5,
            &[PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/it's")],
        ));

        // 回显出来、停在同一行：没有回车。
        let emulator = &browser.terminal_panel.tabs[0].session.emulator;
        for _ in 0..800 {
            if emulator.last_lines(1).starts_with('\'') {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(emulator.last_lines(1), r"'/tmp/a b' '/tmp/it'\''s'");
        assert!(browser.terminal_panel.focused);
    }

    #[test]
    fn each_shell_gets_the_quotes_it_reads() {
        let paths = [PathBuf::from("/x/it's"), PathBuf::from(r"/x/back\slash")];
        assert_eq!(
            ShellQuoting::Posix.quoted_paths(&paths),
            br"'/x/it'\''s' '/x/back\slash' ".to_vec()
        );
        assert_eq!(
            ShellQuoting::Fish.quoted_paths(&paths),
            br"'/x/it\'s' '/x/back\\slash' ".to_vec()
        );
    }
}
