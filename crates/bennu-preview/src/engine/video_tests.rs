use std::path::PathBuf;

use super::*;
use crate::engine::{
    default_preview_size, PreviewEngine, PreviewEngineConfig, PreviewWindowIdentity,
};
use crate::preview::{VideoSubtitle, VideoTrackMenu};
use crate::preview_config::PreviewFileSizeLimits;
use crate::video_preview::{
    AudioTrack, VideoColor, VideoPrimaries, VideoProbe, VideoStreamFacts, VideoTransfer, YuvMatrix,
};
use crate::video_surface::{picture_bytes, SampleDepth, VideoPicture};

fn engine(surface: PreviewLoadSurface) -> PreviewEngine {
    let mut engine = PreviewEngine::new(
        PreviewEngineConfig {
            directory_expand_levels: 1,
            extension_rules: Default::default(),
            size_limits: PreviewFileSizeLimits::with_default_limits(),
        },
        default_preview_size(PreviewWindowProfile::Regular),
        PreviewWindowIdentity {
            app_id: "bennu-preview-tests".to_owned(),
            icon: None,
        },
    );
    engine.preview_load_surface = surface;
    engine
}

fn details(audio_tracks: usize) -> VideoPreviewDetails {
    VideoPreviewDetails {
        len: 1,
        probe: VideoProbe {
            duration: Some(Duration::from_secs(60)),
            container: None,
            bit_rate: None,
            video: Some(VideoStreamFacts {
                ordinal: 0,
                codec: "h264".to_owned(),
                width: 1280,
                height: 720,
                sample_aspect: (1, 1),
                frame_rate: Some((30, 1)),
                rotation: 0,
                pixel_format: None,
                color: VideoColor {
                    matrix: YuvMatrix::Bt709,
                    transfer: VideoTransfer::Sdr,
                    primaries: VideoPrimaries::Bt709,
                },
                interlaced: false,
                bit_rate: None,
            }),
            audio_tracks: (0..audio_tracks)
                .map(|ordinal| AudioTrack {
                    ordinal,
                    codec: "aac".to_owned(),
                    channels: Some(2),
                    sample_rate: Some(48_000),
                    language: None,
                    title: None,
                    is_default: ordinal == 0,
                })
                .collect(),
            subtitle_tracks: Vec::new(),
        },
        subtitles: Vec::new(),
    }
}

/// 打开一个视频并量好屏幕（独立窗口会直接开播）。
fn opened(surface: PreviewLoadSurface, audio_tracks: usize) -> (PreviewEngine, PathBuf) {
    let path = PathBuf::from("/videos/clip.mkv");
    let mut engine = engine(surface);
    let details = Arc::new(details(audio_tracks));
    engine.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: path.clone(),
        details: Arc::clone(&details),
        frame: None,
    }));
    drop(engine.open_video_preview(&path, &details));
    drop(engine.accept_video_display(&path, Some(Size::new(1920, 1080))));
    (engine, path)
}

fn playback(engine: &PreviewEngine) -> &VideoPreviewPlayback {
    engine.video_preview.as_ref().expect("video playback")
}

fn frame(path: &Path, generation: u64) -> VideoPreviewFrame {
    VideoPreviewFrame {
        path: path.to_path_buf(),
        generation,
        position: Duration::ZERO,
        picture: Arc::new(VideoPicture::new(
            2,
            2,
            SampleDepth::Eight,
            VideoColor {
                matrix: YuvMatrix::Bt709,
                transfer: VideoTransfer::Sdr,
                primaries: VideoPrimaries::Bt709,
            },
            vec![16; picture_bytes(2, 2, SampleDepth::Eight)],
        )),
    }
}

#[test]
fn right_panel_shows_a_still_and_waits_for_play_while_the_window_plays_at_once() {
    let (panel, _) = opened(PreviewLoadSurface::RightDockedPanel, 1);
    assert_eq!(playback(&panel).status, VideoPreviewPlaybackStatus::Ready);
    assert!(panel.active_video_preview_stream().is_none());
    assert_eq!(playback(&panel).position, Duration::ZERO);

    let (window, _) = opened(PreviewLoadSurface::StandaloneWindow, 1);
    assert_eq!(
        playback(&window).status,
        VideoPreviewPlaybackStatus::Playing
    );
    let segment = window.active_video_preview_stream().expect("decoding");
    assert_eq!(segment.start, Duration::ZERO);
    assert!(matches!(playback(&window).sound, VideoSound::Starting));
}

