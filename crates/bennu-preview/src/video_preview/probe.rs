//! ffprobe 读视频信息：打开时只跑这一次，解码参数、音轨字幕轨、信息层都从这里取。

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::audio_preview::{positive_number, seconds_field};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoProbe {
    pub duration: Option<Duration>,
    /// 容器名（ffprobe 的 format_long_name，没有就用 format_name）。
    pub container: Option<String>,
    pub bit_rate: Option<u64>,
    /// 第一条真正的画面流；只有封面图（attached_pic）时是 None。
    pub video: Option<VideoStreamFacts>,
    pub audio_tracks: Vec<AudioTrack>,
    pub subtitle_tracks: Vec<SubtitleTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStreamFacts {
    /// 视频流里的序号（`-map 0:v:N`；封面图也算在里面）。
    pub ordinal: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// 像素宽高比（DVD 等非方形像素）。
    pub sample_aspect: (u32, u32),
    /// 平均帧率（分子, 分母）；拿不到时是 None。
    pub frame_rate: Option<(u32, u32)>,
    /// 显示时顺时针转多少度（0/90/180/270），手机竖拍的视频常见。
    pub rotation: u16,
    pub pixel_format: Option<String>,
    pub color: VideoColor,
    pub interlaced: bool,
    pub bit_rate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    /// 音频流里的序号（`-map 0:a:N`）。
    pub ordinal: usize,
    pub codec: String,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleTrack {
    /// 字幕流里的序号（`0:s:N`）。
    pub ordinal: usize,
    pub codec: String,
    /// 图片字幕（蓝光 PGS、DVD、DVB）：只能叠图，不能当文字交给字幕渲染器。
    pub is_image: bool,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

impl VideoStreamFacts {
    /// 显示出来的宽高：转正、把非方形像素（DVD 等）拉成方形之后。
    pub fn display_size(&self) -> (f64, f64) {
        let quarter_turn = self.rotation % 180 == 90;
        let (width, height, (aspect_x, aspect_y)) = if quarter_turn {
            (
                self.height,
                self.width,
                (self.sample_aspect.1, self.sample_aspect.0),
            )
        } else {
            (self.width, self.height, self.sample_aspect)
        };
        (
            f64::from(width) * f64::from(aspect_x) / f64::from(aspect_y),
            f64::from(height),
        )
    }
}

/// YUV 转 RGB 用的矩阵。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

/// 亮度编码方式：普通视频、HDR10（PQ）、HLG。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTransfer {
    Sdr,
    Pq,
    Hlg,
}

/// 三原色范围：普通视频按 BT.709 显示，BT.2020 的要先换到 BT.709。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPrimaries {
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoColor {
    pub matrix: YuvMatrix,
    pub transfer: VideoTransfer,
    pub primaries: VideoPrimaries,
}

impl VideoColor {
    pub fn is_hdr(self) -> bool {
        self.transfer != VideoTransfer::Sdr
    }
}

/// 图片字幕的编码；其余字幕都是文字。
const IMAGE_SUBTITLE_CODECS: [&str; 4] =
    ["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle", "xsub"];

#[derive(Debug)]
pub enum VideoProbeError {
    /// 没装 ffmpeg（ffprobe 启动报 NotFound）。
    FfmpegMissing,
    Failed(String),
}

pub fn probe_video_blocking(path: &Path) -> Result<VideoProbe, VideoProbeError> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json"])
        .args(["-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => VideoProbeError::FfmpegMissing,
            _ => VideoProbeError::Failed(format!("could not start ffprobe: {error}")),
        })?;
    video_probe_from_output(&output)
}

fn video_probe_from_output(output: &Output) -> Result<VideoProbe, VideoProbeError> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("ffprobe failed");
        return Err(VideoProbeError::Failed(reason.trim().to_owned()));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| VideoProbeError::Failed(format!("unreadable ffprobe output: {error}")))?;
    Ok(video_probe_from_document(&document))
}

