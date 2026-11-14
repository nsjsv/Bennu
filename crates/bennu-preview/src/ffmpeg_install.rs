//! 没装 ffmpeg 时给用户一条能直接复制到终端的安装命令。ffmpeg 在各安装包里是可选
//! 依赖，音频、视频预览都靠它。

use std::process::Stdio;
use std::sync::OnceLock;

/// 视频预览打开前先看一眼：没装就直接给安装提示，而不是等解帧失败再报错。
pub async fn ffmpeg_is_installed() -> bool {
    tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// 按 `/etc/os-release` 的 `ID` 与 `ID_LIKE`（派生发行版写上游名）选包管理器命令；
/// 认不出的发行版不给命令，只提示用包管理器装。进程内只读一次：发行版不会在运行中变。
pub fn ffmpeg_install_command() -> Option<&'static str> {
    static COMMAND: OnceLock<Option<&'static str>> = OnceLock::new();
    *COMMAND.get_or_init(|| {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|os_release| install_command_for_os_release(&os_release))
    })
}

fn install_command_for_os_release(os_release: &str) -> Option<&'static str> {
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
        .iter()
        .find_map(|identity| install_command_for_identity(identity))
}

fn install_command_for_identity(identity: &str) -> Option<&'static str> {
    Some(match identity {
        "arch" => "sudo pacman -S ffmpeg",
        "debian" | "ubuntu" => "sudo apt install ffmpeg",
        // Fedora 官方源里的包叫 ffmpeg-free；完整版在第三方源，不替用户选。
        "fedora" => "sudo dnf install ffmpeg-free",
        "suse" | "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" => {
            "sudo zypper install ffmpeg"
        }
        "alpine" => "sudo apk add ffmpeg",
        "void" => "sudo xbps-install ffmpeg",
        "gentoo" => "sudo emerge media-video/ffmpeg",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_distributions_use_their_upstream_package_manager() {
        assert_eq!(
            install_command_for_os_release("NAME=\"Arch Linux\"\nID=arch\n"),
            Some("sudo pacman -S ffmpeg")
        );
        assert_eq!(
            install_command_for_os_release("ID=endeavouros\nID_LIKE=arch\n"),
            Some("sudo pacman -S ffmpeg")
        );
        assert_eq!(
            install_command_for_os_release("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some("sudo apt install ffmpeg")
        );
        assert_eq!(
            install_command_for_os_release(
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"
            ),
            Some("sudo zypper install ffmpeg")
        );
        assert_eq!(
            install_command_for_os_release("ID=fedora\n"),
            Some("sudo dnf install ffmpeg-free")
        );
    }

    #[test]
    fn unknown_distributions_get_no_command() {
        assert_eq!(install_command_for_os_release("ID=nixos\n"), None);
        assert_eq!(install_command_for_os_release(""), None);
    }
}
