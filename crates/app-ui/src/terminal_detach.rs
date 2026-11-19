use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

/// systemd 服务的 stdout/stderr 也是接在这个流套接字上进日志的。
const JOURNAL_STREAM_SOCKET: &str = "/run/systemd/journal/stdout";
/// 驱动警告、panic 这类原始输出单独一个来源，不混进日志页读取的 `bennu-ui` 结构化日志。
const JOURNAL_STREAM_IDENTIFIER: &str = "bennu";

/// 终端里启动时离开终端会话：关终端（SIGHUP）不再带走窗口，提示符立刻回来。
/// fork 只复制调用线程，必须在任何线程起来之前调用。
pub(crate) fn detach_from_launching_terminal() -> io::Result<()> {
    // 没有控制终端（桌面启动器、D-Bus 拉起、GL 兜底 exec 之后）就没有会挂断它的终端。
    if File::open("/dev/tty").is_err() {
        return Ok(());
    }
    // 保留启动目录；标准流不整体关掉，只换指向终端的那几个，用户重定向到文件/管道的照常写。
    if unsafe { libc::daemon(1, 1) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if io::stdin().is_terminal() {
        rustix::stdio::dup2_stdin(File::open("/dev/null")?)?;
    }
    let stdout_on_terminal = io::stdout().is_terminal();
    let stderr_on_terminal = io::stderr().is_terminal();
    if !stdout_on_terminal && !stderr_on_terminal {
        return Ok(());
    }
    // 终端关掉后写它会报 EIO，eprintln! 会 panic；进日志和桌面启动器拉起时一样能用 journalctl 查。
    let output: OwnedFd = match journal_stream() {
        Ok(stream) => stream.into(),
        Err(_) => File::options().write(true).open("/dev/null")?.into(),
    };
    if stdout_on_terminal {
        rustix::stdio::dup2_stdout(&output)?;
    }
    if stderr_on_terminal {
        rustix::stdio::dup2_stderr(&output)?;
    }
    Ok(())
}

fn journal_stream() -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(JOURNAL_STREAM_SOCKET)?;
    stream.shutdown(Shutdown::Read)?;
    // 流协议头：来源、unit（空）、优先级 info、不解析行首 <N>、不转发 syslog/kmsg/console。
    stream.write_all(format!("{JOURNAL_STREAM_IDENTIFIER}\n\n6\n0\n0\n0\n0\n").as_bytes())?;
    Ok(stream)
}
