//! YUV 转 RGB、HDR 转普通亮度。显卡着色器（`video.wgsl`）和软件渲染兜底用同一套公式：
//! 参数在这里算好传给着色器，CPU 兜底直接调这里的函数，改公式两边一起改。
//!
//! HDR 的做法：PQ/HLG 先换成绝对亮度（尼特），BT.2020 原色换到 BT.709，再用 BT.2390
//! 的高光压缩把母带峰值压到普通视频的白（203 尼特，BT.2408）以内——暗部和中间调原样保留，
//! 只压亮部，最后按普通视频的 2.4 伽马编码。

use crate::video_preview::{VideoColor, VideoPrimaries, VideoTransfer, YuvMatrix};

/// 普通视频的白对应多少尼特（BT.2408 参考白）。
pub(crate) const SDR_WHITE_NITS: f32 = 203.0;
/// 没有母带元数据时按这个峰值压缩（HDR10 母带和 HLG 标称峰值都常是 1000 尼特）。
pub(crate) const HDR_SOURCE_PEAK_NITS: f32 = 1000.0;
/// 普通视频显示时的伽马（BT.1886）。
const DISPLAY_GAMMA: f32 = 2.4;

/// 一个采样占几个字节：普通视频 8 位（nv12），HDR 用 16 位容器装 10 位（p010，值在高 10 位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleDepth {
    Eight,
    Sixteen,
}

impl SampleDepth {
    pub(crate) fn bytes(self) -> usize {
        match self {
            SampleDepth::Eight => 1,
            SampleDepth::Sixteen => 2,
        }
    }

    /// 原始采样值除以它得到 8 位等价码值（p010 的 10 位值左移了 6 位，10 位码值再除以 4）。
    pub(crate) fn code_divisor(self) -> f32 {
        match self {
            SampleDepth::Eight => 1.0,
            SampleDepth::Sixteen => 256.0,
        }
    }
}

/// 着色器 `Params.color` 的内容（布局见 `video.wgsl`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColorParams {
    /// R、G、B 三行：`[Y 系数, Cb 系数, Cr 系数, 0]`，作用在归一化后的 Y、Cb、Cr 上。
    pub rows: [[f32; 4]; 3],
    /// x：码值除数；y：传输（0 普通、1 PQ、2 HLG）；z：要不要从 BT.2020 换到 BT.709。
    pub modes: [f32; 4],
}

impl ColorParams {
    pub(crate) fn new(color: VideoColor, depth: SampleDepth) -> Self {
        let (kr, kb) = match color.matrix {
            YuvMatrix::Bt601 => (0.299, 0.114),
            YuvMatrix::Bt709 => (0.2126, 0.0722),
            YuvMatrix::Bt2020 => (0.2627, 0.0593),
        };
        let kg = 1.0 - kr - kb;
        let transfer = match color.transfer {
            VideoTransfer::Sdr => 0.0,
            VideoTransfer::Pq => 1.0,
            VideoTransfer::Hlg => 2.0,
        };
        let convert_gamut = match color.primaries {
            VideoPrimaries::Bt709 => 0.0,
            VideoPrimaries::Bt2020 => 1.0,
        };
        Self {
            rows: [
                [1.0, 0.0, 2.0 * (1.0 - kr), 0.0],
                [
                    1.0,
                    -2.0 * kb * (1.0 - kb) / kg,
                    -2.0 * kr * (1.0 - kr) / kg,
                    0.0,
                ],
                [1.0, 2.0 * (1.0 - kb), 0.0, 0.0],
            ],
            modes: [depth.code_divisor(), transfer, convert_gamut, 0.0],
        }
    }

