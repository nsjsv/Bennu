use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::super::FileOperationControls;
use super::command_plan::{
    audio_encoder_name, avif_preset_crf, gif_stream_args, quality_level_to_avif_crf,
    quality_mode_audio_args, resolve_targeted_rate, scale_filter_args, video_stream_args,
    webp_preset_quality, ImageEncodingParameter, ResolvedRate,
};
use super::{
    output_satisfies_target, AudioChannelSpec, AudioTargetFormat, ConversionRequest,
    ConversionTarget, ImageTargetFormat, QualitySpec, VideoTargetFormat,
};
use crate::FileError;

/// 与 7z 的候选名循环同模式:预留给发行版改名场景。
const FFMPEG_TOOL_CANDIDATES: [&str; 1] = ["ffmpeg"];
const FFPROBE_TOOL_CANDIDATES: [&str; 1] = ["ffprobe"];

/// stderr 最多保留的尾部字节数:错误诊断只需要 ffmpeg 输出的结尾部分。
const CHILD_STDERR_TAIL_LIMIT: usize = 4 * 1024;
/// 图片体积模式二分轮次上限;每轮一次真实编码。
const IMAGE_TARGET_SEARCH_ROUNDS: u8 = 5;

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
            VideoTargetFormat::AviMpeg4 | VideoTargetFormat::Gif => true,
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
            // gif 是内置编码器;jpeg/png 由 image crate 处理,不依赖 ffmpeg。
            ImageTargetFormat::Gif | ImageTargetFormat::Jpeg | ImageTargetFormat::Png => true,
            ImageTargetFormat::Tiff | ImageTargetFormat::Bmp | ImageTargetFormat::Ico => true,
        }
    }

    fn contains(&self, name: &str) -> bool {
        self.encoder_names.contains(name)
    }

    fn any(&self, names: &[&str]) -> bool {
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

async fn probe_encoder_names(ffmpeg_command: &str) -> Option<ConversionEncoderAvailability> {
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
    let encoder_names = listing
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    Some(ConversionEncoderAvailability { encoder_names })
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

/// ffmpeg 引擎统一入口:图片(WebP/AVIF)与视频、音频转换。
pub(super) async fn convert_losing_source(
    request: &ConversionRequest,
    output: &Path,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    controls.checkpoint_now()?;
    let Some(ffmpeg_command) = locate_media_tool(&FFMPEG_TOOL_CANDIDATES).await else {
        return Err(FileError::Convert {
            path: request.source.clone(),
            message: "ffmpeg is required for this conversion but was not found".to_owned(),
        });
    };
    let encoders =
        probe_encoder_names(&ffmpeg_command)
            .await
            .ok_or_else(|| FileError::Convert {
                path: request.source.clone(),
                message: "could not query ffmpeg encoders".to_owned(),
            })?;

    match request.target {
        ConversionTarget::Image(format) => {
            convert_image_format(
                request,
                output,
                format,
                &ffmpeg_command,
                &encoders,
                controls,
            )
            .await
        }
        ConversionTarget::Video(format) => {
            convert_video_format(
                request,
                output,
                format,
                &ffmpeg_command,
                &encoders,
                controls,
            )
            .await
        }
        ConversionTarget::Audio(format) => {
            convert_audio_format(
                request,
                output,
                format,
                &ffmpeg_command,
                &encoders,
                controls,
            )
            .await
        }
    }
}

/// 图片源 → GIF:用视频的调色板滤镜链;动图源(gif/webp)保留动画,静态源输出单帧。
async fn convert_gif_image(
    request: &ConversionRequest,
    output: &Path,
    ffmpeg_command: &str,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let QualitySpec::Preset(preset) = request.quality else {
        return Err(FileError::InvalidInput {
            path: request.source.clone(),
            message: "gif output requires a quality preset".to_owned(),
        });
    };
    let mut args = vec!["-y".to_owned(), "-nostdin".to_owned()];
    args.push("-i".to_owned());
    args.push(request.source.to_string_lossy().into_owned());
    args.extend(gif_stream_args(
        preset,
        request.resize,
        request.fps_override,
    ));
    args.push(output.to_string_lossy().into_owned());

    run_ffmpeg(ffmpeg_command, &args, &request.source, controls).await?;
    output_file_byte_count(output).await
}

async fn convert_image_format(
    request: &ConversionRequest,
    output: &Path,
    format: ImageTargetFormat,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let parameter = match (format, &request.quality) {
        (
            ImageTargetFormat::Webp | ImageTargetFormat::Avif | ImageTargetFormat::Jxl,
            QualitySpec::Lossless,
        ) => {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "lossy image output does not accept lossless quality".to_owned(),
            })
        }
        (ImageTargetFormat::Webp | ImageTargetFormat::Jxl, QualitySpec::Level(level)) => {
            ImageEncodingParameter::EncoderQuality(*level)
        }
        (ImageTargetFormat::Webp | ImageTargetFormat::Jxl, QualitySpec::Preset(preset)) => {
            ImageEncodingParameter::EncoderQuality(webp_preset_quality(*preset))
        }
        (ImageTargetFormat::Webp | ImageTargetFormat::Jxl, QualitySpec::TargetBytes(_)) => {
            return encode_image_towards_target(
                request,
                output,
                format,
                ImageEncodingParameter::EncoderQuality(100),
                ffmpeg_command,
                encoders,
                controls,
            )
            .await;
        }
        (ImageTargetFormat::Avif, QualitySpec::Level(level)) => {
            ImageEncodingParameter::AvifCrf(quality_level_to_avif_crf(*level))
        }
        (ImageTargetFormat::Avif, QualitySpec::Preset(preset)) => {
            ImageEncodingParameter::AvifCrf(avif_preset_crf(*preset))
        }
        (ImageTargetFormat::Avif, QualitySpec::TargetBytes(_)) => {
            return encode_image_towards_target(
                request,
                output,
                format,
                ImageEncodingParameter::AvifCrf(0),
                ffmpeg_command,
                encoders,
                controls,
            )
            .await;
        }
        // GIF 只有调色板档位,单独走动画编码流程。
        (ImageTargetFormat::Gif, QualitySpec::Preset(_)) => {
            return convert_gif_image(request, output, ffmpeg_command, controls).await;
        }
        (ImageTargetFormat::Gif, _) => {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "gif output only accepts a quality preset".to_owned(),
            });
        }
        // 其余组合(JPEG/PNG/TIFF/BMP/ICO 的一切质量、无损目标)都归 image crate 引擎。
        (
            ImageTargetFormat::Jpeg
            | ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico,
            _,
        ) => {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "this target is handled by the image crate engine".to_owned(),
            })
        }
    };

    run_image_encode(
        request,
        output,
        format,
        parameter,
        ffmpeg_command,
        encoders,
        controls,
    )
    .await
}

