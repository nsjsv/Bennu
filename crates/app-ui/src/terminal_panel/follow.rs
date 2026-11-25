//! 终端和文件区互相跟着换目录（Dolphin 做法）。
//!
//! - 文件区的目标变了 → 激活的 shell 标签 cd 过去：只在 shell 空闲（前台没有程序）时，先用
//!   SIGINT 清掉敲了一半的命令——不然 `rm -rf *` 后面接上 cd 就成了另一条命令。
//! - shell 自己换了目录（屏幕变了时看到的，见 `TerminalSession::directory_change`）→ 文件区
//!   跳过去：只在终端有键盘焦点时。文件区连换几个目录时，可能晚一步才看到我们自己发的某个
//!   cd；谁有焦点谁说了算，这种迟到的变化就不会把文件区拽回去，不用记哪些 cd 是我们发的。

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use iced::Task;

use super::session::SessionId;
use super::tabs::TabProgram;
use crate::app::FileBrowser;
use crate::model::{BrowserViewMode, Message, NavigationMode};

/// shell 认哪种单引号写法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellQuoting {
    /// fish：单引号里 `\'`、`\\` 是转义。
    Fish,
    /// sh、bash、zsh 这一类：单引号里什么都不转义，引号本身写成 `'\''`。
    Posix,
}

impl ShellQuoting {
    pub(crate) fn of(shell: &Path) -> Self {
        if shell.file_name().is_some_and(|name| name == "fish") {
            Self::Fish
        } else {
            Self::Posix
        }
    }

    /// 让 shell 换到 `directory` 的那一行：前导空格不进历史（bash 的 ignorespace、zsh 的
    /// HIST_IGNORE_SPACE、fish 默认）；路径都是绝对路径，不会被当成选项，不用 `--`。
    fn cd_line(self, directory: &Path) -> Vec<u8> {
        let mut line = b" cd ".to_vec();
        self.push_quoted(&mut line, directory);
        line.push(b'\r');
        line
    }

    /// 拖进终端的路径：各自加引号，后面各跟一个空格（接着打字不用自己补），不回车。
    pub(crate) fn quoted_paths(self, paths: &[PathBuf]) -> Vec<u8> {
        let mut text = Vec::new();
        for path in paths {
            self.push_quoted(&mut text, path);
            text.push(b' ');
        }
        text
    }

    /// 按字节拼：路径可以不是 UTF-8。
    fn push_quoted(self, text: &mut Vec<u8>, path: &Path) {
        text.push(b'\'');
        for &byte in path.as_os_str().as_bytes() {
            match (self, byte) {
                (Self::Fish, b'\'' | b'\\') => text.extend_from_slice(&[b'\\', byte]),
                (Self::Posix, b'\'') => text.extend_from_slice(b"'\\''"),
                _ => text.push(byte),
            }
        }
        text.push(b'\'');
    }
}

impl FileBrowser {
    /// 终端跟随的目标目录:多栏视图取鼠标悬停的栏,其余视图取活动窗格目录。
    /// 多栏视图鼠标不在任何栏上时返回 None(保持现状,避免来回跳)。
    fn terminal_panel_follow_target(&self) -> Option<PathBuf> {
        let tab = self.tab();
        if tab.view_mode() != BrowserViewMode::Columns {
            return tab.current_directory().map(Path::to_path_buf);
        }
        self.pointer_hovered_column_directory()
    }

