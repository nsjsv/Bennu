use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use thiserror::Error;
use tokio::process::Command;

use crate::desktop_entries::{desktop_entry_requires_terminal, read_desktop_entry_text};

const XDG_OPEN: &str = "xdg-open";
const XDG_MIME: &str = "xdg-mime";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEmulator {
    Automatic,
    XdgTerminalExec,
    Foot,
    Alacritty,
    Kitty,
    WezTerm,
    Ghostty,
    GnomeTerminal,
    Konsole,
    Xfce4Terminal,
    Xterm,
}

pub const TERMINAL_EMULATOR_OPTIONS: &[TerminalEmulator] = &[
    TerminalEmulator::Automatic,
    TerminalEmulator::Kitty,
    TerminalEmulator::Ghostty,
    TerminalEmulator::Alacritty,
    TerminalEmulator::Foot,
    TerminalEmulator::WezTerm,
    TerminalEmulator::GnomeTerminal,
    TerminalEmulator::Konsole,
    TerminalEmulator::Xfce4Terminal,
    TerminalEmulator::Xterm,
    TerminalEmulator::XdgTerminalExec,
];

impl TerminalEmulator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Auto",
            Self::XdgTerminalExec => "xdg-terminal-exec",
            Self::Foot => "Foot",
            Self::Alacritty => "Alacritty",
            Self::Kitty => "Kitty",
            Self::WezTerm => "WezTerm",
            Self::Ghostty => "Ghostty",
            Self::GnomeTerminal => "GNOME Terminal",
            Self::Konsole => "Konsole",
            Self::Xfce4Terminal => "XFCE Terminal",
            Self::Xterm => "Xterm",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Automatic => "auto",
            Self::XdgTerminalExec => "xdg-terminal-exec",
            Self::Foot => "foot",
            Self::Alacritty => "alacritty",
            Self::Kitty => "kitty",
            Self::WezTerm => "wezterm",
            Self::Ghostty => "ghostty",
            Self::GnomeTerminal => "gnome-terminal",
            Self::Konsole => "konsole",
            Self::Xfce4Terminal => "xfce4-terminal",
            Self::Xterm => "xterm",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Automatic),
            "xdg-terminal-exec" => Some(Self::XdgTerminalExec),
            "foot" => Some(Self::Foot),
            "alacritty" => Some(Self::Alacritty),
            "kitty" => Some(Self::Kitty),
            "wezterm" => Some(Self::WezTerm),
            "ghostty" => Some(Self::Ghostty),
            "gnome-terminal" => Some(Self::GnomeTerminal),
            "konsole" => Some(Self::Konsole),
            "xfce4-terminal" => Some(Self::Xfce4Terminal),
            "xterm" => Some(Self::Xterm),
            _ => None,
        }
    }
}

const TERMINAL_LAUNCHERS: &[TerminalLauncher] = &[
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::XdgTerminalExec,
        command: "xdg-terminal-exec",
        before_open_command: &[],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Foot,
        command: "foot",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Alacritty,
        command: "alacritty",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Kitty,
        command: "kitty",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::WezTerm,
        command: "wezterm",
        before_open_command: &["start", "--"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Ghostty,
        command: "ghostty",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::GnomeTerminal,
        command: "gnome-terminal",
        before_open_command: &["--"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Konsole,
        command: "konsole",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Xfce4Terminal,
        command: "xfce4-terminal",
        before_open_command: &["-e"],
    },
    TerminalLauncher {
        terminal_emulator: TerminalEmulator::Xterm,
        command: "xterm",
        before_open_command: &["-e"],
    },
];

struct TerminalLauncher {
    terminal_emulator: TerminalEmulator,
    command: &'static str,
    before_open_command: &'static [&'static str],
}

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("could not start xdg-open for {path:?}: {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("xdg-open failed for {path:?} with status {status}")]
    Failed { path: PathBuf, status: ExitStatus },
    #[error("could not start terminal opener for {path:?}: {source}")]
    TerminalSpawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "could not find {terminal_emulator:?} terminal emulator to open {path:?} with {desktop_id}"
    )]
    TerminalUnavailable {
        path: PathBuf,
        desktop_id: String,
        terminal_emulator: TerminalEmulator,
    },
    #[error("could not find {terminal_emulator:?} terminal emulator to open terminal at {path:?}")]
    TerminalDirectoryUnavailable {
        path: PathBuf,
        terminal_emulator: TerminalEmulator,
    },
}