#[test]
fn nothing_decodes_before_the_screen_is_measured() {
    let path = PathBuf::from("/videos/clip.mkv");
    let mut engine = engine(PreviewLoadSurface::StandaloneWindow);
    let details = Arc::new(details(1));
    engine.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: path.clone(),
        details: Arc::clone(&details),
        frame: None,
    }));
    drop(engine.open_video_preview(&path, &details));
    drop(engine.toggle_video_preview_playback());
    assert!(engine.active_video_preview_stream().is_none());
}

#[test]
fn pausing_keeps_the_same_decoding_segment_and_resuming_reuses_it() {
    let (mut engine, _) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    let before = engine.active_video_preview_stream().expect("decoding");

    drop(engine.toggle_video_preview_playback());
    assert_eq!(playback(&engine).status, VideoPreviewPlaybackStatus::Paused);
    assert!(!playback(&engine).clock.is_running());
    let paused = engine
        .active_video_preview_stream()
        .expect("still decoding");
    assert_eq!(
        (paused.generation, paused.start),
        (before.generation, before.start)
    );

    drop(engine.toggle_video_preview_playback());
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );
    assert!(playback(&engine).clock.is_running());
    let resumed = engine.active_video_preview_stream().expect("decoding");
    assert_eq!(resumed.generation, before.generation);
}

#[test]
fn dragging_stops_decoding_and_release_restarts_from_the_exact_spot() {
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);

    drop(engine.seek_video_preview_playback(20.0));
    assert!(engine.active_video_preview_stream().is_none());
    assert_eq!(
        playback(&engine).still_in_flight,
        Some(Duration::from_secs(20))
    );
    // 拖得快：只记最后一个位置，等前一张解完再解。
    drop(engine.seek_video_preview_playback(25.0));
    drop(engine.seek_video_preview_playback(30.0));
    assert_eq!(
        playback(&engine).pending_still,
        Some(Duration::from_secs(30))
    );
    let generation = playback(&engine).generation;
    drop(engine.accept_video_preview_still(VideoPreviewFrame {
        position: Duration::from_secs(20),
        ..frame(&path, generation)
    }));
    assert_eq!(
        playback(&engine).still_in_flight,
        Some(Duration::from_secs(30))
    );

    drop(engine.commit_video_preview_seek());
    let segment = engine.active_video_preview_stream().expect("decoding");
    assert_eq!(segment.start, Duration::from_secs(30));
    // 按下时在播：松手接着播。
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );

    // 暂停着拖：松手停在那一帧（解码那一段开好，第一帧会送来）。
    drop(engine.toggle_video_preview_playback());
    drop(engine.seek_video_preview_playback(10.0));
    drop(engine.commit_video_preview_seek());
    assert_eq!(playback(&engine).status, VideoPreviewPlaybackStatus::Paused);
    let segment = engine.active_video_preview_stream().expect("decoding");
    assert_eq!(segment.start, Duration::from_secs(10));
}

#[test]
fn frames_from_an_earlier_segment_are_dropped() {
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    let stale = playback(&engine).generation;
    drop(engine.seek_video_to(Duration::from_secs(5)));
    drop(engine.accept_video_preview_frame(frame(&path, stale)));
    assert!(matches!(
        &engine.preview,
        Some(PreviewState::Ready(PreviewContent::Video {
            frame: None,
            ..
        }))
    ));
    let current = playback(&engine).generation;
    drop(engine.accept_video_preview_frame(frame(&path, current)));
    assert!(matches!(
        &engine.preview,
        Some(PreviewState::Ready(PreviewContent::Video {
            frame: Some(_),
            ..
        }))
    ));
}

#[tokio::test]
async fn a_single_click_toggles_after_the_double_click_window_and_a_double_click_does_not() {
    let (mut engine, _) = opened(PreviewLoadSurface::StandaloneWindow, 0);

    drop(engine.video_surface_pressed());
    let click = playback(&engine).pending_click.expect("waiting");
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );
    drop(engine.video_click_settled(click));
    assert_eq!(playback(&engine).status, VideoPreviewPlaybackStatus::Paused);

    drop(engine.video_surface_pressed());
    let first = playback(&engine).pending_click.expect("waiting");
    drop(engine.video_surface_pressed());
    assert_eq!(playback(&engine).pending_click, None);
    drop(engine.video_click_settled(first));
    assert_eq!(playback(&engine).status, VideoPreviewPlaybackStatus::Paused);
}

#[test]
fn silent_videos_run_on_the_wall_clock_without_an_audio_error() {
    let (mut engine, _) = opened(PreviewLoadSurface::RightDockedPanel, 0);
    assert!(matches!(playback(&engine).sound, VideoSound::Silent));
    drop(engine.toggle_video_preview_playback());
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );
    assert!(matches!(playback(&engine).sound, VideoSound::Silent));
    assert_eq!(playback(&engine).error, None);
}