    /// 跟随目标可能变化的转换（导航、窗格/标签切换、视图模式切换、路径迁移、
    /// 多栏悬停、面板展开）在返回前调用:目标变了就让**激活标签**的 shell cd 过去。
    /// 切换终端标签不调:各标签保住自己的目录,否则每个标签都会被拉回文件区目录。
    /// 快照在面板收起时清空,让下次展开立即同步一次。
    pub(crate) fn follow_terminal_panel_directory(&mut self) {
        if !self.terminal_panel.expanded {
            self.terminal_panel.follow_target_snapshot = None;
            return;
        }
        let Some(directory) = self.terminal_panel_follow_target() else {
            return;
        };
        if self.terminal_panel.follow_target_snapshot.as_ref() == Some(&directory) {
            return;
        }
        self.terminal_panel.follow_target_snapshot = Some(directory.clone());
        // 压缩包（包本身和包里面）不是 shell 进得去的目录：终端留在原地，一般就是包所在的文件夹。
        if file_core::archive_path_identity(&directory) != file_core::ArchivePathIdentity::RealFile
        {
            return;
        }
        let Some(active_tab) = self.terminal_panel.active_tab_mut() else {
            return;
        };
        let TabProgram::Shell(quoting) = active_tab.program else {
            return;
        };
        // shell 已经在（或已经被叫去）这个目录：比如从压缩包退回包所在的文件夹。
        // 前台有程序（vim、ssh……）时不动它：打进去的字会进那个程序。
        if active_tab.directory == directory || !active_tab.session.shell_is_idle() {
            return;
        }
        active_tab.session.signal(libc::SIGINT);
        active_tab.session.write_input(&quoting.cd_line(&directory));
        // 先记成它的目录：看到 shell 真换过去之前文件区又换回原来的目录（A→B→A）时照样发 cd。
        active_tab.directory = directory;
    }

    /// shell 标签的 shell 换了目录：记下（标签标题、跟随判断用）；用户正在终端里打字时文件区
    /// 跟过去。
    pub(super) fn accept_shell_directory(
        &mut self,
        session: SessionId,
        directory: PathBuf,
    ) -> Task<Message> {
        let typing_here =
            self.terminal_has_keyboard() && self.terminal_panel.active_session_id == Some(session);
        let Some(tab) = self
            .terminal_panel
            .tabs
            .iter_mut()
            .find(|tab| tab.session.id == session)
        else {
            return Task::none();
        };
        tab.directory = directory.clone();
        if !typing_here || self.terminal_panel.follow_target_snapshot.as_ref() == Some(&directory) {
            return Task::none();
        }
        // 先记成跟随目标：导航里的跟随检查看到目标没变，不会再给 shell 发一次 cd。
        self.terminal_panel.follow_target_snapshot = Some(directory.clone());
        self.navigate_to(directory, NavigationMode::KeepHistory)
    }
}

#[cfg(test)]
mod tests {
    use file_operations::CommandInvocation;

    use super::*;
    use crate::terminal_panel::session::spawn_terminal_session;
    use crate::terminal_panel::tabs::TerminalTab;

