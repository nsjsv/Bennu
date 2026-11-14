//! 测试用视频：用本机 ffmpeg 现场生成（ffmpeg 自带的 MPEG-4 编码器，各发行版都有），
//! 没装 ffmpeg 时相关测试直接跳过。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 彩条测试画面，30 帧每秒；`extra` 接在画面输入之后（可再加一路声音输入和编码参数）。
pub(crate) fn video_fixture(
    directory: &Path,
    name: &str,
    size: &str,
    seconds: u32,
    extra: &[&str],
) -> Option<PathBuf> {
    let ffmpeg_available = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !ffmpeg_available {
        return None;
    }
    let path = directory.join(name);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size={size}:rate=30:duration={seconds}"))
        .args(extra)
        .args(["-c:v", "mpeg4", "-g", "15"])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg fixture {name}");
    Some(path)
}