fn video_probe_from_document(document: &Value) -> VideoProbe {
    let streams = document["streams"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let of_type = |kind: &'static str| {
        streams
            .iter()
            .filter(move |stream| stream["codec_type"] == kind)
            .enumerate()
    };
    let format = &document["format"];
    let video = of_type("video")
        .find(|(_, stream)| stream["disposition"]["attached_pic"] != 1)
        .and_then(|(ordinal, stream)| video_stream_facts(ordinal, stream));
    let duration = seconds_field(&format["duration"]).or_else(|| {
        streams
            .iter()
            .find_map(|stream| seconds_field(&stream["duration"]))
    });
    VideoProbe {
        duration,
        container: text(&format["format_long_name"]).or_else(|| text(&format["format_name"])),
        bit_rate: positive_number(&format["bit_rate"]),
        video,
        audio_tracks: of_type("audio")
            .map(|(ordinal, stream)| AudioTrack {
                ordinal,
                codec: text(&stream["codec_name"]).unwrap_or_default(),
                channels: positive_number(&stream["channels"])
                    .and_then(|count| u32::try_from(count).ok()),
                sample_rate: positive_number(&stream["sample_rate"])
                    .and_then(|rate| u32::try_from(rate).ok()),
                language: language(stream),
                title: text(&stream["tags"]["title"]),
                is_default: stream["disposition"]["default"] == 1,
            })
            .collect(),
        subtitle_tracks: of_type("subtitle")
            .map(|(ordinal, stream)| {
                let codec = text(&stream["codec_name"]).unwrap_or_default();
                SubtitleTrack {
                    ordinal,
                    is_image: IMAGE_SUBTITLE_CODECS.contains(&codec.as_str()),
                    codec,
                    language: language(stream),
                    title: text(&stream["tags"]["title"]),
                    is_default: stream["disposition"]["default"] == 1,
                }
            })
            .collect(),
    }
}

fn video_stream_facts(ordinal: usize, stream: &Value) -> Option<VideoStreamFacts> {
    let width = positive_number(&stream["width"]).and_then(|width| u32::try_from(width).ok())?;
    let height =
        positive_number(&stream["height"]).and_then(|height| u32::try_from(height).ok())?;
    let frame_rate =
        rational(&stream["avg_frame_rate"]).or_else(|| rational(&stream["r_frame_rate"]));
    Some(VideoStreamFacts {
        ordinal,
        codec: text(&stream["codec_name"]).unwrap_or_default(),
        width,
        height,
        sample_aspect: rational(&stream["sample_aspect_ratio"]).unwrap_or((1, 1)),
        frame_rate,
        rotation: rotation(stream),
        pixel_format: text(&stream["pix_fmt"]),
        color: color(stream, height),
        // 「unknown」和缺省都按逐行处理：去隔行滤镜本来也只处理标了隔行的帧。
        interlaced: matches!(
            stream["field_order"].as_str(),
            Some("tt" | "bb" | "tb" | "bt")
        ),
        bit_rate: positive_number(&stream["bit_rate"]),
    })
}

/// 没标颜色的按分辨率猜（同 ffmpeg/mpv）：高清用 BT.709，标清用 BT.601。
fn color(stream: &Value, height: u32) -> VideoColor {
    let matrix = match stream["color_space"].as_str() {
        Some("bt709") => YuvMatrix::Bt709,
        Some("bt470bg" | "smpte170m" | "bt601") => YuvMatrix::Bt601,
        Some("bt2020nc" | "bt2020c") => YuvMatrix::Bt2020,
        _ if height >= 720 => YuvMatrix::Bt709,
        _ => YuvMatrix::Bt601,
    };
    let transfer = match stream["color_transfer"].as_str() {
        Some("smpte2084") => VideoTransfer::Pq,
        Some("arib-std-b67") => VideoTransfer::Hlg,
        _ => VideoTransfer::Sdr,
    };
    let primaries = match stream["color_primaries"].as_str() {
        Some("bt2020") => VideoPrimaries::Bt2020,
        _ => VideoPrimaries::Bt709,
    };
    VideoColor {
        matrix,
        transfer,
        primaries,
    }
}

/// 显示矩阵里的 rotation 是逆时针角度（-90 = 顺时针转 90 度显示）；老文件写在 rotate 标签里（顺时针）。
fn rotation(stream: &Value) -> u16 {
    let clockwise = stream["side_data_list"]
        .as_array()
        .and_then(|side_data| {
            side_data
                .iter()
                .find_map(|entry| entry["rotation"].as_f64())
        })
        .map(|degrees| -degrees)
        .or_else(|| text(&stream["tags"]["rotate"])?.parse::<f64>().ok());
    let degrees = clockwise.unwrap_or(0.0).round() as i64;
    // 只认直角；rem_euclid 把负角度折到 0–359。
    match degrees.rem_euclid(360) {
        90 => 90,
        180 => 180,
        270 => 270,
        _ => 0,
    }
}

fn language(stream: &Value) -> Option<String> {
    text(&stream["tags"]["language"]).filter(|language| language != "und")
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// 「30000/1001」这样的分数；分子或分母为 0 的当作没有。
fn rational(value: &Value) -> Option<(u32, u32)> {
    let (numerator, denominator) = value.as_str()?.split_once(['/', ':'])?;
    let numerator = numerator.parse::<u32>().ok().filter(|value| *value > 0)?;
    let denominator = denominator.parse::<u32>().ok().filter(|value| *value > 0)?;
    Some((numerator, denominator))
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
