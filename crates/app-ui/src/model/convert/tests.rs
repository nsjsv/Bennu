use super::*;

/// 常见发行版 ffmpeg 的编码器集合。
fn full_encoders() -> ConversionEncoderAvailability {
    ConversionEncoderAvailability::from_encoder_names([
        "libx264",
        "libx265",
        "libsvtav1",
        "libvpx-vp9",
        "libmp3lame",
        "aac",
        "libopus",
        "libvorbis",
        "flac",
        "pcm_s16le",
        "libwebp",
        "libjxl",
        "mpeg4",
        "gif",
    ])
}

/// 精简编译的 ffmpeg：没有 AV1、H.265、Opus、WebP。
fn bare_encoders() -> ConversionEncoderAvailability {
    ConversionEncoderAvailability::from_encoder_names(["libx264", "libmp3lame", "aac", "gif"])
}

fn source(path: &str, kind: ConvertibleMediaKind, bytes: u64) -> ConvertSource {
    ConvertSource {
        path: PathBuf::from(path),
        kind,
        bytes,
    }
}

fn mixed_state(encoders: ConversionEncoderAvailability) -> ConvertState {
    let mut state = ConvertState::new(
        vec![
            source("/p/a.png", ConvertibleMediaKind::Image, 100),
            source("/p/clip.mkv", ConvertibleMediaKind::Video, 1000),
            source("/p/song.flac", ConvertibleMediaKind::Audio, 500),
        ],
        &ConvertDefaults::default(),
    )
    .expect("convertible");
    state.accept_encoder_probe(encoders);
    state
}

#[test]
fn quality_controls_follow_the_target_format() {
    assert_eq!(
        image_quality_control(ImageTargetFormat::Png),
        QualityControl::Lossless
    );
    assert_eq!(
        image_quality_control(ImageTargetFormat::Gif),
        QualityControl::PresetOnly
    );
    assert_eq!(
        image_quality_control(ImageTargetFormat::Jpeg),
        QualityControl::PresetOrSize
    );
    assert_eq!(
        video_quality_control(VideoTargetFormat::Gif),
        QualityControl::PresetOnly
    );
    assert_eq!(
        video_quality_control(VideoTargetFormat::WebmVp9),
        QualityControl::PresetOrSize
    );
    assert_eq!(
        audio_quality_control(AudioTargetFormat::Wav),
        QualityControl::Lossless
    );
    assert_eq!(
        audio_quality_control(AudioTargetFormat::Opus),
        QualityControl::PresetOrSize
    );
}

#[test]
fn quality_spec_keeps_the_preset_for_gif_while_by_size_is_chosen() {
    let mut quality = QualitySelection::default();
    quality.select(QualityChoice::Preset(ConversionQualityPreset::High));
    quality.select(QualityChoice::TargetSize);
    assert_eq!(quality.choice(), QualityChoice::TargetSize);
    assert_eq!(
        quality.spec(QualityControl::PresetOrSize),
        Ok(QualitySpec::TargetBytes(1024 * 1024))
    );
    assert_eq!(
        quality.spec(QualityControl::PresetOnly),
        Ok(QualitySpec::Preset(ConversionQualityPreset::High))
    );
    assert_eq!(
        quality.spec(QualityControl::Lossless),
        Ok(QualitySpec::Lossless)
    );

    quality.target_size_text = "abc".to_owned();
    assert_eq!(
        quality.spec(QualityControl::PresetOrSize),
        Err(ConvertInputError::TargetSize)
    );
    // 无损和只认档位的格式不读大小文字。
    assert!(quality.spec(QualityControl::PresetOnly).is_ok());
}

#[test]
fn parses_target_size_with_units() {
    assert_eq!(parse_target_size_text("500KB"), Ok(500 * 1024));
    assert_eq!(parse_target_size_text("2 MB"), Ok(2 * 1024 * 1024));
    assert_eq!(parse_target_size_text("100"), Ok(100));
    assert_eq!(
        parse_target_size_text(DEFAULT_TARGET_SIZE_TEXT),
        Ok(1024 * 1024)
    );
    for invalid in ["", "0KB", "abc"] {
        assert_eq!(
            parse_target_size_text(invalid),
            Err(ConvertInputError::TargetSize)
        );
    }
}

