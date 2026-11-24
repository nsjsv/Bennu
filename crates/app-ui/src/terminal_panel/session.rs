//! 一个终端会话：伪终端里跑的进程，加上它的屏幕（alacritty 的 `Term`）。
//!
//! 输出在读线程里解析进共享的屏幕，界面只收「屏幕变了」的通知，画的时候再去锁屏幕；界面的
//! 输入交给写线程去写。界面线程从不阻塞在伪终端上：程序不读输入、进程迟迟不退，卡住的只是
//! 这两个线程。进程归读线程，读到头由它收；会话丢掉时给整组进程挂断（[`Drop`]）。

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::vte::ansi::Processor;
use file_operations::CommandInvocation;
use portable_pty::{native_pty_system, Child, CommandBuilder, ExitStatus, MasterPty, PtySize};
use tokio::sync::mpsc::UnboundedSender;

use super::emulator::{SharedTerm, TerminalDimensions, TerminalEmulator};

/// 一次会话的稳定标识;事件和标签用它对上号。
pub(crate) type SessionId = u64;

/// 读线程一次最多读这么多就去解析；解析时锁着屏幕，块太大画面要多等。
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// 会话告诉界面的事。所有会话共用一条通道（[`super::SessionEvents`]），带上会话号。
#[derive(Debug, Clone)]
pub(crate) enum SessionEvent {
    /// 屏幕内容变了。
    Wakeup,
    /// shell 在前台时它的当前目录变了（读线程从 /proc 读的）。
    DirectoryChanged(PathBuf),
    /// 要界面回答或转交的终端请求：颜色查询、尺寸查询、写剪贴板。
    Request(Event),
    /// 输出读到头、进程已收：退出状态；读不下去时是原因。
    Ended(Result<ExitStatus, String>),
}

pub(crate) type SessionEventSender = UnboundedSender<(SessionId, SessionEvent)>;

pub(crate) struct TerminalSession {
    pub(crate) id: SessionId,
    pub(crate) emulator: TerminalEmulator,
    input: Arc<PtyInput>,
    master: Box<dyn MasterPty + Send>,
    /// 领头进程的 pid；它开了自己的会话，也是进程组号。读线程收了进程（`Ended`）以后清掉，
    /// 不再往这个号上发信号。
    process: Option<u32>,
    /// 进程输出过东西了（shell 打出了提示符）。
    printed: Arc<AtomicBool>,
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

/// 在新的伪终端里启动 `program`（交互 shell，或超级输入框的一条命令）。参数原样按字节传：
/// 超级输入框把选中的文件当参数，文件名可以不是 UTF-8。
pub(crate) fn spawn_terminal_session(
    id: SessionId,
    program: &CommandInvocation,
    dimensions: TerminalDimensions,
    events: &SessionEventSender,
) -> Result<TerminalSession, String> {
    let not_started = |error: &dyn std::fmt::Display| {
        let name = Path::new(&program.program)
            .file_name()
            .unwrap_or(&program.program);
        tr!("Could not start %1: %2", name.to_string_lossy(), error)
    };
    let pair = native_pty_system()
        .openpty(pty_size(dimensions))
        .map_err(|error| not_started(&error))?;

    let mut command = CommandBuilder::new(&program.program);
    command.args(&program.arguments);
    command.cwd(&program.working_directory);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| not_started(&error))?;
    // slave 句柄在 spawn 后必须释放,否则进程退出后读端等不到头。
    drop(pair.slave);

    let master = pair.master;
    let reader = duplicate_master(master.as_ref()).map_err(|error| not_started(&error))?;
    let failure = Arc::new(OnceLock::new());
    let writer = master.take_writer().map_err(|error| not_started(&error))?;
    let input =
        PtyInput::spawn(id, writer, Arc::clone(&failure)).map_err(|error| not_started(&error))?;
    let emulator = TerminalEmulator::new(
        dimensions,
        EventProxy {
            session: id,
            input: Arc::clone(&input),
            events: events.clone(),
        },
    );
    let process = child.process_id();
    let printed = Arc::new(AtomicBool::new(false));
    let output = OutputReader {
        printed: Arc::clone(&printed),
        session: id,
        reader,
        term: emulator.shared(),
        child,
        failure,
        events: events.clone(),
        directory: program.working_directory.clone(),
    };
    std::thread::Builder::new()
        .name(format!("terminal-read-{id}"))
        .spawn(move || output.run())
        .map_err(|error| not_started(&error))?;
    Ok(TerminalSession {
        id,
        emulator,
        input,
        master,
        process,
        printed,
    })
}

