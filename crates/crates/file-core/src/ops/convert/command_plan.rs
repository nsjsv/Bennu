use std::time::Duration;

use super::{
    AudioTargetFormat, ConversionQualityPreset, ConversionRequest, ConversionTarget, QualitySpec,
    ResizeSpec, VideoTargetFormat,
};
use crate::FileError;

/// 视频体积模式中给视频流码率的下限:低于此值画质不可用,直接钳制并让达标判定失败。
pub(super) const MIN_VIDEO_BITRATE_BPS: u64 = 50_000;
/// 码率估算预留的容器开销比例(千分之九百八十,即扣 2%)。
const CONTAINER_OVERHEAD_PERMILLE: u64 = 980;
/// 质量模式下视频音轨码率;体积模式下压到 96k 给视频流让空间。
pub(super) const TRACK_AUDIO_BITRATE_BPS_QUALITY_MODE: u64 = 128_000;
pub(super) const TRACK_AUDIO_BITRATE_BPS_TARGET_MODE: u64 = 96_000;

/// MP3 可用码率档(降序);体积模式选择不超过估算值的最大档。
const MP3_BITRATE_STEPS_BPS: [u64; 11] = [
    320_000, 256_000, 224_000, 192_000, 160_000, 128_000, 112_000, 96_000, 80_000, 64_000, 32_000,
];

/// 图片 ffmpeg 编码的可调参数;quality 型与 AVIF 的参数体积方向相反,由调用方处理二分方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageEncodingParameter {
    /// -quality 类参数(libwebp -quality、libjxl -q:v),0-100,越大体积越大。
    EncoderQuality(u8),
    /// AV1 的 -crf,0-63,越大体积越小。
    AvifCrf(u32),
}

impl ImageEncodingParameter {
    pub(super) fn search_bounds(self) -> (u32, u32) {
        match self {
            Self::EncoderQuality(_) => (0, 100),
            Self::AvifCrf(_) => (0, 63),
        }
    }

    /// 二分中点换算成具体参数。
    pub(super) fn with_scalar(self, value: u32) -> Self {
        match self {
            Self::EncoderQuality(_) => Self::EncoderQuality(value.clamp(0, 100) as u8),
            Self::AvifCrf(_) => Self::AvifCrf(value.min(63)),
        }
    }

    /// quality 型参数体积单调递增,二分满足后向上找;crf 型相反。
    pub(super) fn is_quality_direction(self) -> bool {
        matches!(self, Self::EncoderQuality(_))
    }
}

/// 视频目标的编码计划:每种「容器 × 编码」组合一份静态数据。
pub(super) struct VideoPlan {
    pub(super) video_encoder: &'static str,
    /// 编码器附加旗标(如 x264 的 -preset)。
    pub(super) encoder_extra: &'static [&'static str],
    /// 质量档位参数名:主流编码器用 -crf,mpeg4 用 -q:v。
    pub(super) quality_flag: &'static str,
    /// (低, 中, 高)档位取值;值越小画质越高(crf/q 语义一致)。
    pub(super) preset_values: (u32, u32, u32),
    /// 恒定质量模式所需的额外参数(VP9 必须 -b:v 0)。
    pub(super) crf_mode_extra: &'static [&'static str],
    pub(super) audio_track: VideoAudioTrack,
    /// 容器旗标(如 MP4/MOV 的 +faststart)。
    pub(super) container_flags: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VideoAudioTrack {
    Aac,
    Opus,
    Mp3,
    None,
}

impl VideoAudioTrack {
    fn encoder(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "libopus",
            Self::Mp3 => "libmp3lame",
            Self::None => "none",
        }
    }
}

