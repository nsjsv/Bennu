use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use portable_pty::native_pty_system;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

use iced::futures::SinkExt;

use super::emulator::{PtyWriter, TerminalDimensions, TerminalEmulator};
use super::TerminalPanelMessage;

/// 一次会话的稳定标识;subscription 与消息用它匹配输出归属。
pub(crate) type SessionId = u64;

pub(crate) struct TerminalSession {
    pub(crate) id: SessionId,
    pub(crate) emulator: TerminalEmulator,
    writer: PtyWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: bool,
}

pub(crate) fn shell_for_user() -> String {
    // 桌面会话里的 $SHELL 不一定跟随用户改过的登录 shell(比如应用从 bash 里启动时
    // $SHELL 仍是 bash);用户默认 shell 以 passwd 登录项为准,读不到再退 $SHELL。
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    if !passwd.is_null() {
        let shell = unsafe { std::ffi::CStr::from_ptr((*passwd).pw_shell) };
        if let Ok(shell) = shell.to_str() {
            if !shell.is_empty() {
                return shell.to_owned();
            }
        }
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned())
}

/// 值得作为交互终端展示的 shell 名称;/etc/shells 里还有 nologin、git-shell、
/// rbash 等特殊条目,对终端下拉纯属噪音,直接不收。
const INTERACTIVE_SHELL_NAMES: &[&str] = &[
    "zsh", "fish", "bash", "sh", "dash", "ksh", "ksh93", "mksh", "csh", "tcsh", "nu", "nushell",
    "elvish", "pwsh", "xonsh",
];
/// 直接探测的安装目录;新装的 shell(如 zsh)未必登记进 /etc/shells,照样出现。
const SHELL_SEARCH_DIRECTORIES: &[&str] = &["/usr/bin", "/bin", "/usr/local/bin"];

/// 可用的交互 shell,按真实路径(canonicalize)去重,进程内缓存一次。
pub(crate) fn available_shells() -> &'static [String] {
    static SHELLS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    SHELLS.get_or_init(scan_interactive_shells)
}

fn scan_interactive_shells() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(content) = std::fs::read_to_string("/etc/shells") {
        for line in content.lines() {
            let path = line.trim();
            if !path.is_empty() && !path.starts_with('#') {
                candidates.push(path.to_owned());
            }
        }
    }
    for directory in SHELL_SEARCH_DIRECTORIES {
        for name in INTERACTIVE_SHELL_NAMES {
            candidates.push(format!("{directory}/{name}"));
        }
    }

    let mut shells: Vec<String> = Vec::new();
    let mut resolved_paths: Vec<std::path::PathBuf> = Vec::new();
    for name in INTERACTIVE_SHELL_NAMES {
        for candidate in &candidates {
            let path = Path::new(candidate);
            if path.file_name().and_then(|file_name| file_name.to_str()) != Some(name) {
                continue;
            }
            if !path.exists() {
                continue;
            }
            let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if resolved_paths.contains(&resolved) {
                continue;
            }
            resolved_paths.push(resolved);
            shells.push(candidate.clone());
        }
    }
    shells
}

pub(crate) fn spawn_terminal_session(
    id: SessionId,
    shell: &str,
    cwd: &Path,
    dimensions: TerminalDimensions,
) -> Result<TerminalSession, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: dimensions.screen_lines as u16,
            cols: dimensions.columns as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("打开 PTY 失败: {error}"))?;

    let mut command = CommandBuilder::new(shell);
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("启动 shell 失败: {error}"))?;
    // slave 句柄在 spawn 后必须释放,否则终端收到 EOF 前会一直等到父进程退出。
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("获取 PTY 写端失败: {error}"))?;
    let writer: PtyWriter = Arc::new(Mutex::new(writer));
    let emulator =
        TerminalEmulator::new(dimensions.columns, dimensions.screen_lines, writer.clone());
    Ok(TerminalSession {
        id,
        emulator,
        writer,
        master: pair.master,
        child,
        exited: false,
    })
}

impl TerminalSession {
    pub(crate) fn write_input(&self, bytes: &[u8]) {
        let Ok(mut writer) = self.writer.lock() else {
            return;
        };
        if writer.write_all(bytes).is_ok() {
            let _ = writer.flush();
        }
    }

    pub(crate) fn resize(&mut self, dimensions: TerminalDimensions) {
        if dimensions.columns == 0 || dimensions.screen_lines == 0 {
            return;
        }
        self.emulator
            .resize(dimensions.columns, dimensions.screen_lines);
        let _ = self.master.resize(PtySize {
            rows: dimensions.screen_lines as u16,
            cols: dimensions.columns as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// 订阅输出时克隆读端;dup 出的 fd 与原读端共享内核缓冲,重订阅不丢数据。
    pub(crate) fn clone_reader(&self) -> Result<Box<dyn Read + Send>, String> {
        self.master
            .try_clone_reader()
            .map_err(|error| format!("克隆 PTY 读端失败: {error}"))
    }

    /// 彻底关闭:杀掉 shell(若还活着)并回收子进程避免留僵尸;
    /// shell 已自行退出时 kill 失败由 wait 兜底回收。主从端随后释放,
    /// 读端线程因 EOF 退出。
    pub(crate) fn terminate(&mut self) {
        self.exited = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 读取 PTY 输出的后台线程桥接到 iced stream。
pub(crate) fn terminal_output_stream(
    session_id: SessionId,
    mut reader: Box<dyn Read + Send>,
) -> impl iced::futures::Stream<Item = TerminalPanelMessage> + 'static {
    iced::stream::channel(16, async move |mut output| {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<TerminalPanelMessage>(16);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.blocking_send(TerminalPanelMessage::ProcessExited {
                            session: session_id,
                        });
                        break;
                    }
                    Ok(length) => {
                        let message = TerminalPanelMessage::OutputReceived {
                            session: session_id,
                            bytes: buffer[..length].to_vec(),
                        };
                        if sender.blocking_send(message).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        while let Some(message) = receiver.recv().await {
            let _ = output.send(message).await;
        }

        iced::futures::future::pending().await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanned_shells_are_deduplicated_and_interactive_only() {
        let shells = scan_interactive_shells();
        assert!(shells.iter().any(|shell| shell.ends_with("/bash")));

        let mut basenames: Vec<&str> = shells
            .iter()
            .map(|shell| {
                Path::new(shell)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("shell path has a file name")
            })
            .collect();
        basenames.sort_unstable();
        let duplicate_count = basenames.len()
            - basenames
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len();
        assert_eq!(duplicate_count, 0, "duplicate shell entries: {basenames:?}");

        for basename in &basenames {
            assert!(
                INTERACTIVE_SHELL_NAMES.contains(basename),
                "non-interactive shell leaked into the list: {basename}"
            );
        }
    }
}