/// 体积模式:在编码参数域内二分,寻找不超目标的最大质量;压不到时以极限参数交付。
/// WebP 的参数体积单调递增(satisfies 在高端成立),AVIF 的 crf 相反,各自处理方向。
async fn encode_image_towards_target(
    request: &ConversionRequest,
    output: &Path,
    format: ImageTargetFormat,
    upper_bound_parameter: ImageEncodingParameter,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let QualitySpec::TargetBytes(target_bytes) = request.quality else {
        return Err(FileError::InvalidInput {
            path: request.source.clone(),
            message: "target search requires a target size".to_owned(),
        });
    };

    // 快路径:最优参数已达标则无需二分。
    controls.checkpoint_now()?;
    let top_bytes = run_image_encode_to_probe(
        request,
        output,
        format,
        upper_bound_parameter,
        ffmpeg_command,
        encoders,
        controls,
    )
    .await?;
    if output_satisfies_target(top_bytes, target_bytes) {
        promote_probe_output(output);
        return Ok(top_bytes);
    }

    let (mut low, mut high) = upper_bound_parameter.search_bounds();
    // 上界已验证不满足,域收缩到上界以下。
    high = high.saturating_sub(1);
    let mut best_parameter: Option<ImageEncodingParameter> = None;
    for _ in 0..IMAGE_TARGET_SEARCH_ROUNDS {
        controls.checkpoint_now()?;
        if low > high {
            break;
        }
        let mid = low + (high - low) / 2;
        let parameter = upper_bound_parameter.with_scalar(mid);
        let bytes = run_image_encode_to_probe(
            request,
            output,
            format,
            parameter,
            ffmpeg_command,
            encoders,
            controls,
        )
        .await?;
        let satisfies = output_satisfies_target(bytes, target_bytes);
        // quality 型参数体积越大:满足后向上找;crf 型相反,满足后向下找。
        let quality_direction = parameter.is_quality_direction();
        if satisfies {
            promote_probe_output(output);
            best_parameter = Some(parameter);
        }
        if quality_direction {
            if satisfies {
                low = mid + 1;
            } else {
                high = mid.saturating_sub(1);
            }
        } else if satisfies {
            high = mid.saturating_sub(1);
        } else {
            low = mid + 1;
        }
    }

    let final_parameter = best_parameter.unwrap_or(match upper_bound_parameter {
        // 全域不满足:交付极限质量(quality 型最低 0,crf 型最高 63)。
        ImageEncodingParameter::EncoderQuality(_) => ImageEncodingParameter::EncoderQuality(0),
        ImageEncodingParameter::AvifCrf(_) => ImageEncodingParameter::AvifCrf(63),
    });
    run_image_encode(
        request,
        output,
        format,
        final_parameter,
        ffmpeg_command,
        encoders,
        controls,
    )
    .await
}