/// URL 不是文件系统路径，单独建错误概念，避免把链接伪装成 Path 复用 OpenError。
#[derive(Debug, Error)]
pub enum UrlOpenError {
    #[error("could not start xdg-open for {url:?}: {source}")]
    Spawn {
        url: String,
        #[source]
        source: std::io::Error,
    },
    #[error("xdg-open failed for {url:?} with status {status}")]
    Failed { url: String, status: ExitStatus },
}

pub async fn open_url(url: &str) -> Result<(), UrlOpenError> {
    let status = Command::new(XDG_OPEN)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|source| UrlOpenError::Spawn {
            url: url.to_owned(),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(UrlOpenError::Failed {
            url: url.to_owned(),
            status,
        })
    }
}

pub async fn open_path(path: impl AsRef<Path>) -> Result<(), OpenError> {
    open_path_with_terminal_emulator(path, TerminalEmulator::Automatic).await
}

pub async fn open_path_with_terminal_emulator(
    path: impl AsRef<Path>,
    terminal_emulator: TerminalEmulator,
) -> Result<(), OpenError> {
    let path = path.as_ref().to_path_buf();
    if let Some(desktop_id) = terminal_default_desktop_id(&path).await {
        return open_terminal_default_application(&path, desktop_id, terminal_emulator).await;
    }

    open_path_with_opener(path.as_ref(), OsStr::new(XDG_OPEN)).await
}

pub async fn open_terminal_at_directory(
    directory: impl AsRef<Path>,
    terminal_emulator: TerminalEmulator,
) -> Result<(), OpenError> {
    let directory = directory.as_ref().to_path_buf();
    let launcher = terminal_launcher(terminal_emulator).ok_or_else(|| {
        OpenError::TerminalDirectoryUnavailable {
            path: directory.clone(),
            terminal_emulator,
        }
    })?;
    let mut command = terminal_directory_command(launcher, &directory);
    command.spawn().map_err(|source| OpenError::TerminalSpawn {
        path: directory,
        source,
    })?;
    Ok(())
}

async fn terminal_default_desktop_id(path: &Path) -> Option<String> {
    let mime_type = query_file_mime_type(path).await?;
    let desktop_id = query_default_desktop_id(&mime_type).await?;
    let desktop_entry = read_desktop_entry_text(&desktop_id).await?;
    desktop_entry_requires_terminal(&desktop_entry).then_some(desktop_id)
}

async fn query_file_mime_type(path: &Path) -> Option<String> {
    let output = Command::new(XDG_MIME)
        .arg("query")
        .arg("filetype")
        .arg(path.as_os_str())
        .output()
        .await
        .ok()?;
    successful_command_output(output)
}

async fn query_default_desktop_id(mime_type: &str) -> Option<String> {
    let output = Command::new(XDG_MIME)
        .arg("query")
        .arg("default")
        .arg(mime_type)
        .output()
        .await
        .ok()?;
    successful_command_output(output)
}