fn pty_size(dimensions: TerminalDimensions) -> PtySize {
    PtySize {
        rows: dimensions.screen_lines as u16,
        cols: dimensions.columns as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// 读线程自己的一份主端：要 poll 它（同步更新超时要按时醒），portable-pty 给的读端是
/// `Box<dyn Read>`，拿不到 fd。
fn duplicate_master(master: &dyn MasterPty) -> std::io::Result<File> {
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("the pseudo terminal has no file descriptor"))?;
    // SAFETY: fd 归 `master`，这里只借来复制一份，复制出的 fd 自己持有。
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    Ok(File::from(rustix::io::dup(borrowed)?))
}

impl TerminalSession {
    /// 原样写进去：跟随目录注入的 cd、超级输入框打进 shell 的命令。
    pub(crate) fn write_input(&self, bytes: &[u8]) {
        self.input.write(bytes);
    }

    /// 用户打的字：回看先滚回底部，跟别的终端一样。
    pub(crate) fn type_input(&self, bytes: &[u8]) {
        self.emulator.scroll_to_bottom();
        self.input.write(bytes);
    }

    /// 粘贴：程序开了 bracketed paste 就包起来，多行文本不会被当成几条命令直接执行。
    pub(crate) fn paste(&self, text: &str) {
        self.type_input(&super::input::paste_bytes(text, self.emulator.mode()));
    }

    /// 断开的原因;有值时界面显示"已断开"，之后的输入直接丢掉。
    pub(crate) fn disconnected(&self) -> Option<&str> {
        self.input.failure.get().map(String::as_str)
    }

    pub(crate) fn resize(&self, dimensions: TerminalDimensions) {
        if dimensions.columns == 0 || dimensions.screen_lines == 0 {
            return;
        }
        self.emulator.resize(dimensions);
        // 尺寸没同步过去只影响换行排版,会话照常可用。
        if let Err(error) = self.master.resize(pty_size(dimensions)) {
            tracing::warn!(
                target: "app_ui::terminal",
                event = "terminal_resize_failed",
                %error,
                "terminal size could not be sent to the shell"
            );
        }
    }

    /// 给会话里的整组进程发信号。进程收掉以后（[`Self::process_ended`]）什么都不发；读线程
    /// 收进程到界面处理 `Ended` 之间那一下 pid 被别的进程拿去的可能忽略。
    pub(crate) fn signal(&self, signal: libc::c_int) {
        if let Some(process) = self.process {
            // 组里已经没有进程（ESRCH）不算错：刚好都结束了。
            unsafe {
                libc::kill(-(process as libc::pid_t), signal);
            }
        }
    }

    /// shell 起来了、在等输入：输出过东西（还在读 rc 文件时一个 SIGINT 会打断它的配置），
    /// 而且它自己在前台，没有程序在跑。
    pub(crate) fn shell_is_idle(&self) -> bool {
        self.printed.load(Ordering::Relaxed)
            && self.process.is_some_and(|process| {
                self.master.process_group_leader() == Some(process as libc::pid_t)
            })
    }

    /// 测试里造一个断开的会话（真的读写失败不好碰上）。
    #[cfg(test)]
    pub(crate) fn disconnect_for_test(&self, reason: &str) {
        let _ = self.input.failure.set(reason.to_owned());
    }

    /// 读线程已经收了进程（界面收到 `Ended`）。
    pub(crate) fn process_ended(&mut self) {
        self.process = None;
    }
}

impl Drop for TerminalSession {
    /// 关掉这个终端跟关掉终端窗口一样：整组挂断。shell 存好历史、转告它的作业再退出，读线程
    /// 读到头自己收进程。读线程手里还有一份主端，光丢掉主端内核不会挂断，所以要自己发。
    fn drop(&mut self) {
        self.signal(libc::SIGHUP);
    }
}

/// 往伪终端写的唯一入口：用户输入、粘贴、终端应答（`PtyWrite`）都经它交给写线程。
/// 第一次写不进去（读线程读不下去也算）就记下原因、记一条日志，此后的写入直接丢弃：会话
/// 已经断了，界面据此显示"已断开"。
pub(crate) struct PtyInput {
    sender: mpsc::Sender<Vec<u8>>,
    failure: Arc<OnceLock<String>>,
}

impl PtyInput {
    fn spawn(
        id: SessionId,
        mut writer: Box<dyn Write + Send>,
        failure: Arc<OnceLock<String>>,
    ) -> std::io::Result<Arc<Self>> {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let thread_failure = Arc::clone(&failure);
        // 发送端都丢掉（会话和它的屏幕都不在了）时通道关上，线程随之退出。
        std::thread::Builder::new()
            .name(format!("terminal-write-{id}"))
            .spawn(move || {
                for bytes in receiver {
                    if thread_failure.get().is_some() {
                        continue;
                    }
                    if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                        let reason = error.to_string();
                        if thread_failure.set(reason.clone()).is_ok() {
                            tracing::error!(
                                target: "app_ui::terminal",
                                event = "terminal_write_failed",
                                error = %reason,
                                "terminal input could not be written; the session is disconnected"
                            );
                        }
                    }
                }
            })?;
        Ok(Arc::new(Self { sender, failure }))
    }

    fn write(&self, bytes: &[u8]) {
        if self.failure.get().is_none() {
            // 写线程活得比所有发送端久，发不出去不会发生。
            let _ = self.sender.send(bytes.to_vec());
        }
    }
}