/// 探测编码写到同目录 probe 临时文件;满足目标后由 promote 覆盖最终输出,
/// 保证 output 任意时刻都是完整可用的编码结果。
async fn run_image_encode_to_probe(
    request: &ConversionRequest,
    output: &Path,
    format: ImageTargetFormat,
    parameter: ImageEncodingParameter,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let probe_output = probe_output_path(output);
    let _ = tokio::fs::remove_file(&probe_output).await;
    let byte_count = run_image_encode(
        request,
        &probe_output,
        format,
        parameter,
        ffmpeg_command,
        encoders,
        controls,
    )
    .await;
    if byte_count.is_err() {
        let _ = tokio::fs::remove_file(&probe_output).await;
    }
    byte_count
}

fn probe_output_path(output: &Path) -> PathBuf {
    // probe 标记插在扩展名之前,保留 ffmpeg 依赖的格式后缀。
    let mut name = output
        .file_stem()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(".convert-probe");
    if let Some(extension) = output.extension() {
        name.push(".");
        name.push(extension);
    }
    output.with_file_name(name)
}

fn promote_probe_output(output: &Path) {
    let probe_output = probe_output_path(output);
    if std::fs::rename(&probe_output, output).is_err() {
        // 提升失败时由最终一次编码兜底,清理探测文件即可。
        let _ = std::fs::remove_file(&probe_output);
    }
}

async fn run_image_encode(
    request: &ConversionRequest,
    output: &Path,
    format: ImageTargetFormat,
    parameter: ImageEncodingParameter,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let mut args = vec!["-y".to_owned(), "-nostdin".to_owned(), "-i".to_owned()];
    args.push(request.source.to_string_lossy().into_owned());
    args.extend(scale_filter_args(request.resize));
    match format {
        ImageTargetFormat::Webp => {
            let encoder = if encoders.contains("libwebp_anim") {
                "libwebp_anim"
            } else {
                "libwebp"
            };
            let ImageEncodingParameter::EncoderQuality(quality) = parameter else {
                return Err(FileError::InvalidInput {
                    path: request.source.clone(),
                    message: "webp encoding requires a quality value".to_owned(),
                });
            };
            args.push("-c:v".to_owned());
            args.push(encoder.to_owned());
            args.push("-lossless".to_owned());
            args.push("0".to_owned());
            args.push("-quality".to_owned());
            args.push(quality.to_string());
        }
        ImageTargetFormat::Jxl => {
            let ImageEncodingParameter::EncoderQuality(quality) = parameter else {
                return Err(FileError::InvalidInput {
                    path: request.source.clone(),
                    message: "jxl encoding requires a quality value".to_owned(),
                });
            };
            args.push("-c:v".to_owned());
            args.push("libjxl".to_owned());
            args.push("-q:v".to_owned());
            args.push(quality.to_string());
        }
        ImageTargetFormat::Avif => {
            let encoder_name = if encoders.contains("libsvtav1") {
                "libsvtav1"
            } else {
                "libaom-av1"
            };
            if !encoders.any(&[encoder_name]) {
                return Err(FileError::Convert {
                    path: request.source.clone(),
                    message: "ffmpeg has no AV1 encoder for avif output".to_owned(),
                });
            }
            let ImageEncodingParameter::AvifCrf(crf) = parameter else {
                return Err(FileError::InvalidInput {
                    path: request.source.clone(),
                    message: "avif encoding requires a crf value".to_owned(),
                });
            };
            args.push("-frames:v".to_owned());
            args.push("1".to_owned());
            args.push("-c:v".to_owned());
            args.push(encoder_name.to_owned());
            args.push("-crf".to_owned());
            args.push(crf.to_string());
            args.push("-pix_fmt".to_owned());
            args.push("yuv420p".to_owned());
        }
        ImageTargetFormat::Gif
        | ImageTargetFormat::Jpeg
        | ImageTargetFormat::Png
        | ImageTargetFormat::Tiff
        | ImageTargetFormat::Bmp
        | ImageTargetFormat::Ico => {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "this target is not a tunable ffmpeg image encode".to_owned(),
            });
        }
    }
    args.push(output.to_string_lossy().into_owned());

    run_ffmpeg(ffmpeg_command, &args, &request.source, controls).await?;
    output_file_byte_count(output).await
}

