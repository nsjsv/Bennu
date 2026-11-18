use super::*;

fn source_path() -> PathBuf {
    PathBuf::from("/tmp/convert-tests/photo.png")
}

fn base_request(target: ConversionTarget, quality: QualitySpec) -> ConversionRequest {
    ConversionRequest {
        source: source_path(),
        target,
        quality,
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: AudioChannelSpec::Keep,
        edits: MediaEdits::default(),
        photo_metadata: PhotoMetadata::Keep,
        output: OutputLocation::BesideSource,
    }
}

#[test]
fn jpeg_accepts_preset_and_target_size_only() {
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Jpeg),
        QualitySpec::Preset(ConversionQualityPreset::Medium),
    ))
    .is_ok());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Jpeg),
        QualitySpec::TargetBytes(500 * 1024),
    ))
    .is_ok());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Jpeg),
        QualitySpec::Lossless,
    ))
    .is_err());
}

#[test]
fn png_rejects_quality_levels() {
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Png),
        QualitySpec::Lossless,
    ))
    .is_ok());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Png),
        QualitySpec::Preset(ConversionQualityPreset::Medium),
    ))
    .is_err());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Png),
        QualitySpec::TargetBytes(1024),
    ))
    .is_err());
}

#[test]
fn svg_sources_are_rejected() {
    let mut request = base_request(
        ConversionTarget::Image(ImageTargetFormat::Png),
        QualitySpec::Lossless,
    );
    request.source = PathBuf::from("/tmp/convert-tests/logo.svg");
    assert!(validate_conversion_request(&request).is_err());
}

#[test]
fn lossless_audio_only_accepts_lossless() {
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Audio(AudioTargetFormat::Flac),
        QualitySpec::Lossless,
    ))
    .is_ok());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Audio(AudioTargetFormat::Flac),
        QualitySpec::TargetBytes(1024),
    ))
    .is_err());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Audio(AudioTargetFormat::Flac),
        QualitySpec::Preset(ConversionQualityPreset::High),
    ))
    .is_err());
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Audio(AudioTargetFormat::Mp3),
        QualitySpec::Lossless,
    ))
    .is_err());
}

#[test]
fn zero_target_size_is_rejected() {
    assert!(validate_conversion_request(&base_request(
        ConversionTarget::Image(ImageTargetFormat::Jpeg),
        QualitySpec::TargetBytes(0),
    ))
    .is_err());
}

#[test]
fn resize_bounds_are_validated() {
    let mut request = base_request(
        ConversionTarget::Image(ImageTargetFormat::Jpeg),
        QualitySpec::Preset(ConversionQualityPreset::Medium),
    );
    request.resize = ResizeSpec::Percent(0);
    assert!(validate_conversion_request(&request).is_err());
    request.resize = ResizeSpec::Percent(100);
    assert!(validate_conversion_request(&request).is_ok());
    request.resize = ResizeSpec::LongEdge(0);
    assert!(validate_conversion_request(&request).is_err());
    request.resize = ResizeSpec::LongEdge(1920);
    assert!(validate_conversion_request(&request).is_ok());
}

#[test]
fn video_rejects_zero_fps_and_lossless() {
    let mut request = base_request(
        ConversionTarget::Video(VideoTargetFormat::Mp4H264),
        QualitySpec::Preset(ConversionQualityPreset::Medium),
    );
    request.fps_override = Some(0);
    assert!(validate_conversion_request(&request).is_err());
    request.fps_override = Some(30);
    assert!(validate_conversion_request(&request).is_ok());
    request.quality = QualitySpec::Lossless;
    assert!(validate_conversion_request(&request).is_err());
}

#[test]
fn gif_video_only_accepts_presets() {
    let mut request = base_request(
        ConversionTarget::Video(VideoTargetFormat::Gif),
        QualitySpec::Preset(ConversionQualityPreset::Medium),
    );
    assert!(validate_conversion_request(&request).is_ok());
    request.quality = QualitySpec::TargetBytes(1024);
    assert!(validate_conversion_request(&request).is_err());
}

