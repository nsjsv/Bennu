//! 解出来的画面多大、多快、什么格式，以及怎么让 ffmpeg 这样出帧。
//!
//! 输出 YUV 原样字节（nv12，HDR 用 p010），颜色换算交给显卡；画面不超过屏幕大小（4K 视频在
//! 1080p 屏幕上按 1080p 解），改窗口大小不重开解码，缩放由显卡做。帧率按视频自己的，
//! 最高 60；`fps` 滤镜把可变帧率规整成固定帧率，第 i 帧的时间就是 i / 帧率。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use iced::Size;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::video_preview::hwaccel;
use crate::video_preview::subtitles::{text_subtitle_filter, SubtitleBurn};
use crate::video_preview::VideoStreamFacts;
use crate::video_surface::{picture_bytes, SampleDepth, VideoPicture};

/// 帧率上限：更高帧率的视频按这个放，省电。
const MAX_FRAME_RATE: u32 = 60;
/// 拿不到帧率时按这个出帧。
const FALLBACK_FRAME_RATE: (u32, u32) = (30, 1);
/// 不知道屏幕多大时（窗口还没显示到屏幕上），按 4K 封顶。
pub(crate) const FALLBACK_DISPLAY: Size<u32> = Size::new(3840, 2160);

/// 拖进度条时只解最近的关键帧（快，可能差几秒）；松手、按键跳转要精确到帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeekPrecision {
    Keyframe,
    Exact,
}

/// 一个视频在这块屏幕上的出帧方式：打开时按 ffprobe 信息和屏幕大小定一次。
#[derive(Debug, Clone, PartialEq)]
pub struct FramePlan {
    video_ordinal: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    depth: SampleDepth,
    color: crate::video_preview::VideoColor,
    frame_rate: (u32, u32),
    deinterlace: bool,
}

impl FramePlan {
    pub(crate) fn new(video: &VideoStreamFacts, display: Size<u32>) -> Self {
        // 非方形像素在这里拉成方形，显卡只管等比缩放。
        let (natural_width, natural_height) = video.display_size();
        let scale = (f64::from(display.width) / natural_width)
            .min(f64::from(display.height) / natural_height)
            .min(1.0);
        let frame_rate = match video.frame_rate {
            Some((numerator, denominator)) if numerator > MAX_FRAME_RATE * denominator => {
                (MAX_FRAME_RATE, 1)
            }
            Some(rate) => rate,
            None => FALLBACK_FRAME_RATE,
        };
        Self {
            video_ordinal: video.ordinal,
            width: even(natural_width * scale),
            height: even(natural_height * scale),
            depth: if video.color.is_hdr() {
                SampleDepth::Sixteen
            } else {
                SampleDepth::Eight
            },
            color: video.color,
            frame_rate,
            deinterlace: video.interlaced,
        }
    }

    /// 第 `index` 帧离这段解码起点多远。
    pub(crate) fn frame_offset(&self, index: u64) -> Duration {
        let (numerator, denominator) = self.frame_rate;
        Duration::from_secs_f64(index as f64 * f64::from(denominator) / f64::from(numerator))
    }

    fn frame_bytes(&self) -> usize {
        picture_bytes(self.width, self.height, self.depth)
    }

    /// 去隔行（只处理标了隔行的帧）→ 规整帧率 → 缩放并转成有限范围 → 文字字幕 → 像素格式。
    /// 文字字幕放在缩放之后：按屏幕上的大小渲染，清楚也省事。
    fn stream_filter(&self, subtitle: Option<&SubtitleBurn>) -> String {
        let (numerator, denominator) = self.frame_rate;
        let text_subtitle = match subtitle {
            Some(SubtitleBurn::Text(file)) => Some(text_subtitle_filter(file)),
            Some(SubtitleBurn::Image { .. }) | None => None,
        };
        self.deinterlace_filter()
            .into_iter()
            .chain([
                format!("fps={numerator}/{denominator}"),
                self.scale_filter(),
            ])
            .chain(text_subtitle)
            .chain([self.format_filter()])
            .collect::<Vec<_>>()
            .join(",")
    }

    /// 单帧不用规整帧率，也不烧字幕（拖动时的预览、右侧面板的静止画面）。
    fn still_filter(&self) -> String {
        self.deinterlace_filter()
            .into_iter()
            .chain([self.scale_filter(), self.format_filter()])
            .collect::<Vec<_>>()
            .join(",")
    }