async fn convert_video_format(
    request: &ConversionRequest,
    output: &Path,
    format: VideoTargetFormat,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    if !encoders.supports_video(format) {
        return Err(FileError::Convert {
            path: request.source.clone(),
            message: format!("ffmpeg lacks the encoder required for {format:?} output"),
        });
    }
    let duration = if matches!(request.quality, QualitySpec::TargetBytes(_)) {
        Some(
            media_duration(&request.source)
                .await
                .ok_or_else(|| FileError::Convert {
                    path: request.source.clone(),
                    message: "could not read media duration for target size mode".to_owned(),
                })?,
        )
    } else {
        None
    };
    let rate = resolve_targeted_rate(request, duration)?;

    let mut args = vec!["-y".to_owned(), "-nostdin".to_owned()];
    args.push("-i".to_owned());
    args.push(request.source.to_string_lossy().into_owned());
    if format == VideoTargetFormat::Gif {
        let QualitySpec::Preset(preset) = request.quality else {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "gif output requires a quality preset".to_owned(),
            });
        };
        args.extend(gif_stream_args(
            preset,
            request.resize,
            request.fps_override,
        ));
    } else {
        args.extend(video_stream_args(format, &request.quality, rate));
        args.extend(scale_filter_args(request.resize));
        if let Some(fps) = request.fps_override {
            args.push("-r".to_owned());
            args.push(fps.to_string());
        }
    }
    args.push(output.to_string_lossy().into_owned());

    run_ffmpeg(ffmpeg_command, &args, &request.source, controls).await?;
    output_file_byte_count(output).await
}

async fn convert_audio_format(
    request: &ConversionRequest,
    output: &Path,
    format: AudioTargetFormat,
    ffmpeg_command: &str,
    encoders: &ConversionEncoderAvailability,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    if !encoders.supports_audio(format) {
        return Err(FileError::Convert {
            path: request.source.clone(),
            message: format!("ffmpeg lacks the encoder required for {format:?} output"),
        });
    }
    let duration = if matches!(request.quality, QualitySpec::TargetBytes(_)) {
        Some(
            media_duration(&request.source)
                .await
                .ok_or_else(|| FileError::Convert {
                    path: request.source.clone(),
                    message: "could not read media duration for target size mode".to_owned(),
                })?,
        )
    } else {
        None
    };
    let rate = resolve_targeted_rate(request, duration)?;

    let mut args = vec!["-y".to_owned(), "-nostdin".to_owned()];
    args.push("-i".to_owned());
    args.push(request.source.to_string_lossy().into_owned());
    args.push("-vn".to_owned());
    match (format, rate) {
        (
            AudioTargetFormat::Mp3
            | AudioTargetFormat::M4a
            | AudioTargetFormat::Opus
            | AudioTargetFormat::Ogg,
            Some(ResolvedRate::Audio { bps }),
        ) => {
            args.push("-c:a".to_owned());
            args.push(audio_encoder_name(format).to_owned());
            args.push("-b:a".to_owned());
            args.push(format!("{}k", bps / 1000));
        }
        (AudioTargetFormat::Flac | AudioTargetFormat::Wav, _) => {
            args.push("-c:a".to_owned());
            args.push(audio_encoder_name(format).to_owned());
        }
        (format, None) => {
            let QualitySpec::Preset(preset) = request.quality else {
                return Err(FileError::InvalidInput {
                    path: request.source.clone(),
                    message: "lossy audio output requires a quality preset or target size"
                        .to_owned(),
                });
            };
            args.push("-c:a".to_owned());
            args.push(audio_encoder_name(format).to_owned());
            args.extend(quality_mode_audio_args(format, preset));
        }
        (
            AudioTargetFormat::Mp3
            | AudioTargetFormat::M4a
            | AudioTargetFormat::Opus
            | AudioTargetFormat::Ogg,
            Some(ResolvedRate::Video { .. }),
        ) => {
            return Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "audio output cannot resolve a video rate".to_owned(),
            })
        }
    }
    if request.audio_channels == AudioChannelSpec::Mono {
        args.push("-ac".to_owned());
        args.push("1".to_owned());
    }
    args.push(output.to_string_lossy().into_owned());

    run_ffmpeg(ffmpeg_command, &args, &request.source, controls).await?;
    output_file_byte_count(output).await
}