    /// 列表视图、终端展开、激活标签是跑着 `script` 的 shell 标签。
    fn browser_with_shell(directory: &Path, script: &str) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let mut tab =
            crate::model::BrowserTab::directory(browser.active_tab_id(), directory.to_path_buf());
        tab.choose_view_mode(BrowserViewMode::List);
        *browser.tab_mut() = tab;
        let session = spawn_terminal_session(
            7,
            &CommandInvocation {
                program: "/bin/sh".into(),
                arguments: vec!["-c".into(), script.into()],
                working_directory: directory.to_path_buf(),
            },
            crate::terminal_panel::emulator_dimensions(80, 24),
            browser.terminal_panel.events.sender(),
        )
        .expect("spawn shell");
        browser.terminal_panel.tabs.push(TerminalTab {
            session,
            directory: directory.to_path_buf(),
            has_unread_output: false,
            program: TabProgram::Shell(ShellQuoting::Posix),
        });
        browser.terminal_panel.active_session_id = Some(7);
        browser.terminal_panel.expand_without_shell();
        browser
    }

    fn wait_for_file(path: &Path) -> String {
        for _ in 0..800 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("{} was never written", path.display());
    }

    /// 等 shell 打出提示符、在 `read` 上等着。
    fn wait_until_idle(browser: &FileBrowser) {
        let shell = &browser.terminal_panel.active_tab().unwrap().session;
        for _ in 0..800 {
            if shell.shell_is_idle() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("the shell never got ready for input");
    }

    #[test]
    fn entering_an_archive_and_coming_back_leaves_the_shell_alone() {
        // 2026-11-22 用户：进了压缩包，终端里打出 `cd '…/ventoy….zip'`，bash 说不是目录。
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let archive = root.join("bundle.zip");
        std::fs::write(&archive, b"").unwrap();
        let mut browser = browser_with_shell(
            &root,
            "trap '' INT; printf '$ '; if read -r -t 3 line; then echo \"$line\"; else echo none; fi > got",
        );
        wait_until_idle(&browser);

        drop(browser.navigate_to(archive, NavigationMode::KeepHistory));
        drop(browser.navigate_to(root.clone(), NavigationMode::KeepHistory));

        assert_eq!(wait_for_file(&root.join("got")).trim_end(), "none");
    }

    #[test]
    fn navigating_sends_the_idle_shell_a_quoted_cd_after_clearing_its_line() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let target = root.join("it's here");
        std::fs::create_dir(&target).unwrap();
        // 不理 SIGINT（交互 shell 收到它只是清掉当前行），把收到的那一行当命令执行。
        let mut browser = browser_with_shell(
            &root,
            "trap '' INT; printf '$ '; read -r line; eval \"$line\"; pwd > \"$OLDPWD/got\"",
        );

        wait_until_idle(&browser);
        drop(browser.navigate_to(target.clone(), NavigationMode::KeepHistory));

        assert_eq!(
            wait_for_file(&root.join("got")).trim_end(),
            target.to_str().unwrap()
        );
    }

    #[test]
    fn a_directory_change_in_the_focused_terminal_moves_the_file_area() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let inner = root.join("inner");
        std::fs::create_dir(&inner).unwrap();
        let mut browser = browser_with_shell(&root, "read x");
        browser.terminal_panel.focused = true;

        drop(browser.accept_shell_directory(7, inner.clone()));

        assert_eq!(browser.tab().current_directory(), Some(inner.as_path()));
        assert_eq!(
            browser.terminal_panel.active_tab().unwrap().directory,
            inner
        );
    }

    #[tokio::test]
    async fn a_cd_in_the_focused_terminal_moves_the_file_area_once_the_prompt_shows() {
        use iced::futures::StreamExt;

        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let inner = root.join("inner");
        std::fs::create_dir(&inner).unwrap();
        let mut browser = browser_with_shell(&root, "cd inner && printf '$ ' && read x");
        browser.terminal_panel.focused = true;

        let mut events = Box::pin(browser.terminal_panel.events.stream());
        while browser.tab().current_directory() != Some(inner.as_path()) {
            let message = tokio::time::timeout(std::time::Duration::from_secs(20), events.next())
                .await
                .expect("the shell's prompt arrives")
                .expect("the event stream stays open");
            if let Message::TerminalPanel(message) = message {
                drop(browser.handle_terminal_panel_message(message));
            }
        }
        assert_eq!(
            browser.terminal_panel.active_tab().unwrap().directory,
            inner
        );
    }

    #[test]
    fn a_late_report_while_the_file_area_has_focus_does_not_drag_it_back() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let inner = root.join("inner");
        std::fs::create_dir(&inner).unwrap();
        let mut browser = browser_with_shell(&root, "read x");
        browser.terminal_panel.focused = false;

        drop(browser.accept_shell_directory(7, inner.clone()));

        assert_eq!(browser.tab().current_directory(), Some(root.as_path()));
        // 标签标题照样跟着 shell 的真实目录。
        assert_eq!(
            browser.terminal_panel.active_tab().unwrap().directory,
            inner
        );
    }

    #[test]
    fn the_cd_line_quotes_the_way_each_shell_reads_it() {
        let path = Path::new("/home/a b/it's\\here");
        assert_eq!(
            ShellQuoting::Posix.cd_line(path),
            b" cd '/home/a b/it'\\''s\\here'\r".to_vec()
        );
        assert_eq!(
            ShellQuoting::Fish.cd_line(path),
            b" cd '/home/a b/it\\'s\\\\here'\r".to_vec()
        );
    }

    #[test]
    fn fish_is_told_apart_by_the_program_name() {
        assert_eq!(
            ShellQuoting::of(Path::new("/usr/bin/fish")),
            ShellQuoting::Fish
        );
        assert_eq!(ShellQuoting::of(Path::new("/bin/zsh")), ShellQuoting::Posix);
    }
}