/// 屏幕解析时发出的事（在读线程里）：应答直接写回伪终端，要界面的转给界面，其余不要。
pub(crate) struct EventProxy {
    session: SessionId,
    input: Arc<PtyInput>,
    events: SessionEventSender,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.input.write(text.as_bytes()),
            Event::ColorRequest(..) | Event::TextAreaSizeRequest(_) | Event::ClipboardStore(..) => {
                // 界面不在了（应用在退出）就不用回答。
                let _ = self
                    .events
                    .send((self.session, SessionEvent::Request(event)));
            }
            // 标题不用：标签显示 shell 所在目录名，bash/fish 默认设的 `user@host:路径` 标签里
            // 放不下。铃声不用：后台标签有输出已经亮未读圆点。屏幕变化由读线程统一通知。
            _ => {}
        }
    }
}

#[cfg(test)]
impl EventProxy {
    /// 不接伪终端、没人听的出口：测试直接往屏幕里喂字节时用。
    pub(crate) fn detached() -> Self {
        Self {
            session: 0,
            input: PtyInput::spawn(0, Box::new(std::io::sink()), Arc::new(OnceLock::new()))
                .expect("spawn the writer thread"),
            events: tokio::sync::mpsc::unbounded_channel().0,
        }
    }
}

/// 读线程：读输出、解析进屏幕、告诉界面；读到头收进程。
struct OutputReader {
    printed: Arc<AtomicBool>,
    session: SessionId,
    reader: File,
    term: SharedTerm,
    child: Box<dyn Child + Send + Sync>,
    failure: Arc<OnceLock<String>>,
    events: SessionEventSender,
    /// shell 上次报告的当前目录。
    directory: PathBuf,
}

