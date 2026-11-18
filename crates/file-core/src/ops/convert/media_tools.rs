//! 探测 ffmpeg 安装与媒体本身:命令在不在、有哪些编码器、时长多长、有没有音轨。

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::{AudioTargetFormat, ImageTargetFormat, VideoTargetFormat};

/// 与 7z 的候选名循环同模式:预留给发行版改名场景。
pub(super) const FFMPEG_TOOL_CANDIDATES: [&str; 1] = ["ffmpeg"];
const FFPROBE_TOOL_CANDIDATES: [&str; 1] = ["ffprobe"];

/// UI 与执行层共用的 ffmpeg 可执行文件发现:spawn `-version` 成功即视为存在。
pub async fn locate_media_tool(candidates: &[&str]) -> Option<String> {
    for candidate in candidates {
        let spawn_check = Command::new(candidate)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawn_check {
            let _ = child.wait().await;
            return Some((*candidate).to_owned());
        }
    }
    None
}

/// 编码器可用性:UI 据此禁用系统 ffmpeg 不支持的目标格式选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionEncoderAvailability {
    encoder_names: HashSet<String>,
}

impl ConversionEncoderAvailability {
    /// 空集表示 ffmpeg 不可用或探测失败。
    pub fn is_empty(&self) -> bool {
        self.encoder_names.is_empty()
    }

    /// UI 初始状态:尚未探测到任何编码器。
    pub fn unavailable() -> Self {
        Self {
            encoder_names: HashSet::new(),
        }
    }

    /// 名字与 `ffmpeg -encoders` 列出的一致(`libx264`、`aac`……)。
    pub fn from_encoder_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            encoder_names: names.into_iter().map(str::to_owned).collect(),
        }
    }

    pub fn supports_video(&self, format: VideoTargetFormat) -> bool {
        match format {
            VideoTargetFormat::Mp4H264 | VideoTargetFormat::MovH264 => {
                self.encoder_names.contains("libx264")
            }
            VideoTargetFormat::Mp4H265 | VideoTargetFormat::MkvH265 => {
                self.encoder_names.contains("libx265")
            }
            VideoTargetFormat::MkvAv1 | VideoTargetFormat::WebmAv1 => {
                self.any(&["libsvtav1", "libaom-av1"])
            }
            VideoTargetFormat::WebmVp9 => self.encoder_names.contains("libvpx-vp9"),
            // mpeg4 与 gif 是 ffmpeg 内置编码器,存在 ffmpeg 即可用。
            VideoTargetFormat::AviMpeg4 | VideoTargetFormat::Gif => !self.is_empty(),
        }
    }

    pub fn supports_audio(&self, format: AudioTargetFormat) -> bool {
        match format {
            AudioTargetFormat::Mp3 => self.encoder_names.contains("libmp3lame"),
            // aac、flac、pcm_s16le 是内置编码器。
            AudioTargetFormat::M4a => self.encoder_names.contains("aac"),
            AudioTargetFormat::Opus => self.encoder_names.contains("libopus"),
            AudioTargetFormat::Flac => self.encoder_names.contains("flac"),
            AudioTargetFormat::Ogg => self.encoder_names.contains("libvorbis"),
            AudioTargetFormat::Wav => self.encoder_names.contains("pcm_s16le"),
        }
    }

    pub fn supports_lossy_image(&self, format: ImageTargetFormat) -> bool {
        match format {
            ImageTargetFormat::Webp => {
                self.encoder_names.contains("libwebp_anim")
                    || self.encoder_names.contains("libwebp")
            }
            ImageTargetFormat::Avif => self.any(&["libsvtav1", "libaom-av1"]),
            ImageTargetFormat::Jxl => self.encoder_names.contains("libjxl"),
            ImageTargetFormat::Gif => !self.is_empty(),
            // 这几种由 image crate 处理,不依赖 ffmpeg。
            ImageTargetFormat::Jpeg
            | ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico => true,
        }
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.encoder_names.contains(name)
    }

    pub(super) fn any(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.contains(name))
    }
}