#[test]
fn resize_selection_maps_to_specs() {
    let mut resize = ResizeSelection::default();
    assert_eq!(resize.spec(), Ok(ResizeSpec::Keep));
    resize.choice = ResizeChoice::Percent(50);
    assert_eq!(resize.spec(), Ok(ResizeSpec::Percent(50)));
    resize.choice = ResizeChoice::LongEdge;
    assert_eq!(resize.spec(), Ok(ResizeSpec::LongEdge(1920)));
    for invalid in ["0", "wide", ""] {
        resize.long_edge_text = invalid.to_owned();
        assert_eq!(resize.spec(), Err(ConvertInputError::LongEdge));
    }
}

#[test]
fn sources_are_grouped_with_size_and_most_common_formats_first() {
    let state = ConvertState::new(
        vec![
            source("/p/a.png", ConvertibleMediaKind::Image, 10),
            source("/p/b.jpg", ConvertibleMediaKind::Image, 20),
            source("/p/c.JPG", ConvertibleMediaKind::Image, 30),
            source("/p/d.webp", ConvertibleMediaKind::Image, 40),
            source("/p/e.heic", ConvertibleMediaKind::Image, 50),
            source("/p/song.mp3", ConvertibleMediaKind::Audio, 7),
        ],
        &ConvertDefaults::default(),
    )
    .expect("convertible");
    let images = &state.image.as_ref().expect("images").sources;
    assert_eq!(images.paths.len(), 5);
    assert_eq!(images.total_bytes, 150);
    assert_eq!(images.format_summary(", "), "JPG, HEIC, PNG…");
    assert!(state.video.is_none());
    assert_eq!(state.source_count(), 6);
    assert_eq!(
        state
            .audio
            .as_ref()
            .expect("audio")
            .sources
            .format_summary(", "),
        "MP3"
    );
}

#[test]
fn nothing_convertible_opens_no_dialog() {
    assert!(ConvertState::new(Vec::new(), &ConvertDefaults::default()).is_none());
}

#[test]
fn requests_wait_for_the_encoder_probe() {
    let state = ConvertState::new(
        vec![source("/p/a.png", ConvertibleMediaKind::Image, 1)],
        &ConvertDefaults::default(),
    )
    .expect("convertible");
    assert_eq!(state.requests(), Err(ConvertInputError::MissingEncoder));
}

#[test]
fn requests_carry_each_section_settings_and_the_output_location() {
    let mut state = mixed_state(full_encoders());
    state.apply(ConvertEdit::Image(SectionEdit::Options(ImageEdit::Resize(
        ResizeChoice::LongEdge,
    ))));
    state.apply(ConvertEdit::Video(SectionEdit::Quality(
        QualityChoice::TargetSize,
    )));
    state.apply(ConvertEdit::Audio(SectionEdit::Options(
        AudioEdit::Channels(AudioChannelSpec::Mono),
    )));
    state.apply(ConvertEdit::OutputDirectory(" /tmp/out ".to_owned()));

    assert_eq!(state.input_error(), None);
    let requests = state.requests().expect("valid");
    assert_eq!(requests.len(), 3);
    let output = OutputLocation::Directory(PathBuf::from("/tmp/out"));
    assert!(requests.iter().all(|request| request.output == output));
    assert_eq!(requests[0].resize, ResizeSpec::LongEdge(1920));
    assert_eq!(
        requests[0].quality,
        QualitySpec::Preset(ConversionQualityPreset::Medium)
    );
    assert_eq!(requests[1].quality, QualitySpec::TargetBytes(1024 * 1024));
    assert_eq!(requests[2].audio_channels, AudioChannelSpec::Mono);
    assert_eq!(requests[2].resize, ResizeSpec::Keep);
}

#[test]
fn output_directory_must_be_a_full_path() {
    let mut state = mixed_state(full_encoders());
    assert_eq!(state.output_location(), Ok(OutputLocation::BesideSource));
    state.apply(ConvertEdit::OutputDirectory("  ".to_owned()));
    assert_eq!(state.output_location(), Ok(OutputLocation::BesideSource));
    state.apply(ConvertEdit::OutputDirectory("out/images".to_owned()));
    assert_eq!(state.requests(), Err(ConvertInputError::OutputDirectory));
    assert_eq!(
        state.input_error(),
        Some(ConvertInputError::OutputDirectory)
    );
}