#[test]
fn audio_that_fails_to_start_falls_back_to_the_wall_clock() {
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 1);
    let generation = playback(&engine).generation;
    drop(engine.accept_video_preview_audio_started(&path, generation, Err("no device".to_owned())));
    assert!(matches!(playback(&engine).sound, VideoSound::Failed));
    assert!(playback(&engine).error.is_some());
    // 画面照放：再按播放不再起声音。
    drop(engine.toggle_video_preview_playback());
    drop(engine.toggle_video_preview_playback());
    assert!(matches!(playback(&engine).sound, VideoSound::Failed));
}

#[test]
fn finishing_stops_and_play_starts_again_from_the_beginning() {
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    let generation = playback(&engine).generation;
    drop(engine.accept_video_preview_finished(&path, generation));
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Finished
    );
    assert!(engine.active_video_preview_stream().is_none());
    assert_eq!(playback(&engine).position, Duration::from_secs(60));

    drop(engine.toggle_video_preview_playback());
    let segment = engine.active_video_preview_stream().expect("decoding");
    assert_eq!(segment.start, Duration::ZERO);
}

#[test]
fn audio_content_opens_its_own_playback_and_window() {
    // 只有声音的「视频」走视频分派：内容到了才知道是音频，播放和窗口由引擎在这里打开。
    let path = PathBuf::from("/videos/voice.mp4");
    let mut window = engine(PreviewLoadSurface::StandaloneWindow);
    window.preview = Some(PreviewState::Loading(path.clone()));
    let content = PreviewContent::Audio {
        path: path.clone(),
        details: Box::new(crate::preview::AudioPreviewDetails {
            len: 1,
            probe: crate::audio_preview::AudioProbe {
                duration: Some(Duration::from_secs(3)),
                duration_is_estimate: false,
                tags: Default::default(),
                stream: Default::default(),
                embedded_cover_stream: None,
                embedded_lyrics: None,
            },
        }),
        cover: None,
        lyrics: None,
    };
    drop(window.accept_preview(path.clone(), Ok(content.clone()), 1));
    assert_eq!(window.preview_window_profile, PreviewWindowProfile::Audio);
    assert!(window.audio_preview.is_some());

    let mut panel = engine(PreviewLoadSurface::RightDockedPanel);
    panel.preview = Some(PreviewState::Loading(path.clone()));
    drop(panel.accept_preview(path, Ok(content), 1));
    assert_eq!(
        panel.audio_preview.as_ref().map(|playback| playback.status),
        Some(crate::preview::AudioPreviewPlaybackStatus::Ready)
    );
}

fn with_subtitles(engine: &mut PreviewEngine, path: &Path) {
    let Some(PreviewState::Ready(PreviewContent::Video { details, .. })) = &mut engine.preview
    else {
        panic!("video content");
    };
    let details = Arc::make_mut(details);
    details.subtitles = vec![crate::video_preview::SubtitleChoice::External(
        crate::video_preview::ExternalSubtitle {
            path: path.with_extension("srt"),
            language: None,
        },
    )];
}

#[tokio::test]
async fn changing_subtitles_waits_for_preparation_then_restarts_where_it_was() {
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    with_subtitles(&mut engine, &path);
    let before = engine.active_video_preview_stream().expect("decoding");
    assert!(before.subtitle.is_none());

    drop(engine.select_video_subtitle(Some(0)));
    assert!(matches!(
        playback(&engine).subtitle,
        VideoSubtitle::Preparing { choice: 0, .. }
    ));
    // 准备好之前照常放，不重开。
    assert_eq!(
        engine
            .active_video_preview_stream()
            .expect("decoding")
            .generation,
        before.generation
    );

    let burn = Arc::new(crate::video_preview::SubtitleBurn::Image { ordinal: 0 });
    drop(engine.accept_video_subtitle(&path, 0, Ok(burn)));
    let with_subtitle = engine.active_video_preview_stream().expect("decoding");
    assert!(with_subtitle.subtitle.is_some());
    assert!(with_subtitle.generation > before.generation);
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );

    // 关掉：再从当前位置重开一段，没有字幕。
    drop(engine.select_video_subtitle(None));
    let off = engine.active_video_preview_stream().expect("decoding");
    assert!(off.subtitle.is_none());
    assert!(off.generation > with_subtitle.generation);

    // 迟到的准备结果（已经换过选择）不理。
    drop(engine.accept_video_subtitle(
        &path,
        0,
        Ok(Arc::new(crate::video_preview::SubtitleBurn::Image {
            ordinal: 0,
        })),
    ));
    assert!(matches!(playback(&engine).subtitle, VideoSubtitle::Off));
}

