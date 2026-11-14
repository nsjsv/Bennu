//! 视频硬解：只在正在驱动屏幕的那块卡上用 VAAPI。
//!
//! 不用 ffmpeg 的默认设备，也不试 CUDA/Vulkan：混合显卡笔记本上它们挑的是第一块卡，常是断电的
//! 独显——每起一次 ffmpeg（打开、跳转）都要把它唤醒（实测 2.4 秒），它的 VAAPI 驱动还可能解不了
//! H.264，最后照样退回 CPU，白白耗电。合成器正在用的卡肯定醒着，建设备只要几十毫秒；某种编码
//! 这块卡解不了时 ffmpeg 自己退回 CPU，代价也只有这几十毫秒。
//!
//! 探测在后台做（打开第一个视频时开始），出帧不等它：还没探完的那一段先用 CPU 解。建设备太慢的
//! （每次起 ffmpeg、每次跳转都要付一遍）也不用。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

/// 探测进程（含建设备）超过这么久就不用硬解：醒着的核显实测约 65 毫秒，唤醒断电独显要好几秒。
const MAX_PROBE_TIME: Duration = Duration::from_millis(300);

static STARTED: Once = Once::new();
static RENDER_NODE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 开始在后台探测（只探一次），不阻塞调用方。
pub(crate) fn start_detection() {
    STARTED.call_once(|| {
        // 起不了线程就不探，一直用 CPU 解。
        let _ = std::thread::Builder::new()
            .name("video-hwaccel-probe".to_owned())
            .spawn(|| {
                let _ = RENDER_NODE.set(detect());
            });
    });
}

/// 探好且能用的 render 节点；还没探完或不能用都是 None（用 CPU 解）。
pub(crate) fn render_node() -> Option<&'static Path> {
    RENDER_NODE.get()?.as_deref()
}

fn detect() -> Option<PathBuf> {
    let node = display_renderer::display_render_node()?;
    let started = Instant::now();
    let opens = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "quiet", "-init_hw_device"])
        .arg(device_argument(&node))
        .args(["-f", "lavfi", "-i", "nullsrc=s=16x16:d=0.04"])
        .args(["-frames:v", "1", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    (opens && started.elapsed() <= MAX_PROBE_TIME).then_some(node)
}

/// `-init_hw_device` 的值：在指定节点上建一个叫 va 的 VAAPI 设备。
pub(crate) fn device_argument(node: &Path) -> String {
    format!("vaapi=va:{}", node.display())
}