#[test]
fn new_lossless_targets_only_accept_lossless() {
    for format in [
        ImageTargetFormat::Tiff,
        ImageTargetFormat::Bmp,
        ImageTargetFormat::Ico,
    ] {
        let mut request = base_request(ConversionTarget::Image(format), QualitySpec::Lossless);
        assert!(validate_conversion_request(&request).is_ok(), "{format:?}");
        request.quality = QualitySpec::Preset(ConversionQualityPreset::Medium);
        assert!(validate_conversion_request(&request).is_err(), "{format:?}");
        request.quality = QualitySpec::TargetBytes(4096);
        assert!(validate_conversion_request(&request).is_err(), "{format:?}");
    }
}

#[test]
fn wav_audio_only_accepts_lossless() {
    let mut request = base_request(
        ConversionTarget::Audio(AudioTargetFormat::Wav),
        QualitySpec::Lossless,
    );
    assert!(validate_conversion_request(&request).is_ok());
    request.quality = QualitySpec::Preset(ConversionQualityPreset::High);
    assert!(validate_conversion_request(&request).is_err());
    request.quality = QualitySpec::TargetBytes(1024);
    assert!(validate_conversion_request(&request).is_err());
}

#[test]
fn output_reaches_target_within_tolerance() {
    assert!(output_reaches_target(500 * 1024, 500 * 1024));
    assert!(output_reaches_target(540 * 1024, 500 * 1024));
    assert!(!output_reaches_target(560 * 1024, 500 * 1024));
    assert!(output_satisfies_target(499_999, 500_000));
}

#[tokio::test]
async fn reserve_output_path_avoids_existing_names_atomically() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("photo.png");
    std::fs::write(&source, b"stub").expect("write source");

    let beside = OutputLocation::BesideSource;
    let jpeg = ConversionTarget::Image(ImageTargetFormat::Jpeg);
    let first = reserve_output_path(&source, &beside, jpeg)
        .await
        .expect("reserve first");
    assert_eq!(first, directory.path().join("photo.jpg"));

    let second = reserve_output_path(&source, &beside, jpeg)
        .await
        .expect("reserve second");
    assert_eq!(second, directory.path().join("photo_1.jpg"));

    std::fs::remove_file(&first).expect("drop first placeholder");
    let third = reserve_output_path(
        &source,
        &beside,
        ConversionTarget::Image(ImageTargetFormat::Webp),
    )
    .await
    .expect("reserve third");
    assert_eq!(third, directory.path().join("photo.webp"));
}

#[tokio::test]
async fn reserve_output_path_creates_the_chosen_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("photo.png");
    std::fs::write(&source, b"stub").expect("write source");
    let chosen = directory.path().join("converted/2026");
    let location = OutputLocation::Directory(chosen.clone());
    let jpeg = ConversionTarget::Image(ImageTargetFormat::Jpeg);

    let first = reserve_output_path(&source, &location, jpeg)
        .await
        .expect("reserve first");
    assert_eq!(first, chosen.join("photo.jpg"));
    let second = reserve_output_path(&source, &location, jpeg)
        .await
        .expect("reserve second");
    assert_eq!(second, chosen.join("photo_1.jpg"));
}

#[tokio::test]
async fn convert_failure_cleans_up_placeholder() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("clip.mp4");
    std::fs::write(&source, b"not a real video").expect("write source");

    let request = ConversionRequest {
        source: source.clone(),
        target: ConversionTarget::Audio(AudioTargetFormat::Mp3),
        quality: QualitySpec::Preset(ConversionQualityPreset::Medium),
        resize: ResizeSpec::Keep,
        fps_override: None,
        audio_channels: AudioChannelSpec::Keep,
        edits: MediaEdits::default(),
        photo_metadata: PhotoMetadata::Keep,
        output: OutputLocation::BesideSource,
    };
    let controls = FileOperationControls::running(tokio_util::sync::CancellationToken::new());
    let result = convert_file_with_controls(request, &controls).await;
    // ffmpeg 缺失或输入非法都应报错,且目录里不残留占位文件。
    assert!(result.is_err(), "invalid input must fail");
    let leftovers = std::fs::read_dir(directory.path())
        .expect("read dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path() != source)
        .count();
    assert_eq!(leftovers, 0, "no leftover output files");
}
