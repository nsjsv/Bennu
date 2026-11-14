//! 视频硬解跟着正在画屏幕的那块卡走：窗口本来就在这块卡上画（启动时把渲染钉在它上面），
//! 解码也在它上面，按它的驱动选接口——Mesa 系（Intel、AMD……）用 VAAPI，NVIDIA 专有驱动用
//! NVDEC（CUDA）。
//!
//! 不用 ffmpeg 的默认设备：多显卡机器上默认是第一块卡，常是断电的独显——每起一次 ffmpeg
//! （打开、跳转）都要把它唤醒（实测 2.4 秒），它还可能解不了这种编码、最后照样退回 CPU，白白
//! 耗电。正在画屏幕的卡肯定醒着，建设备只要几十毫秒；某种编码这块卡解不了时 ffmpeg 自己退回
//! CPU，代价也只有这几十毫秒。
//!
//! 探测在后台做（打开第一个视频时开始），出帧不等它：还没探完的那一段先用 CPU 解。建设备太慢的
//! （每次起 ffmpeg、每次跳转都要付一遍）也不用。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

/// 探测进程（含建设备）超过这么久就不用硬解。醒着的卡起进程加建设备几十毫秒，唤醒断电的卡要
/// 好几秒，两者差得很远，这个上限只为挡住后一种。
const MAX_PROBE_TIME: Duration = Duration::from_millis(300);
/// NVIDIA 专有驱动在 sysfs 里的驱动名。
const NVIDIA_DRIVER: &str = "nvidia";
const NVIDIA_VENDOR: &str = "0x10de";

/// 用哪种硬解、在哪块卡上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HardwareDecoder {
    /// Mesa 系驱动（amdgpu、i915、xe、radeon、nouveau……）：VAAPI 开在这块卡的 render 节点上。
    Vaapi { render_node: PathBuf },
    /// NVIDIA 专有驱动：NVDEC。CUDA 的设备序号按 PCI 总线顺序数（`CUDA_DEVICE_ORDER=PCI_BUS_ID`），
    /// 这样才对得上同一块卡；默认顺序是「最快的在前」，可能指到别的卡。
    Cuda { device_index: usize },
}

impl HardwareDecoder {
    /// `-init_hw_device` 的值：建一个叫 hw 的设备。
    fn device_spec(&self) -> String {
        match self {
            HardwareDecoder::Vaapi { render_node } => {
                format!("vaapi=hw:{}", render_node.display())
            }
            HardwareDecoder::Cuda { device_index } => format!("cuda=hw:{device_index}"),
        }
    }

    fn api(&self) -> &'static str {
        match self {
            HardwareDecoder::Vaapi { .. } => "vaapi",
            HardwareDecoder::Cuda { .. } => "cuda",
        }
    }

    /// 解码命令里写在 `-i` 之前的参数。
    pub(crate) fn input_arguments(&self) -> [String; 6] {
        [
            "-init_hw_device".to_owned(),
            self.device_spec(),
            "-hwaccel".to_owned(),
            self.api().to_owned(),
            "-hwaccel_device".to_owned(),
            "hw".to_owned(),
        ]
    }

    /// 子进程要带的环境变量。
    pub(crate) fn environment(&self) -> Option<(&'static str, &'static str)> {
        match self {
            HardwareDecoder::Vaapi { .. } => None,
            HardwareDecoder::Cuda { .. } => Some(("CUDA_DEVICE_ORDER", "PCI_BUS_ID")),
        }
    }
}

static STARTED: Once = Once::new();
static DECODER: OnceLock<Option<HardwareDecoder>> = OnceLock::new();

/// 开始在后台探测（只探一次），不阻塞调用方。
pub(crate) fn start_detection() {
    STARTED.call_once(|| {
        // 起不了线程就不探，一直用 CPU 解。
        let _ = std::thread::Builder::new()
            .name("video-hwaccel-probe".to_owned())
            .spawn(|| {
                let _ = DECODER.set(detect());
            });
    });
}

/// 探好且能用的硬解；还没探完或不能用都是 None（用 CPU 解）。
pub(crate) fn decoder() -> Option<&'static HardwareDecoder> {
    DECODER.get()?.as_ref()
}

fn detect() -> Option<HardwareDecoder> {
    let render_node = display_renderer::display_render_node()?;
    let decoder = decoder_for(&render_node, Path::new("/sys"))?;
    let started = Instant::now();
    let mut probe = Command::new("ffmpeg");
    probe
        .args(["-nostdin", "-hide_banner", "-v", "quiet", "-init_hw_device"])
        .arg(decoder.device_spec())
        .args(["-f", "lavfi", "-i", "nullsrc=s=16x16:d=0.04"])
        .args(["-frames:v", "1", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((name, value)) = decoder.environment() {
        probe.env(name, value);
    }
    let opens = probe.status().is_ok_and(|status| status.success());
    (opens && started.elapsed() <= MAX_PROBE_TIME).then_some(decoder)
}

/// 这块卡（render 节点）该用哪种硬解：看 sysfs 里它的驱动；NVIDIA 再按 PCI 地址数出它是第几块
/// NVIDIA 卡。只读 sysfs，不打开设备，不会唤醒断电的卡。
fn decoder_for(render_node: &Path, sys_root: &Path) -> Option<HardwareDecoder> {
    let device = sys_root
        .join("class/drm")
        .join(render_node.file_name()?)
        .join("device");
    let driver = std::fs::read_link(device.join("driver")).ok()?;
    if driver.file_name()? != NVIDIA_DRIVER {
        return Some(HardwareDecoder::Vaapi {
            render_node: render_node.to_path_buf(),
        });
    }
    let address = std::fs::canonicalize(&device).ok()?.file_name()?.to_owned();
    let mut nvidia_cards = std::fs::read_dir(sys_root.join("bus/pci/devices"))
        .ok()?
        .flatten()
        .filter(|entry| {
            let field = |name: &str| {
                std::fs::read_to_string(entry.path().join(name))
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default()
            };
            field("vendor") == NVIDIA_VENDOR && field("class").starts_with("0x03")
        })
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    nvidia_cards.sort();
    let device_index = nvidia_cards.iter().position(|card| *card == address)?;
    Some(HardwareDecoder::Cuda { device_index })
}

#[cfg(test)]
#[path = "hwaccel_tests.rs"]
mod tests;
