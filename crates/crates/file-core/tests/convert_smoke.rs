// 真实 ffmpeg 冒烟:仅当系统存在 ffmpeg 时运行,覆盖体积模式与视频转码的端到端路径。
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use file_core::{
    convert_file_with_controls, ConversionRequest, ConversionTarget, FileOperationControls,
    ImageTargetFormat, QualitySpec, ResizeSpec, VideoTargetFormat,
};
use tokio_util::sync::CancellationToken;

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn controls() -> FileOperationControls {
    FileOperationControls::running(CancellationToken::new())
}

fn write_test_png(directory: &Path, name: &str, size: u32) -> PathBuf {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(size, size, |x, y| {
        image::Rgb([
            (x * 11 % 256) as u8,
            (y * 17 % 256) as u8,
            ((x * y) % 256) as u8,
        ])
    }));
    let path = directory.join(name);
    image
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write png");
    path
}

fn write_test_video(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=15",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "generate test video");
    path
}

#[tokio::test]
async fn png_to_webp_target_size_converges() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_png(directory.path(), "photo.png", 512);

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Image(ImageTargetFormat::Webp),
        quality: QualitySpec::TargetBytes(20 * 1024),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("convert succeeds");
    assert!(
        converted.byte_count <= 22 * 1024,
        "webp output {} exceeds target",
        converted.byte_count
    );
    assert!(converted.reached_target);
}

#[tokio::test]
async fn mp4_quality_and_resize_conversion() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_video(directory.path(), "clip.mp4");

    let request = ConversionRequest {
        source: source.clone(),
        target: ConversionTarget::Video(VideoTargetFormat::WebmVp9),
        quality: QualitySpec::Preset(file_core::ConversionQualityPreset::Medium),
        resize: ResizeSpec::Width(160),
        fps_override: Some(10),
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("convert succeeds");
    assert!(converted.byte_count > 0);
    assert_eq!(converted.output, directory.path().join("clip.webm"));
}

#[tokio::test]
async fn mp4_target_size_estimates_bitrate() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_video(directory.path(), "clip.mp4");

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Video(VideoTargetFormat::Mp4H264),
        quality: QualitySpec::TargetBytes(200 * 1024),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("convert succeeds");
    assert!(
        converted.byte_count <= 220 * 1024,
        "mp4 output {} exceeds target tolerance",
        converted.byte_count
    );
}

#[tokio::test]
async fn duration_probe_reads_test_video() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_video(directory.path(), "clip.mp4");
    let duration = file_core::media_duration(&source).await.expect("duration");
    assert!(
        (duration - Duration::from_secs_f64(2.0))
            .as_secs_f64()
            .abs()
            < 0.2
    );
}

#[tokio::test]
async fn png_to_gif_palette_conversion() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_png(directory.path(), "frame.png", 200);

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Image(ImageTargetFormat::Gif),
        quality: file_core::QualitySpec::Preset(file_core::ConversionQualityPreset::Medium),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("gif convert succeeds");
    assert!(converted.byte_count > 0);
    assert_eq!(converted.output, directory.path().join("frame.gif"));
}

#[tokio::test]
async fn mp4_to_gif_uses_palette_chain() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_video(directory.path(), "clip.mp4");

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Video(VideoTargetFormat::Gif),
        quality: file_core::QualitySpec::Preset(file_core::ConversionQualityPreset::Medium),
        resize: ResizeSpec::Width(320),
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("video gif convert succeeds");
    assert!(converted.byte_count > 0);
}

#[tokio::test]
async fn mp4_to_avi_mpeg4_conversion() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = write_test_video(directory.path(), "clip.mp4");

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Video(VideoTargetFormat::AviMpeg4),
        quality: file_core::QualitySpec::Preset(file_core::ConversionQualityPreset::Medium),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("avi convert succeeds");
    assert!(converted.byte_count > 0);
    assert_eq!(converted.output, directory.path().join("clip.avi"));
}

#[tokio::test]
async fn wav_to_m4a_conversion() {
    if !ffmpeg_available() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("tone.wav");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
        ])
        .arg(&source)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "generate test wav");

    let request = ConversionRequest {
        source,
        target: ConversionTarget::Audio(file_core::AudioTargetFormat::M4a),
        quality: file_core::QualitySpec::Preset(file_core::ConversionQualityPreset::High),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: file_core::AudioChannelSpec::Keep,
    };
    let converted = convert_file_with_controls(request, &controls())
        .await
        .expect("m4a convert succeeds");
    assert!(converted.byte_count > 0);
}