impl OutputReader {
    fn run(mut self) {
        match self.read_to_end() {
            Ok(()) => {
                let end = self.child.wait().map_err(|error| error.to_string());
                self.send(SessionEvent::Ended(end));
            }
            Err(error) => {
                let reason = error.to_string();
                if self.failure.set(reason.clone()).is_ok() {
                    tracing::error!(
                        target: "app_ui::terminal",
                        event = "terminal_read_failed",
                        error = %reason,
                        "terminal output could not be read; the session is disconnected"
                    );
                }
                self.send(SessionEvent::Ended(Err(reason)));
                // 读不下去时进程可能还在跑：照样等它退出再收，不留僵尸。
                let _ = self.child.wait();
            }
        }
    }

    fn read_to_end(&mut self) -> std::io::Result<()> {
        let mut processor: Processor = Processor::new();
        let mut buffer = vec![0; READ_CHUNK_BYTES];
        loop {
            if !wait_readable(&self.reader, processor.sync_timeout().sync_timeout())? {
                // 程序开了同步更新（DECSET 2026）迟迟不收尾：超时了照样把攒着的画出来。
                processor.stop_sync(&mut *self.term.lock());
                self.send(SessionEvent::Wakeup);
                continue;
            }
            let length = match self.reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(length) => length,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                // 另一端都关了（进程都退出了）：Linux 上读主端是 EIO，就是读到头。
                Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(()),
                Err(error) => return Err(error),
            };
            processor.advance(&mut *self.term.lock(), &buffer[..length]);
            self.printed.store(true, Ordering::Relaxed);
            // 读到的全被同步更新攒着时屏幕没变，等它收尾或超时再说。
            if processor.sync_bytes_count() < length {
                self.send(SessionEvent::Wakeup);
            }
            self.report_directory();
        }
    }

    /// shell 自己在前台（没有程序在跑）时看它的当前目录，变了就告诉界面。每读一块看一次：
    /// 在 shell 里 cd 以后一定会输出提示符；程序在前台时只多一次 ioctl。
    fn report_directory(&mut self) {
        let Some(process) = self.child.process_id() else {
            return;
        };
        // SAFETY: 只读这个 fd 所在终端的前台进程组。
        let foreground = unsafe { libc::tcgetpgrp(self.reader.as_raw_fd()) };
        if foreground != process as libc::pid_t {
            return;
        }
        let Ok(directory) = std::fs::read_link(format!("/proc/{process}/cwd")) else {
            return;
        };
        if directory != self.directory {
            self.directory = directory.clone();
            self.send(SessionEvent::DirectoryChanged(directory));
        }
    }

    fn send(&self, event: SessionEvent) {
        // 界面不在了（应用在退出）就没人要听。
        let _ = self.events.send((self.session, event));
    }
}