fn successful_command_output(output: std::process::Output) -> Option<String> {
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

async fn open_terminal_default_application(
    path: &Path,
    desktop_id: String,
    terminal_emulator: TerminalEmulator,
) -> Result<(), OpenError> {
    let launcher =
        terminal_launcher(terminal_emulator).ok_or_else(|| OpenError::TerminalUnavailable {
            path: path.to_path_buf(),
            desktop_id,
            terminal_emulator,
        })?;
    let mut command = terminal_open_command(launcher, path);
    command.spawn().map_err(|source| OpenError::TerminalSpawn {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn terminal_launcher(terminal_emulator: TerminalEmulator) -> Option<&'static TerminalLauncher> {
    match terminal_emulator {
        TerminalEmulator::Automatic => TERMINAL_LAUNCHERS
            .iter()
            .find(|launcher| command_exists(launcher.command)),
        selected => {
            terminal_launcher_for(selected).filter(|launcher| command_exists(launcher.command))
        }
    }
}

fn terminal_launcher_for(terminal_emulator: TerminalEmulator) -> Option<&'static TerminalLauncher> {
    TERMINAL_LAUNCHERS
        .iter()
        .find(|launcher| launcher.terminal_emulator == terminal_emulator)
}

fn terminal_open_command(launcher: &TerminalLauncher, path: &Path) -> Command {
    let mut command = Command::new(launcher.command);
    command
        .args(launcher.before_open_command)
        // Terminal=true 的默认程序（例如 vim.desktop）需要真实终端承载；
        // 在终端里继续交给 xdg-open，保留系统 MIME 关联选择。
        .arg(XDG_OPEN)
        .arg(path.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn terminal_directory_command(launcher: &TerminalLauncher, directory: &Path) -> Command {
    let mut command = Command::new(launcher.command);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn command_exists(command_name: &str) -> bool {
    let Some(search_path) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&search_path).any(|directory| directory.join(command_name).is_file())
}

async fn open_path_with_opener(path: &Path, opener: &OsStr) -> Result<(), OpenError> {
    let path = path.to_path_buf();
    let status = open_command(opener, &path)
        .status()
        .await
        .map_err(|source| OpenError::Spawn {
            path: path.clone(),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(OpenError::Failed { path, status })
    }
}

fn open_command(opener: &OsStr, path: &Path) -> Command {
    let mut command = Command::new(opener);
    command
        .arg(path.as_os_str())
        // xdg-open 可能回退到 vim/nvim 这类终端应用，不能继承启动器所在终端。
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn open_path_disconnects_stdio_from_launcher_terminal() {
        let dir = tempdir().unwrap();
        let opener = dir.path().join("fake-open");
        let report = dir.path().join("stdio-report");

        fs::write(
            &opener,
            r#"#!/bin/sh
exec 3>"$1"
readlink "/proc/$$/fd/0" >&3
readlink "/proc/$$/fd/1" >&3
readlink "/proc/$$/fd/2" >&3
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&opener).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&opener, permissions).unwrap();

        open_path_with_opener(&report, opener.as_os_str())
            .await
            .unwrap();

        let targets = fs::read_to_string(report).unwrap();
        assert_eq!(targets.lines().collect::<Vec<_>>(), ["/dev/null"; 3]);
    }

    #[test]
    fn terminal_emulator_config_values_round_trip() {
        for terminal_emulator in TERMINAL_EMULATOR_OPTIONS {
            assert_eq!(
                TerminalEmulator::from_config_value(terminal_emulator.config_value()),
                Some(*terminal_emulator)
            );
        }
        assert_eq!(
            TerminalEmulator::from_config_value("missing-terminal"),
            None
        );
    }

    #[test]
    fn terminal_open_command_uses_selected_launcher() {
        let launcher = terminal_launcher_for(TerminalEmulator::Ghostty).unwrap();
        let command = terminal_open_command(launcher, Path::new("/tmp/example"));
        let command = command.as_std();
        let arguments = command.get_args().map(OsString::from).collect::<Vec<_>>();

        assert_eq!(command.get_program(), OsStr::new("ghostty"));
        assert_eq!(
            arguments,
            [
                OsString::from("-e"),
                OsString::from(XDG_OPEN),
                OsString::from("/tmp/example"),
            ]
        );
    }

    #[test]
    fn terminal_directory_command_sets_working_directory() {
        let launcher = terminal_launcher_for(TerminalEmulator::Kitty).unwrap();
        let directory = Path::new("/tmp/example");
        let command = terminal_directory_command(launcher, directory);
        let command = command.as_std();

        assert_eq!(command.get_program(), OsStr::new("kitty"));
        assert_eq!(command.get_current_dir(), Some(directory));
        assert!(command.get_args().next().is_none());
    }

    #[test]
    fn desktop_entry_requires_terminal_reads_desktop_entry_group() {
        let desktop_entry = r#"
[Desktop Entry]
Name=Vim
Terminal=true
Exec=vim %F

[Desktop Action New]
Terminal=false
"#;

        assert!(desktop_entry_requires_terminal(desktop_entry));
    }

    #[test]
    fn desktop_entry_requires_terminal_ignores_other_groups() {
        let desktop_entry = r#"
[Desktop Action New]
Terminal=true

[Desktop Entry]
Name=Graphical Editor
Terminal=false
Exec=editor %F
"#;

        assert!(!desktop_entry_requires_terminal(desktop_entry));
    }
}
