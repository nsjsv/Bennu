use std::io::{self, Read};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn open_pseudo_terminal() -> (OwnedFd, OwnedFd) {
    let mut controller = -1;
    let mut terminal = -1;
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0, "openpty: {}", io::Error::last_os_error());
    unsafe {
        (
            OwnedFd::from_raw_fd(controller),
            OwnedFd::from_raw_fd(terminal),
        )
    }
}

#[test]
fn launch_from_a_terminal_returns_at_once_and_keeps_starting_outside_it() {
    let root = tempfile::tempdir().expect("create isolated launch root");
    let (_controller, terminal) = open_pseudo_terminal();
    let mut command = Command::new(env!("CARGO_BIN_EXE_app-ui"));
    command
        .arg(root.path())
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bennu-test-bus",
        )
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::from(terminal))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // 和在终端里敲命令一样：伪终端是这个进程的控制终端，关终端时它会收到 SIGHUP。
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut launcher = command.spawn().expect("launch app-ui from a terminal");
    let mut stderr = launcher.stderr.take().expect("launcher stderr");

    // 没有会话总线，留在终端里的进程会以失败退出；离开终端后提示符立刻拿回成功。
    assert!(launcher.wait().expect("wait for launcher").success());

    // 管道读到结尾说明离开终端的进程也已退出；它照常往下启动，报错仍写进调用方给的管道。
    let mut detached_output = String::new();
    stderr
        .read_to_string(&mut detached_output)
        .expect("read detached stderr");
    assert!(
        detached_output.contains("desktop activation failed"),
        "{detached_output}"
    );
}