/// 等到主端可读；`deadline` 到了还没有就返回 `false`。
fn wait_readable(reader: &File, deadline: Option<Instant>) -> std::io::Result<bool> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    loop {
        let timeout = deadline.map(|deadline| {
            let left = deadline.saturating_duration_since(Instant::now());
            Timespec {
                tv_sec: left.as_secs() as _,
                tv_nsec: left.subsec_nanos() as _,
            }
        });
        let mut fds = [PollFd::new(reader, PollFlags::IN)];
        match poll(&mut fds, timeout.as_ref()) {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::time::Duration;

    use super::*;

    type Events = tokio::sync::mpsc::UnboundedReceiver<(SessionId, SessionEvent)>;

    fn spawn(program: &str, arguments: &[&str], folder: &Path) -> (TerminalSession, Events) {
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let session = spawn_terminal_session(
            1,
            &CommandInvocation {
                program: program.into(),
                arguments: arguments.iter().map(Into::into).collect(),
                working_directory: folder.to_path_buf(),
            },
            TerminalDimensions {
                columns: 80,
                screen_lines: 24,
            },
            &sender,
        )
        .expect("spawn");
        (session, events)
    }

    /// 等到 `wanted` 认出一个事件为止，返回它；中间的跳过。
    fn wait_for<T>(events: &mut Events, wanted: impl Fn(&SessionEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match events.try_recv() {
                Ok((_, event)) => {
                    if let Some(found) = wanted(&event) {
                        return found;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("the terminal session never reported what the test waits for");
    }

    fn ended(events: &mut Events) -> Result<ExitStatus, String> {
        wait_for(events, |event| match event {
            SessionEvent::Ended(end) => Some(end.clone()),
            _ => None,
        })
    }

    #[test]
    fn output_is_parsed_into_the_screen_off_the_ui_thread() {
        let folder = tempfile::tempdir().unwrap();
        let (session, mut events) =
            spawn("/bin/sh", &["-c", "printf hello; read x"], folder.path());

        wait_for(&mut events, |event| {
            (matches!(event, SessionEvent::Wakeup) && session.emulator.last_lines(1) == "hello")
                .then_some(())
        });
        session.write_input(b"\r");
        assert!(ended(&mut events).unwrap().success());
    }

    #[test]
    fn the_end_brings_the_exit_status() {
        let folder = tempfile::tempdir().unwrap();
        let (_session, mut events) = spawn("/bin/sh", &["-c", "exit 3"], folder.path());

        assert_eq!(ended(&mut events).unwrap().exit_code(), 3);
    }

    #[test]
    fn a_directory_change_of_the_waiting_shell_is_reported() {
        let folder = tempfile::tempdir().unwrap();
        let inner = folder.path().canonicalize().unwrap().join("inner");
        std::fs::create_dir(&inner).unwrap();
        let (session, mut events) = spawn(
            "/bin/sh",
            &["-c", "cd inner && printf x && read x"],
            folder.path(),
        );

        let reported = wait_for(&mut events, |event| match event {
            SessionEvent::DirectoryChanged(directory) => Some(directory.clone()),
            _ => None,
        });
        assert_eq!(reported, inner);
        session.write_input(b"\r");
        ended(&mut events).unwrap();
    }

    #[test]
    fn arguments_reach_the_program_byte_for_byte() {
        let folder = tempfile::tempdir().unwrap();
        let name = std::ffi::OsString::from_vec(vec![0xb2, 0xe2, b'.', b't', b'x', b't']);
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let _session = spawn_terminal_session(
            1,
            &CommandInvocation {
                program: "/bin/sh".into(),
                arguments: vec![
                    "-c".into(),
                    "printf %s \"$1\" > given".into(),
                    "bennu".into(),
                    name.clone(),
                ],
                working_directory: folder.path().to_path_buf(),
            },
            TerminalDimensions {
                columns: 80,
                screen_lines: 24,
            },
            &sender,
        )
        .expect("spawn");

        assert!(ended(&mut events).unwrap().success());
        assert_eq!(
            std::fs::read(folder.path().join("given")).unwrap(),
            name.into_vec()
        );
    }

    #[test]
    fn a_shell_running_a_foreground_job_is_not_idle() {
        let folder = tempfile::tempdir().unwrap();
        let (waiting, _events) = spawn("/bin/sh", &["-c", "printf '$ '; read x"], folder.path());
        // `set -m`：作业控制打开，前台作业有自己的进程组，跟交互 shell 一样。
        let (busy, _busy_events) = spawn(
            "/bin/sh",
            &["-c", "set -m; printf '$ '; sleep 30"],
            folder.path(),
        );
        // 刚起来、什么都还没打出来的 shell 不算在等输入。
        assert!(!waiting.shell_is_idle());

        let deadline = Instant::now() + Duration::from_secs(20);
        while !waiting.shell_is_idle() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(waiting.shell_is_idle());
        assert!(!busy.shell_is_idle());
    }

    /// 写端已断（如 shell 那头关了伪终端）：每次写都失败，并记下被调用了几次。
    struct BrokenPty(Arc<std::sync::atomic::AtomicUsize>);

    impl Write for BrokenPty {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_disconnects_the_session_and_drops_later_input() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let input = PtyInput::spawn(
            1,
            Box::new(BrokenPty(Arc::clone(&attempts))),
            Arc::new(OnceLock::new()),
        )
        .unwrap();

        input.write(b"ls\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while input.failure.get().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        input.write(b"pwd\r");
        std::thread::sleep(Duration::from_millis(50));

        assert!(input.failure.get().is_some());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

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
