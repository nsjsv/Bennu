//! 可选的外部命令行工具：音频、视频预览靠 ffmpeg，zip、7z、rar 靠 7-Zip。没装时给用户一条
//! 能直接复制到终端的安装命令。

use std::fmt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalTool {
    Ffmpeg,
    SevenZip,
}

impl ExternalTool {
    /// 各发行版装出来的 7-Zip 命令名不一样（7z、7zz、7za），哪个先找到用哪个。
    fn command_names(self) -> &'static [&'static str] {
        match self {
            Self::Ffmpeg => &["ffmpeg"],
            Self::SevenZip => &["7z", "7zz", "7za"],
        }
    }

    /// 每次都现找：用户可能在软件开着的时候装上，装完不必重启。只是几次 stat。
    pub fn locate(self) -> Option<PathBuf> {
        let search_path = std::env::var_os("PATH")?;
        self.command_names().iter().find_map(|name| {
            std::env::split_paths(&search_path)
                .map(|directory| directory.join(name))
                .find(|candidate| {
                    candidate.metadata().is_ok_and(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    })
                })
        })
    }

    /// 按 `/etc/os-release` 的 `ID` 与 `ID_LIKE`（派生发行版写上游名）选包管理器命令；
    /// 认不出的发行版不给命令，只提示用包管理器装。
    pub fn install_command(self) -> Option<&'static str> {
        distribution_identities()
            .iter()
            .find_map(|identity| install_command_for_identity(self, identity))
    }
}

impl fmt::Display for ExternalTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ffmpeg => "ffmpeg",
            Self::SevenZip => "7-Zip",
        })
    }
}

/// `FileError::ToolMissing` 的后半句：错误会被转成文字一路传到界面，安装命令要跟着走。
pub(crate) fn install_hint(tool: ExternalTool) -> String {
    match tool.install_command() {
        Some(command) => format!("install it with: {command}"),
        None => "install it with your system's package manager".to_owned(),
    }
}

/// 进程内只读一次：发行版不会在运行中变。
fn distribution_identities() -> &'static [String] {
    static IDENTITIES: OnceLock<Vec<String>> = OnceLock::new();
    IDENTITIES.get_or_init(|| {
        std::fs::read_to_string("/etc/os-release")
            .map(|os_release| identities_in_os_release(&os_release))
            .unwrap_or_default()
    })
}

fn identities_in_os_release(os_release: &str) -> Vec<String> {
    let mut identities = Vec::new();
    for line in os_release.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if matches!(key.trim(), "ID" | "ID_LIKE") {
            let value = value
                .trim()
                .trim_matches(|quote| quote == '"' || quote == '\'');
            identities.extend(value.split_whitespace().map(str::to_ascii_lowercase));
        }
    }
    identities
}

fn install_command_for_identity(tool: ExternalTool, identity: &str) -> Option<&'static str> {
    use ExternalTool::{Ffmpeg, SevenZip};
    Some(match (tool, identity) {
        (Ffmpeg, "arch") => "sudo pacman -S ffmpeg",
        (Ffmpeg, "debian" | "ubuntu") => "sudo apt install ffmpeg",
        // Fedora 官方源里的包叫 ffmpeg-free；完整版在第三方源，不替用户选。
        (Ffmpeg, "fedora") => "sudo dnf install ffmpeg-free",
        (Ffmpeg, "suse" | "opensuse" | "opensuse-tumbleweed" | "opensuse-leap") => {
            "sudo zypper install ffmpeg"
        }
        (Ffmpeg, "alpine") => "sudo apk add ffmpeg",
        (Ffmpeg, "void") => "sudo xbps-install ffmpeg",
        (Ffmpeg, "gentoo") => "sudo emerge media-video/ffmpeg",
        (SevenZip, "arch") => "sudo pacman -S 7zip",
        (SevenZip, "debian" | "ubuntu") => "sudo apt install 7zip",
        (SevenZip, "fedora") => "sudo dnf install 7zip",
        (SevenZip, "suse" | "opensuse" | "opensuse-tumbleweed" | "opensuse-leap") => {
            "sudo zypper install 7zip"
        }
        (SevenZip, "alpine") => "sudo apk add 7zip",
        (SevenZip, "void") => "sudo xbps-install 7zip",
        (SevenZip, "gentoo") => "sudo emerge app-arch/7zip",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_for(tool: ExternalTool, os_release: &str) -> Option<&'static str> {
        identities_in_os_release(os_release)
            .iter()
            .find_map(|identity| install_command_for_identity(tool, identity))
    }

    #[test]
    fn derived_distributions_use_their_upstream_package_manager() {
        use ExternalTool::{Ffmpeg, SevenZip};
        assert_eq!(
            command_for(Ffmpeg, "NAME=\"Arch Linux\"\nID=arch\n"),
            Some("sudo pacman -S ffmpeg")
        );
        assert_eq!(
            command_for(SevenZip, "ID=endeavouros\nID_LIKE=arch\n"),
            Some("sudo pacman -S 7zip")
        );
        assert_eq!(
            command_for(Ffmpeg, "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some("sudo apt install ffmpeg")
        );
        assert_eq!(
            command_for(
                SevenZip,
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"
            ),
            Some("sudo zypper install 7zip")
        );
        assert_eq!(
            command_for(Ffmpeg, "ID=fedora\n"),
            Some("sudo dnf install ffmpeg-free")
        );
        assert_eq!(
            command_for(SevenZip, "ID=gentoo\n"),
            Some("sudo emerge app-arch/7zip")
        );
    }

    #[test]
    fn unknown_distributions_get_no_command() {
        assert_eq!(command_for(ExternalTool::Ffmpeg, "ID=nixos\n"), None);
        assert_eq!(command_for(ExternalTool::SevenZip, ""), None);
    }
}