    /// 一个像素：原始采样值 → 按显示伽马编码的 RGB（0–1）。解码输出一律是有限范围（ffmpeg
    /// 的 `out_range=tv`），这里按 16–235 / 16–240 展开。
    pub(crate) fn display_rgb(&self, y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let divisor = self.modes[0];
        let luma = (y / divisor - 16.0) / 219.0;
        let blue = (cb / divisor - 128.0) / 224.0;
        let red = (cr / divisor - 128.0) / 224.0;
        let encoded = self
            .rows
            .map(|row| (row[0] * luma + row[1] * blue + row[2] * red).clamp(0.0, 1.0));
        let linear = match self.modes[1] as u32 {
            1 => tone_map(gamut(pq_nits(encoded), self.modes[2])),
            2 => tone_map(gamut(hlg_nits(encoded), self.modes[2])),
            // 普通视频原样显示；少见的 BT.2020 原色普通视频才需要先解伽马换原色。
            _ if self.modes[2] > 0.5 => gamut(encoded.map(|value| value.powf(DISPLAY_GAMMA)), 1.0),
            _ => return encoded,
        };
        linear.map(|value| value.clamp(0.0, 1.0).powf(1.0 / DISPLAY_GAMMA))
    }
}

/// PQ（SMPTE ST 2084）编码值 → 尼特。
pub(crate) fn pq_to_nits(encoded: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let power = encoded.max(0.0).powf(1.0 / M2);
    10000.0 * ((power - C1).max(0.0) / (C2 - C3 * power)).powf(1.0 / M1)
}

/// 尼特 → PQ 编码值。
pub(crate) fn nits_to_pq(nits: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let power = (nits / 10000.0).max(0.0).powf(M1);
    ((C1 + C2 * power) / (1.0 + C3 * power)).powf(M2)
}

fn pq_nits(encoded: [f32; 3]) -> [f32; 3] {
    encoded.map(pq_to_nits)
}

/// HLG：反 OETF 得场景光，再按 1000 尼特显示的系统伽马 1.2 做 OOTF。
fn hlg_nits(encoded: [f32; 3]) -> [f32; 3] {
    let scene = encoded.map(hlg_to_scene);
    let luminance = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
    let gain = HDR_SOURCE_PEAK_NITS * luminance.max(0.0).powf(0.2);
    scene.map(|value| value * gain)
}

pub(crate) fn hlg_to_scene(encoded: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    if encoded <= 0.5 {
        encoded * encoded / 3.0
    } else {
        (((encoded - C) / A).exp() + B) / 12.0
    }
}

/// 线性光的 BT.2020 → BT.709（`enabled` 为 0 时原样）。
fn gamut(rgb: [f32; 3], enabled: f32) -> [f32; 3] {
    if enabled < 0.5 {
        return rgb;
    }
    let [r, g, b] = rgb;
    [
        1.6605 * r - 0.5876 * g - 0.0728 * b,
        -0.1246 * r + 1.1329 * g - 0.0083 * b,
        -0.0182 * r - 0.1006 * g + 1.1187 * b,
    ]
}

/// 按最亮的那个通道压：色相不变，压完不会有通道溢出。结果是相对普通视频白的线性值。
fn tone_map(nits: [f32; 3]) -> [f32; 3] {
    let peak = nits[0].max(nits[1]).max(nits[2]);
    if peak <= 0.0 {
        return [0.0; 3];
    }
    let scale = compressed_nits(peak) / peak / SDR_WHITE_NITS;
    nits.map(|value| value.max(0.0) * scale)
}

/// BT.2390 EETF：在 PQ 域里，膝点以下原样，以上用三次 Hermite 曲线把
/// `HDR_SOURCE_PEAK_NITS` 压到 `SDR_WHITE_NITS`。
pub(crate) fn compressed_nits(nits: f32) -> f32 {
    let source_peak = nits_to_pq(HDR_SOURCE_PEAK_NITS);
    let normalized = (nits_to_pq(nits) / source_peak).min(1.0);
    let white = nits_to_pq(SDR_WHITE_NITS) / source_peak;
    let knee = 1.5 * white - 0.5;
    let compressed = if normalized < knee {
        normalized
    } else {
        let t = (normalized - knee) / (1.0 - knee);
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * knee
            + (t3 - 2.0 * t2 + t) * (1.0 - knee)
            + (-2.0 * t3 + 3.0 * t2) * white
    };
    pq_to_nits(compressed * source_peak)
}

#[cfg(test)]
#[path = "color_tests.rs"]
mod tests;