/// 解析 `ffmpeg -encoders` 输出;ffmpeg 缺失时返回空集,由 UI 统一禁用选项。
pub async fn available_conversion_encoders() -> ConversionEncoderAvailability {
    let Some(ffmpeg_command) = locate_media_tool(&FFMPEG_TOOL_CANDIDATES).await else {
        return ConversionEncoderAvailability::unavailable();
    };
    probe_encoder_names(&ffmpeg_command)
        .await
        .unwrap_or_else(ConversionEncoderAvailability::unavailable)
}

pub(super) async fn probe_encoder_names(
    ffmpeg_command: &str,
) -> Option<ConversionEncoderAvailability> {
    let output = Command::new(ffmpeg_command)
        .arg("-hide_banner")
        .arg("-encoders")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    // 行形如 " A....L libmp3lame  description";第二个空白分隔字段是编码器名。
    let listing = String::from_utf8_lossy(&output.stdout);
    Some(ConversionEncoderAvailability::from_encoder_names(
        listing
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1)),
    ))
}

/// ffprobe 读取媒体总时长;视频/音频体积模式依赖它做码率估算。
pub async fn media_duration(path: &Path) -> Option<Duration> {
    let ffprobe_command = locate_media_tool(&FFPROBE_TOOL_CANDIDATES).await?;
    let output = Command::new(ffprobe_command)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let seconds = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()?;
    if seconds.is_finite() && seconds > 0.0 {
        Some(Duration::from_secs_f64(seconds))
    } else {
        None
    }
}

pub(super) async fn has_audio_stream(path: &Path) -> Option<bool> {
    let ffprobe_command = locate_media_tool(&FFPROBE_TOOL_CANDIDATES).await?;
    let output = Command::new(ffprobe_command)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_availability_reports_format_support() {
        let availability = ConversionEncoderAvailability::from_encoder_names([
            "libx264",
            "libmp3lame",
            "flac",
            "libwebp",
            "libsvtav1",
            "libjxl",
        ]);

        assert!(availability.supports_video(VideoTargetFormat::Mp4H264));
        assert!(!availability.supports_video(VideoTargetFormat::MkvH265));
        // 内置编码器目标(mpeg4/gif)只要有 ffmpeg 就可用。
        assert!(availability.supports_video(VideoTargetFormat::AviMpeg4));
        assert!(availability.supports_video(VideoTargetFormat::Gif));
        assert!(availability.supports_audio(AudioTargetFormat::Mp3));
        assert!(availability.supports_audio(AudioTargetFormat::Flac));
        assert!(!availability.supports_audio(AudioTargetFormat::Opus));
        assert!(!availability.supports_audio(AudioTargetFormat::Wav));
        assert!(availability.supports_lossy_image(ImageTargetFormat::Webp));
        assert!(availability.supports_lossy_image(ImageTargetFormat::Avif));
        assert!(availability.supports_lossy_image(ImageTargetFormat::Jxl));
        assert!(availability.supports_lossy_image(ImageTargetFormat::Jpeg));
        assert!(availability.supports_lossy_image(ImageTargetFormat::Gif));
        // 没有 libjxl 时 JXL 目标应不可用。
        let without_jxl = ConversionEncoderAvailability::from_encoder_names(["libx264"]);
        assert!(!without_jxl.supports_lossy_image(ImageTargetFormat::Jxl));
    }

    #[test]
    fn missing_ffmpeg_leaves_only_image_crate_targets() {
        let missing = ConversionEncoderAvailability::unavailable();
        assert!(missing.is_empty());
        assert!(!missing.supports_video(VideoTargetFormat::AviMpeg4));
        assert!(!missing.supports_video(VideoTargetFormat::Gif));
        assert!(!missing.supports_lossy_image(ImageTargetFormat::Gif));
        assert!(missing.supports_lossy_image(ImageTargetFormat::Jpeg));
        assert!(missing.supports_lossy_image(ImageTargetFormat::Png));
    }
}
