//! 哪种显卡解码能用：程序里第一次放视频时试一次，之后都用这次的结论。
//!
//! 不用 `-hwaccel auto`：它每起一次 ffmpeg 都把 CUDA、VAAPI、VDPAU、Vulkan 挨个试一遍，
//! 每次跳转都多等几十毫秒。这里只试能不能建出设备；某种编码显卡解不了时 ffmpeg 会自己
//! 退回 CPU 解码，不用我们管。

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// 试的顺序：Intel/AMD 最常见的 VAAPI，NVIDIA 的 CUDA，最后是驱动支持不一的 Vulkan。
const CANDIDATES: [&str; 3] = ["vaapi", "cuda", "vulkan"];

static AVAILABLE: OnceLock<Option<&'static str>> = OnceLock::new();

/// 会阻塞（第一次要起几个 ffmpeg 试设备，约一两百毫秒），只在后台线程里调。
pub(crate) fn available_hwaccel() -> Option<&'static str> {
    *AVAILABLE.get_or_init(|| CANDIDATES.into_iter().find(|api| device_opens(api)))
}

fn device_opens(api: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "quiet", "-init_hw_device"])
        .arg(format!("{api}=probe"))
        .args(["-f", "lavfi", "-i", "nullsrc=s=16x16:d=0.04"])
        .args(["-frames:v", "1", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