    fn deinterlace_filter(&self) -> Option<String> {
        self.deinterlace
            .then(|| "bwdif=deint=interlaced".to_owned())
    }

    fn scale_filter(&self) -> String {
        format!(
            "scale={}:{}:out_range=tv:flags=area",
            self.width, self.height
        )
    }

    fn format_filter(&self) -> String {
        match self.depth {
            SampleDepth::Eight => "format=nv12",
            SampleDepth::Sixteen => "format=p010le",
        }
        .to_owned()
    }
}

/// 取最近的偶数（4:2:0 的色度是一半宽高），至少 2。
fn even(value: f64) -> u32 {
    ((value / 2.0).round() as u32 * 2).max(2)
}

/// 从 `start` 起连续出帧（播放用）。`hwaccel` 是探测出能用的显卡（render 节点，见 `hwaccel`
/// 模块），`subtitle` 是要烧进画面的字幕。
pub(crate) fn stream_command(
    path: &Path,
    plan: &FramePlan,
    start: Duration,
    hwaccel: Option<&Path>,
    subtitle: Option<&SubtitleBurn>,
) -> Command {
    let mut command = base_command();
    if let Some(node) = hwaccel {
        command
            .arg("-init_hw_device")
            .arg(hwaccel::device_argument(node))
            .args(["-hwaccel", "vaapi", "-hwaccel_device", "va"]);
    }
    seek_and_input(&mut command, path, start);
    let filter = plan.stream_filter(subtitle);
    match subtitle {
        // 图片字幕是另一路流，要用滤镜图把它叠到画面上。
        Some(SubtitleBurn::Image { ordinal }) => {
            command
                .arg("-filter_complex")
                .arg(format!(
                    "[0:v:{}][0:s:{ordinal}]overlay=eof_action=pass,{filter}[picture]",
                    plan.video_ordinal
                ))
                .args(["-map", "[picture]"]);
        }
        Some(SubtitleBurn::Text(_)) | None => video_filter(&mut command, plan, filter),
    }
    output(&mut command, &[]);
    command
}

/// 只出 `position` 处的一帧（静止画面、拖动时的预览）。一帧用 CPU 解比初始化显卡还快。
pub(crate) fn still_command(
    path: &Path,
    plan: &FramePlan,
    position: Duration,
    precision: SeekPrecision,
) -> Command {
    let mut command = base_command();
    if precision == SeekPrecision::Keyframe {
        command.arg("-noaccurate_seek");
    }
    seek_and_input(&mut command, path, position);
    video_filter(&mut command, plan, plan.still_filter());
    output(&mut command, &["-frames:v", "1"]);
    command
}

fn base_command() -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .kill_on_drop(true)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    command
}

/// `-ss` 放在 `-i` 前是输入端跳转：先按索引定位到关键帧，精确模式再解码丢弃到目标位置。
fn seek_and_input(command: &mut Command, path: &Path, position: Duration) {
    if position > Duration::ZERO {
        command
            .arg("-ss")
            .arg(format!("{:.3}", position.as_secs_f64()));
    }
    command.arg("-i").arg(path);
}

fn video_filter(command: &mut Command, plan: &FramePlan, filter: String) {
    command
        .arg("-map")
        .arg(format!("0:v:{}", plan.video_ordinal))
        .arg("-vf")
        .arg(filter);
}

/// `limits` 是输出端选项（如只要一帧），必须写在输出目标之前，写在后面 ffmpeg 不认。
fn output(command: &mut Command, limits: &[&str]) {
    command
        .args(["-an", "-sn", "-dn"])
        .args(limits)
        .args(["-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

/// 读下一帧；正好读完时是 `None`，读到半帧是错误。
pub(crate) async fn read_picture<R: AsyncRead + Unpin>(
    reader: &mut R,
    plan: &FramePlan,
) -> std::io::Result<Option<VideoPicture>> {
    let mut bytes = vec![0; plan.frame_bytes()];
    let mut filled = 0;
    while filled < bytes.len() {
        let read = reader.read(&mut bytes[filled..]).await?;
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        filled += read;
    }
    Ok(Some(VideoPicture::new(
        plan.width,
        plan.height,
        plan.depth,
        plan.color,
        bytes,
    )))
}

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;