#[test]
fn the_default_audio_track_is_chosen_and_menus_toggle_and_close() {
    let (mut engine, _) = opened(PreviewLoadSurface::RightDockedPanel, 3);
    // details() 把第 0 条标成默认。
    assert_eq!(playback(&engine).audio_track, 0);

    drop(engine.toggle_video_track_menu(VideoTrackMenu::Audio));
    assert_eq!(playback(&engine).track_menu, Some(VideoTrackMenu::Audio));
    drop(engine.toggle_video_track_menu(VideoTrackMenu::Subtitles));
    assert_eq!(
        playback(&engine).track_menu,
        Some(VideoTrackMenu::Subtitles)
    );
    drop(engine.toggle_video_track_menu(VideoTrackMenu::Subtitles));
    assert_eq!(playback(&engine).track_menu, None);

    drop(engine.toggle_video_track_menu(VideoTrackMenu::Audio));
    drop(engine.select_video_audio_track(2));
    assert_eq!(playback(&engine).audio_track, 2);
    assert_eq!(playback(&engine).track_menu, None);

    // 菜单开着时点画面只关菜单，不算一次单击。
    drop(engine.toggle_video_track_menu(VideoTrackMenu::Audio));
    drop(engine.video_surface_pressed());
    assert_eq!(playback(&engine).track_menu, None);
    assert_eq!(playback(&engine).pending_click, None);
}

#[test]
fn info_layer_toggles_for_this_preview_only() {
    let (mut engine, _) = opened(PreviewLoadSurface::RightDockedPanel, 0);
    assert!(!playback(&engine).info_visible);
    drop(engine.toggle_video_info());
    assert!(playback(&engine).info_visible);
    drop(engine.handle_media_key(crate::preview_message::MediaKeyCommand::ToggleInfo));
    assert!(!playback(&engine).info_visible);
}

#[tokio::test]
async fn finishing_follows_the_shared_end_action() {
    use crate::preview_config::MediaEndAction;
    use crate::preview_loading::PreviewPathKind;

    // 单个循环：从头开一段接着放。
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    engine.set_media_end_action(MediaEndAction::Repeat);
    let generation = playback(&engine).generation;
    drop(engine.accept_video_preview_finished(&path, generation));
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );
    let segment = engine.active_video_preview_stream().expect("decoding");
    assert_eq!(segment.start, Duration::ZERO);
    assert!(segment.generation > generation);

    // 播下一个：停下，交宿主找下一个视频。
    let (mut engine, path) = opened(PreviewLoadSurface::StandaloneWindow, 0);
    engine.set_media_end_action(MediaEndAction::Next);
    let generation = playback(&engine).generation;
    let task = engine.accept_video_preview_finished(&path, generation);
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Finished
    );
    let outputs = task_outputs(task).await;
    assert!(matches!(
        outputs.as_slice(),
        [PreviewMessage::MediaNextRequested(finished, PreviewPathKind::Video)] if finished == &path
    ));
}

async fn task_outputs(task: Task<PreviewMessage>) -> Vec<PreviewMessage> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut outputs = Vec::new();
    while let Some(action) = stream.next().await {
        if let iced_runtime::Action::Output(message) = action {
            outputs.push(message);
        }
    }
    outputs
}

#[tokio::test]
async fn the_queued_next_video_plays_in_the_panel_and_keeps_fullscreen() {
    use crate::engine::video_fullscreen::VideoFullscreen;

    let (mut engine, _) = opened(PreviewLoadSurface::RightDockedPanel, 0);
    engine.video_fullscreen = Some(VideoFullscreen::MainWindow);
    let next = PathBuf::from("/videos/next.mkv");
    engine.queue_next_media(next.clone());
    // 下一个在加载、旧的已经清掉：主窗口别退出全屏。
    engine.clear_video_preview();
    assert!(engine.keeps_video_fullscreen());

    let details = Arc::new(details(0));
    engine.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: next.clone(),
        details: Arc::clone(&details),
        frame: None,
    }));
    drop(engine.open_video_preview(&next, &details));
    drop(engine.accept_video_display(&next, Some(Size::new(1920, 1080))));
    assert_eq!(
        playback(&engine).status,
        VideoPreviewPlaybackStatus::Playing
    );
    assert_eq!(engine.video_fullscreen, Some(VideoFullscreen::MainWindow));
    assert!(engine.queued_media.is_none());
}