async fn run_ffmpeg(
    ffmpeg_command: &str,
    args: &[String],
    error_path: &Path,
    controls: &FileOperationControls,
) -> Result<(), FileError> {
    let mut child = Command::new(ffmpeg_command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| FileError::Convert {
            path: error_path.to_path_buf(),
            message: format!("could not start ffmpeg: {error}"),
        })?;

    // stderr 必须持续排水直到 EOF,否则子进程可能因管道写满而阻塞。
    let mut stderr_tail = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(drain_stderr_tail(stderr)));
    let cancel_token = controls.cancellation_token();

    let wait_result;
    tokio::select! {
        _ = cancel_token.cancelled() => {
            // kill_on_drop 兜底,显式 kill 让 ffmpeg 立即退出。
            let _ = child.kill().await;
            wait_result = Err(FileError::Cancelled);
        }
        status = child.wait() => {
            wait_result = match status {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => {
                    let tail = take_stderr_tail(&mut stderr_tail).await;
                    Err(FileError::Convert {
                        path: error_path.to_path_buf(),
                        message: format!("ffmpeg exited with {status}: {}", tail.trim()),
                    })
                }
                Err(error) => Err(FileError::Convert {
                    path: error_path.to_path_buf(),
                    message: format!("could not wait for ffmpeg: {error}"),
                }),
            };
        }
    }
    wait_result
}

async fn take_stderr_tail(stderr_tail: &mut Option<tokio::task::JoinHandle<String>>) -> String {
    match stderr_tail.take() {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    }
}

/// 有界 stderr 排水:尾部保留量之外继续丢弃,直到 EOF,避免管道背压死锁。
async fn drain_stderr_tail(
    mut stderr: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> String {
    let mut tail = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
                if tail.len() > CHILD_STDERR_TAIL_LIMIT {
                    let keep_from = tail.len() - CHILD_STDERR_TAIL_LIMIT;
                    tail = tail.split_off(keep_from);
                }
            }
            Err(_) => break,
        }
    }
    tail
}

async fn output_file_byte_count(output: &Path) -> Result<u64, FileError> {
    let metadata = tokio::fs::metadata(output)
        .await
        .map_err(|error| FileError::Convert {
            path: output.to_path_buf(),
            message: format!("converted output is missing: {error}"),
        })?;
    Ok(metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_availability_reports_format_support() {
        let availability = ConversionEncoderAvailability {
            encoder_names: [
                "libx264",
                "libmp3lame",
                "flac",
                "libwebp",
                "libsvtav1",
                "libjxl",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>(),
        };

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
        let without_jxl = ConversionEncoderAvailability {
            encoder_names: ["libx264"].into_iter().map(str::to_owned).collect(),
        };
        assert!(!without_jxl.supports_lossy_image(ImageTargetFormat::Jxl));
    }

    #[test]
    fn unavailable_availability_is_empty() {
        assert!(ConversionEncoderAvailability::unavailable().is_empty());
    }
}