#[test]
fn missing_ffmpeg_falls_back_to_image_crate_formats_and_blocks_media() {
    let mut defaults = ConvertDefaults::default();
    defaults.image.target_format = ImageTargetFormat::Webp;
    let mut state = ConvertState::new(
        vec![
            source("/p/a.png", ConvertibleMediaKind::Image, 1),
            source("/p/clip.mkv", ConvertibleMediaKind::Video, 1),
        ],
        &defaults,
    )
    .expect("convertible");
    state.accept_encoder_probe(ConversionEncoderAvailability::unavailable());
    assert_eq!(
        state.image.as_ref().expect("image").options.target_format,
        ImageTargetFormat::Jpeg
    );
    assert_eq!(state.requests(), Err(ConvertInputError::MissingEncoder));
}

#[test]
fn scenes_fill_every_section_and_light_up_until_edited() {
    let mut state = mixed_state(full_encoders());
    assert_eq!(state.active_scene(), None);

    state.apply(ConvertEdit::Scene(ConvertScene::ForWeb));
    assert_eq!(state.active_scene(), Some(ConvertScene::ForWeb));
    let image = &state.image.as_ref().expect("image").options;
    assert_eq!(image.target_format, ImageTargetFormat::Webp);
    assert_eq!(image.resize.spec(), Ok(ResizeSpec::LongEdge(1920)));
    let video = &state.video.as_ref().expect("video").options;
    assert_eq!(video.target_format, VideoTargetFormat::Mp4H264);
    assert_eq!(video.resize.spec(), Ok(ResizeSpec::LongEdge(1920)));
    assert_eq!(
        state.audio.as_ref().expect("audio").options.target_format,
        AudioTargetFormat::M4a
    );

    state.apply(ConvertEdit::Audio(SectionEdit::Quality(
        QualityChoice::Preset(ConversionQualityPreset::High),
    )));
    assert_eq!(state.active_scene(), None);

    // 场景之外的选项（帧率）不动，也不影响点亮。
    state.apply(ConvertEdit::Video(SectionEdit::Options(VideoEdit::Fps(
        Some(30),
    ))));
    state.apply(ConvertEdit::Scene(ConvertScene::MostCompatible));
    assert_eq!(state.active_scene(), Some(ConvertScene::MostCompatible));
    let video = &state.video.as_ref().expect("video").options;
    assert_eq!(video.fps_override, Some(30));
    assert_eq!(video.resize.spec(), Ok(ResizeSpec::Keep));
}

#[test]
fn scenes_fall_back_when_encoders_are_missing() {
    let mut state = mixed_state(bare_encoders());
    state.apply(ConvertEdit::Scene(ConvertScene::Smallest));
    assert_eq!(state.active_scene(), Some(ConvertScene::Smallest));
    let image = &state.image.as_ref().expect("image").options;
    assert_eq!(image.target_format, ImageTargetFormat::Jpeg);
    assert_eq!(
        image.quality.choice(),
        QualityChoice::Preset(ConversionQualityPreset::Low)
    );
    let video = &state.video.as_ref().expect("video").options;
    assert_eq!(video.target_format, VideoTargetFormat::Mp4H264);
    assert_eq!(
        video.quality.choice(),
        QualityChoice::Preset(ConversionQualityPreset::Low)
    );
    assert_eq!(
        state.audio.as_ref().expect("audio").options.target_format,
        AudioTargetFormat::Mp3
    );
}

#[test]
fn scenes_wait_for_the_encoder_probe() {
    let mut state = ConvertState::new(
        vec![source("/p/a.png", ConvertibleMediaKind::Image, 1)],
        &ConvertDefaults::default(),
    )
    .expect("convertible");
    state.apply(ConvertEdit::Scene(ConvertScene::Smallest));
    assert_eq!(
        state.image.as_ref().expect("image").options,
        ImageConvertOptions::default()
    );
    assert_eq!(state.active_scene(), None);
}

#[test]
fn remembered_defaults_replace_only_the_sections_present() {
    let mut previous = ConvertDefaults::default();
    previous.video.fps_override = Some(24);
    let mut state = ConvertState::new(
        vec![source("/p/a.png", ConvertibleMediaKind::Image, 1)],
        &previous,
    )
    .expect("convertible");
    state.apply(ConvertEdit::Image(SectionEdit::Options(ImageEdit::Format(
        ImageTargetFormat::Avif,
    ))));
    state.apply(ConvertEdit::Image(SectionEdit::MoreToggled));

    let remembered = state.remembered(&previous);
    assert_eq!(remembered.image.target_format, ImageTargetFormat::Avif);
    assert_eq!(remembered.video, previous.video);
    assert_eq!(remembered.audio, previous.audio);
}