/// 每目标的编码计划;GIF 不在此表,走独立调色板流程。
pub(super) fn video_plan(format: VideoTargetFormat) -> VideoPlan {
    match format {
        VideoTargetFormat::Mp4H264 => VideoPlan {
            video_encoder: "libx264",
            encoder_extra: &["-preset", "medium"],
            quality_flag: "-crf",
            preset_values: (28, 23, 18),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Aac,
            container_flags: &["-movflags", "+faststart"],
        },
        VideoTargetFormat::Mp4H265 => VideoPlan {
            video_encoder: "libx265",
            encoder_extra: &["-preset", "medium"],
            quality_flag: "-crf",
            preset_values: (30, 26, 22),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Aac,
            container_flags: &["-movflags", "+faststart"],
        },
        VideoTargetFormat::MkvH265 => VideoPlan {
            video_encoder: "libx265",
            encoder_extra: &["-preset", "medium"],
            quality_flag: "-crf",
            preset_values: (30, 26, 22),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Aac,
            container_flags: &[],
        },
        VideoTargetFormat::MkvAv1 => VideoPlan {
            video_encoder: "libsvtav1",
            encoder_extra: &[],
            quality_flag: "-crf",
            preset_values: (45, 35, 25),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Aac,
            container_flags: &[],
        },
        VideoTargetFormat::WebmVp9 => VideoPlan {
            video_encoder: "libvpx-vp9",
            encoder_extra: &[],
            quality_flag: "-crf",
            preset_values: (35, 31, 27),
            crf_mode_extra: &["-b:v", "0"],
            audio_track: VideoAudioTrack::Opus,
            container_flags: &[],
        },
        VideoTargetFormat::WebmAv1 => VideoPlan {
            video_encoder: "libsvtav1",
            encoder_extra: &[],
            quality_flag: "-crf",
            preset_values: (45, 35, 25),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Opus,
            container_flags: &[],
        },
        VideoTargetFormat::MovH264 => VideoPlan {
            video_encoder: "libx264",
            encoder_extra: &["-preset", "medium"],
            quality_flag: "-crf",
            preset_values: (28, 23, 18),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Aac,
            container_flags: &["-movflags", "+faststart"],
        },
        VideoTargetFormat::AviMpeg4 => VideoPlan {
            video_encoder: "mpeg4",
            encoder_extra: &[],
            quality_flag: "-q:v",
            preset_values: (6, 4, 2),
            crf_mode_extra: &[],
            audio_track: VideoAudioTrack::Mp3,
            container_flags: &[],
        },
        VideoTargetFormat::Gif => unreachable!("gif uses the dedicated palette plan"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolvedRate {
    Video { video_bps: u64, audio_bps: u64 },
    Audio { bps: u64 },
}

pub(super) fn webp_preset_quality(preset: ConversionQualityPreset) -> u8 {
    match preset {
        ConversionQualityPreset::Low => 60,
        ConversionQualityPreset::Medium => 80,
        ConversionQualityPreset::High => 95,
    }
}

pub(super) fn avif_preset_crf(preset: ConversionQualityPreset) -> u32 {
    match preset {
        ConversionQualityPreset::Low => 45,
        ConversionQualityPreset::Medium => 35,
        ConversionQualityPreset::High => 25,
    }
}

/// 图片质量滑块 0-100 → AV1 crf 域 0-63(方向反转)。
pub(super) fn quality_level_to_avif_crf(level: u8) -> u32 {
    let level = u32::from(level).min(100);
    ((100 - level) * 63 + 50) / 100
}

/// 体积模式 → 码率估算;视频按时长均摊,音频选可用码率档。
pub(super) fn resolve_targeted_rate(
    request: &ConversionRequest,
    duration: Option<Duration>,
) -> Result<Option<ResolvedRate>, FileError> {
    let QualitySpec::TargetBytes(target_bytes) = request.quality else {
        return Ok(None);
    };
    let duration = duration.ok_or_else(|| FileError::Convert {
        path: request.source.clone(),
        message: "target size mode requires a readable media duration".to_owned(),
    })?;
    if duration.is_zero() {
        return Err(FileError::Convert {
            path: request.source.clone(),
            message: "media duration is zero; cannot estimate a bitrate".to_owned(),
        });
    }

    match request.target {
        // GIF 调色板编码没有码率语义,校验层已拒绝目标体积组合。
        ConversionTarget::Video(VideoTargetFormat::Gif) => Ok(None),
        ConversionTarget::Video(_) => {
            let audio_bps = TRACK_AUDIO_BITRATE_BPS_TARGET_MODE;
            let video_bps = bitrate_budget_bps(target_bytes, duration).saturating_sub(audio_bps);
            Ok(Some(ResolvedRate::Video {
                video_bps: video_bps.max(MIN_VIDEO_BITRATE_BPS),
                audio_bps,
            }))
        }
        ConversionTarget::Audio(AudioTargetFormat::Mp3) => {
            let budget_bps = bitrate_budget_bps(target_bytes, duration);
            let bps = MP3_BITRATE_STEPS_BPS
                .iter()
                .copied()
                .find(|step| *step <= budget_bps)
                .unwrap_or(*MP3_BITRATE_STEPS_BPS.last().expect("non-empty steps"));
            Ok(Some(ResolvedRate::Audio { bps }))
        }
        ConversionTarget::Audio(AudioTargetFormat::Opus) => {
            let budget_bps = bitrate_budget_bps(target_bytes, duration).min(256_000);
            Ok(Some(ResolvedRate::Audio {
                bps: budget_bps.max(6_000),
            }))
        }
        ConversionTarget::Audio(AudioTargetFormat::M4a) => {
            let budget_bps = bitrate_budget_bps(target_bytes, duration).clamp(16_000, 320_000);
            Ok(Some(ResolvedRate::Audio { bps: budget_bps }))
        }
        ConversionTarget::Audio(AudioTargetFormat::Ogg) => {
            let budget_bps = bitrate_budget_bps(target_bytes, duration).max(24_000);
            Ok(Some(ResolvedRate::Audio { bps: budget_bps }))
        }
        // FLAC/WAV 是无损目标,校验层已拒绝目标体积组合。
        ConversionTarget::Audio(AudioTargetFormat::Flac | AudioTargetFormat::Wav) => Ok(None),
        ConversionTarget::Image(_) => Ok(None),
    }
}

/// 目标字节 → 总码率预算(扣 2% 容器开销):bits*980/millis。
fn bitrate_budget_bps(target_bytes: u64, duration: Duration) -> u64 {
    let millis = duration.as_millis().max(1);
    ((u128::from(target_bytes) * 8 * u128::from(CONTAINER_OVERHEAD_PERMILLE)) / millis) as u64
}

/// 视频流编码参数:质量模式用编码器档位,体积模式用估算码率 + 上限封顶。
pub(super) fn video_stream_args(
    format: VideoTargetFormat,
    quality: &QualitySpec,
    rate: Option<ResolvedRate>,
) -> Vec<String> {
    let plan = video_plan(format);
    let mut args = vec!["-c:v".to_owned(), plan.video_encoder.to_owned()];
    args.extend(plan.encoder_extra.iter().map(|flag| (*flag).to_owned()));

    match rate {
        Some(ResolvedRate::Video {
            video_bps,
            audio_bps,
        }) => {
            args.extend(bitrate_args(video_bps));
            args.extend(audio_track_args(plan.audio_track, Some(audio_bps)));
        }
        _ => {
            args.extend(plan.crf_mode_extra.iter().map(|flag| (*flag).to_owned()));
            args.push(plan.quality_flag.to_owned());
            args.push(video_preset_value(quality, plan.preset_values).to_string());
            args.extend(audio_track_args(plan.audio_track, None));
        }
    }
    args.extend(plan.container_flags.iter().map(|flag| (*flag).to_owned()));
    args
}

/// GIF 走调色板两段滤镜:fps/缩放 → palettegen → paletteuse。
/// 质量档位映射帧率与调色板色数;用户帧率覆盖优先生效。
pub(super) fn gif_stream_args(
    preset: ConversionQualityPreset,
    resize: ResizeSpec,
    fps_override: Option<u32>,
) -> Vec<String> {
    let (preset_fps, max_colors) = match preset {
        ConversionQualityPreset::Low => (10u32, 64u32),
        ConversionQualityPreset::Medium => (15, 128),
        ConversionQualityPreset::High => (24, 256),
    };
    let fps = fps_override.unwrap_or(preset_fps);
    let mut chain = format!("fps={fps}");
    if let Some(scale) = scale_expression(resize) {
        chain.push_str(&format!(",{scale}:flags=lanczos"));
    }
    chain.push_str(&format!(
        ",split[p][q];[p]palettegen=max_colors={max_colors}[out];[q][out]paletteuse=dither=sierra2_4a"
    ));
    vec!["-filter_complex".to_owned(), chain, "-an".to_owned()]
}

fn audio_track_args(track: VideoAudioTrack, bitrate_bps: Option<u64>) -> Vec<String> {
    if track == VideoAudioTrack::None {
        return vec!["-an".to_owned()];
    }
    let bitrate = bitrate_bps.unwrap_or(TRACK_AUDIO_BITRATE_BPS_QUALITY_MODE);
    vec![
        "-c:a".to_owned(),
        track.encoder().to_owned(),
        "-b:a".to_owned(),
        format!("{}k", bitrate / 1000),
    ]
}

fn bitrate_args(video_bps: u64) -> Vec<String> {
    let kbps = (video_bps / 1000).max(1);
    let max_kbps = kbps * 3 / 2;
    let buffer_kbps = kbps * 2;
    vec![
        "-b:v".to_owned(),
        format!("{kbps}k"),
        "-maxrate".to_owned(),
        format!("{max_kbps}k"),
        "-bufsize".to_owned(),
        format!("{buffer_kbps}k"),
    ]
}

/// crf/q 值越小画质越高,档位顺序 (低, 中, 高) 与取值单调对应。
fn video_preset_value(quality: &QualitySpec, preset_values: (u32, u32, u32)) -> u32 {
    match quality {
        QualitySpec::Preset(ConversionQualityPreset::Low) => preset_values.0,
        QualitySpec::Preset(ConversionQualityPreset::High) => preset_values.2,
        _ => preset_values.1,
    }
}

pub(super) fn audio_encoder_name(format: AudioTargetFormat) -> &'static str {
    match format {
        AudioTargetFormat::Mp3 => "libmp3lame",
        AudioTargetFormat::M4a => "aac",
        AudioTargetFormat::Opus => "libopus",
        AudioTargetFormat::Ogg => "libvorbis",
        AudioTargetFormat::Flac => "flac",
        AudioTargetFormat::Wav => "pcm_s16le",
    }
}

/// 质量模式的音频档位:码率族按档位取码率,ogg 用 q 标度,无损无参数。
pub(super) fn quality_mode_audio_args(
    format: AudioTargetFormat,
    preset: ConversionQualityPreset,
) -> Vec<String> {
    let bitrate_kbps = match (format, preset) {
        (AudioTargetFormat::Mp3, ConversionQualityPreset::Low) => 128,
        (AudioTargetFormat::Mp3, ConversionQualityPreset::Medium) => 192,
        (AudioTargetFormat::Mp3, ConversionQualityPreset::High) => 320,
        (AudioTargetFormat::M4a, ConversionQualityPreset::Low) => 128,
        (AudioTargetFormat::M4a, ConversionQualityPreset::Medium) => 192,
        (AudioTargetFormat::M4a, ConversionQualityPreset::High) => 256,
        (AudioTargetFormat::Opus, ConversionQualityPreset::Low) => 96,
        (AudioTargetFormat::Opus, ConversionQualityPreset::Medium) => 128,
        (AudioTargetFormat::Opus, ConversionQualityPreset::High) => 192,
        _ => return ogg_or_lossless_audio_args(format, preset),
    };
    vec!["-b:a".to_owned(), format!("{bitrate_kbps}k")]
}

fn ogg_or_lossless_audio_args(
    format: AudioTargetFormat,
    preset: ConversionQualityPreset,
) -> Vec<String> {
    match format {
        AudioTargetFormat::Ogg => {
            let q = match preset {
                ConversionQualityPreset::Low => "4",
                ConversionQualityPreset::High => "8",
                _ => "6",
            };
            vec!["-q:a".to_owned(), q.to_owned()]
        }
        AudioTargetFormat::Flac | AudioTargetFormat::Wav => Vec::new(),
        // 其余格式在 bitrate_kbps 已返回。
        _ => Vec::new(),
    }
}

/// 缩放表达式;宽度向偶数取整以满足 H.264/VP9 等编码器的偶数尺寸要求。
pub(super) fn scale_expression(resize: ResizeSpec) -> Option<String> {
    match resize {
        ResizeSpec::Keep => None,
        ResizeSpec::Percent(percent) => Some(format!("scale=trunc(iw*{percent}/100/2)*2:-2")),
        ResizeSpec::Width(width) => {
            let even_width = (width.div_ceil(2) * 2).max(2);
            Some(format!("scale={even_width}:-2"))
        }
    }
}

pub(super) fn scale_filter_args(resize: ResizeSpec) -> Vec<String> {
    match scale_expression(resize) {
        Some(scale) => vec!["-vf".to_owned(), scale],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::AudioChannelSpec;
    use super::*;
    use crate::ops::convert::{ConversionRequest, ConversionTarget};

    fn request(target: ConversionTarget, quality: QualitySpec) -> ConversionRequest {
        ConversionRequest {
            source: PathBuf::from("/tmp/convert-tests/clip.mov"),
            target,
            quality,
            resize: ResizeSpec::Keep,
            fps_override: None,
            audio_channels: AudioChannelSpec::Keep,
        }
    }

    #[test]
    fn quality_level_maps_to_avif_crf_inverted() {
        assert_eq!(quality_level_to_avif_crf(100), 0);
        assert_eq!(quality_level_to_avif_crf(0), 63);
        assert_eq!(quality_level_to_avif_crf(50), 32);
    }

    #[test]
    fn video_plans_cover_all_targets_except_gif() {
        for format in [
            VideoTargetFormat::Mp4H264,
            VideoTargetFormat::Mp4H265,
            VideoTargetFormat::MkvH265,
            VideoTargetFormat::MkvAv1,
            VideoTargetFormat::WebmVp9,
            VideoTargetFormat::WebmAv1,
            VideoTargetFormat::MovH264,
            VideoTargetFormat::AviMpeg4,
        ] {
            let plan = video_plan(format);
            assert!(!plan.video_encoder.is_empty(), "{format:?}");
            assert!(!plan.quality_flag.is_empty(), "{format:?}");
        }
    }

    #[test]
    fn target_size_resolves_video_bitrate_estimate() {
        let request = request(
            ConversionTarget::Video(VideoTargetFormat::Mp4H264),
            QualitySpec::TargetBytes(1024 * 1024),
        );
        let rate = resolve_targeted_rate(&request, Some(Duration::from_secs(60)))
            .expect("resolves")
            .expect("video rate");
        let ResolvedRate::Video {
            video_bps,
            audio_bps,
        } = rate
        else {
            panic!("expected video rate");
        };
        // 1MB / 60s ≈ 139k bps 总预算,扣 96k 音轨后视频预算低于下限 → 钳制。
        assert_eq!(video_bps, MIN_VIDEO_BITRATE_BPS);
        assert_eq!(audio_bps, TRACK_AUDIO_BITRATE_BPS_TARGET_MODE);
    }

    #[test]
    fn gif_target_size_is_rejected_by_validation() {
        let request = request(
            ConversionTarget::Video(VideoTargetFormat::Gif),
            QualitySpec::TargetBytes(1024 * 1024),
        );
        assert!(
            resolve_targeted_rate(&request, Some(Duration::from_secs(60)))
                .expect("resolves")
                .is_none()
        );
    }

    #[test]
    fn target_size_requires_duration() {
        let request = request(
            ConversionTarget::Video(VideoTargetFormat::Mp4H264),
            QualitySpec::TargetBytes(1024 * 1024),
        );
        assert!(resolve_targeted_rate(&request, None).is_err());
    }

    #[test]
    fn mp3_bitrate_steps_pick_largest_affordable() {
        let request = request(
            ConversionTarget::Audio(AudioTargetFormat::Mp3),
            QualitySpec::TargetBytes(1_000_000),
        );
        // 1MB / 100s ≈ 78.4k bps 预算 → 选 64k 档。
        let rate = resolve_targeted_rate(&request, Some(Duration::from_secs(100)))
            .expect("resolves")
            .expect("audio rate");
        assert_eq!(rate, ResolvedRate::Audio { bps: 64_000 });
    }

    #[test]
    fn scale_filter_enforces_even_width() {
        // 自定义宽度向上取偶,避免缩小丢列。
        assert_eq!(
            scale_filter_args(ResizeSpec::Width(1921)),
            vec!["-vf".to_owned(), "scale=1922:-2".to_owned()]
        );
        assert_eq!(
            scale_filter_args(ResizeSpec::Width(1920)),
            vec!["-vf".to_owned(), "scale=1920:-2".to_owned()]
        );
        assert_eq!(
            scale_filter_args(ResizeSpec::Percent(50)),
            vec!["-vf".to_owned(), "scale=trunc(iw*50/100/2)*2:-2".to_owned()]
        );
        assert!(scale_filter_args(ResizeSpec::Keep).is_empty());
    }

    #[test]
    fn video_stream_args_follow_plan_table() {
        let crf_args = video_stream_args(
            VideoTargetFormat::Mp4H264,
            &QualitySpec::Preset(ConversionQualityPreset::Medium),
            None,
        );
        assert!(crf_args.windows(2).any(|pair| pair == ["-crf", "23"]));
        assert!(crf_args.windows(2).any(|pair| pair == ["-c:a", "aac"]));

        let rate_args = video_stream_args(
            VideoTargetFormat::Mp4H264,
            &QualitySpec::TargetBytes(1024 * 1024),
            Some(ResolvedRate::Video {
                video_bps: 400_000,
                audio_bps: 96_000,
            }),
        );
        assert!(rate_args.windows(2).any(|pair| pair == ["-b:v", "400k"]));
        assert!(rate_args
            .windows(2)
            .any(|pair| pair == ["-maxrate", "600k"]));
        assert!(rate_args.windows(2).any(|pair| pair == ["-b:a", "96k"]));

        let vp9_args = video_stream_args(
            VideoTargetFormat::WebmVp9,
            &QualitySpec::Preset(ConversionQualityPreset::Low),
            None,
        );
        assert!(vp9_args.windows(2).any(|pair| pair == ["-b:v", "0"]));
        assert!(vp9_args.windows(2).any(|pair| pair == ["-crf", "35"]));

        // AVI 的 mpeg4 用 -q:v 档位 + mp3 音轨。
        let avi_args = video_stream_args(
            VideoTargetFormat::AviMpeg4,
            &QualitySpec::Preset(ConversionQualityPreset::High),
            None,
        );
        assert!(avi_args.windows(2).any(|pair| pair == ["-q:v", "2"]));
        assert!(avi_args
            .windows(2)
            .any(|pair| pair == ["-c:a", "libmp3lame"]));
    }

    #[test]
    fn gif_stream_args_build_palette_chain() {
        let args = gif_stream_args(
            ConversionQualityPreset::Medium,
            ResizeSpec::Width(480),
            None,
        );
        let filter = args
            .iter()
            .position(|arg| arg == "-filter_complex")
            .map(|index| args[index + 1].clone())
            .expect("filter complex");
        assert!(filter.starts_with("fps=15,"));
        assert!(filter.contains("scale=480:-2:flags=lanczos"));
        assert!(filter.contains("palettegen=max_colors=128"));
        assert!(filter.contains("paletteuse=dither=sierra2_4a"));
        assert!(args.iter().any(|arg| arg == "-an"));

        // 帧率覆盖优先于档位默认。
        let overridden = gif_stream_args(ConversionQualityPreset::High, ResizeSpec::Keep, Some(12));
        let filter = overridden
            .iter()
            .position(|arg| arg == "-filter_complex")
            .map(|index| overridden[index + 1].clone())
            .expect("filter complex");
        assert!(filter.starts_with("fps=12,"));
        assert!(filter.contains("palettegen=max_colors=256"));
    }

    #[test]
    fn audio_quality_presets_map_to_bitrate_ladder() {
        assert_eq!(
            quality_mode_audio_args(AudioTargetFormat::Mp3, ConversionQualityPreset::High),
            vec!["-b:a".to_owned(), "320k".to_owned()]
        );
        assert_eq!(
            quality_mode_audio_args(AudioTargetFormat::M4a, ConversionQualityPreset::Low),
            vec!["-b:a".to_owned(), "128k".to_owned()]
        );
        assert_eq!(
            quality_mode_audio_args(AudioTargetFormat::Opus, ConversionQualityPreset::Medium),
            vec!["-b:a".to_owned(), "128k".to_owned()]
        );
        assert_eq!(
            quality_mode_audio_args(AudioTargetFormat::Ogg, ConversionQualityPreset::High),
            vec!["-q:a".to_owned(), "8".to_owned()]
        );
        assert!(
            quality_mode_audio_args(AudioTargetFormat::Wav, ConversionQualityPreset::High)
                .is_empty()
        );
    }
}
