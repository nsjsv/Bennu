//! 测试用音频：用本机 ffmpeg 现场生成，没装 ffmpeg 时相关测试直接跳过
//! （同依赖 poppler 的 PDF 测试）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 440 Hz 正弦波，格式按文件后缀。
pub(crate) fn sine_fixture(directory: &Path, name: &str, seconds: u32) -> Option<PathBuf> {
    encoded_sine_fixture(directory, name, seconds, &[])
}

/// 同上，再加编码参数（如做一个没有 Xing 头的 VBR MP3）。
pub(crate) fn encoded_sine_fixture(
    directory: &Path,
    name: &str,
    seconds: u32,
    encoder_args: &[&str],
) -> Option<PathBuf> {
    if !ffmpeg_available() {
        return None;
    }
    let path = directory.join(name);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds}"))
        .args(encoder_args)
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg fixture {name}");
    Some(path)
}
