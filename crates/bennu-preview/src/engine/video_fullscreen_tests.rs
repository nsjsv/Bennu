use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::engine::{
    default_preview_size, PointerInterest, PreviewEngine, PreviewEngineConfig,
    PreviewWindowIdentity,
};
use crate::preview::{
    PreviewContent, PreviewState, PreviewWindowProfile, VideoPreviewDetails, VideoTrackMenu,
};
use crate::preview_config::PreviewFileSizeLimits;
use crate::video_preview::VideoProbe;

fn engine_with_video(surface: PreviewLoadSurface) -> PreviewEngine {
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
    let path = PathBuf::from("/videos/clip.mkv");
    let details = Arc::new(VideoPreviewDetails {
        len: 1,
        probe: VideoProbe::default(),
        subtitles: Vec::new(),
    });
    engine.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: path.clone(),
        details: Arc::clone(&details),
        frame: None,
    }));
    drop(engine.open_video_preview(&path, &details));
    engine
}

#[tokio::test]
async fn the_window_or_the_main_window_goes_fullscreen_and_back() {
    let mut window = engine_with_video(PreviewLoadSurface::StandaloneWindow);
    window.preview_window = Some(window::Id::unique());
    drop(window.toggle_video_fullscreen());
    assert_eq!(
        window.video_fullscreen,
        Some(VideoFullscreen::PreviewWindow)
    );
    drop(window.toggle_video_fullscreen());
    assert_eq!(window.video_fullscreen, None);

    let mut panel = engine_with_video(PreviewLoadSurface::RightDockedPanel);
    drop(panel.toggle_video_fullscreen());
    assert_eq!(panel.video_fullscreen, Some(VideoFullscreen::MainWindow));
    drop(panel.leave_video_fullscreen());
    assert_eq!(panel.video_fullscreen, None);
}

#[tokio::test]
async fn a_double_click_toggles_fullscreen_without_toggling_playback() {
    let mut engine = engine_with_video(PreviewLoadSurface::RightDockedPanel);
    let status = engine.video_preview.as_ref().expect("video").status;
    drop(engine.video_surface_pressed());
    drop(engine.video_surface_pressed());
    assert_eq!(engine.video_fullscreen, Some(VideoFullscreen::MainWindow));
    assert_eq!(engine.video_preview.as_ref().expect("video").status, status);
}

#[tokio::test]
async fn idle_pointer_hides_controls_and_cursor_until_it_moves() {
    let mut engine = engine_with_video(PreviewLoadSurface::RightDockedPanel);
    drop(engine.toggle_video_fullscreen());
    assert!(engine.preview_window_bottom_controls.target_is_visible());

    // 截止时间还没到：再等，不藏。
    drop(engine.video_fullscreen_idle_elapsed());
    assert!(!engine.video_preview.as_ref().expect("video").cursor_hidden);

    engine.fullscreen_idle.deadline = Some(Instant::now());
    drop(engine.video_fullscreen_idle_elapsed());
    assert!(engine.video_preview.as_ref().expect("video").cursor_hidden);
    assert!(!engine.preview_window_bottom_controls.target_is_visible());

    // 一动就出来。
    drop(engine.preview_pointer_moved(10.0));
    assert!(!engine.video_preview.as_ref().expect("video").cursor_hidden);
    assert!(engine.preview_window_bottom_controls.target_is_visible());

    // 开着菜单时不藏。
    engine.video_preview.as_mut().expect("video").track_menu = Some(VideoTrackMenu::Audio);
    engine.fullscreen_idle.deadline = Some(Instant::now());
    drop(engine.video_fullscreen_idle_elapsed());
    assert!(!engine.video_preview.as_ref().expect("video").cursor_hidden);

    // 退出全屏：鼠标回来。
    engine.video_preview.as_mut().expect("video").cursor_hidden = true;
    drop(engine.leave_video_fullscreen());
    assert!(!engine.video_preview.as_ref().expect("video").cursor_hidden);
}

#[tokio::test]
async fn closing_the_fullscreen_preview_window_forgets_fullscreen() {
    let mut engine = engine_with_video(PreviewLoadSurface::StandaloneWindow);
    engine.preview_window = Some(window::Id::unique());
    drop(engine.toggle_video_fullscreen());
    drop(engine.close_preview_window());
    assert_eq!(engine.video_fullscreen, None);
}

/// 宿主只在引擎要时才把指针移动交过来：没有底部控制条的预览什么都不要；有控制条时只要底部那一条
/// 的进出；全屏时控件藏着才要每次移动，亮起来以后不要。
#[tokio::test]
async fn the_engine_asks_for_pointer_moves_only_when_they_change_something() {
    let idle = PreviewEngine::new(
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
    assert_eq!(
        idle.preview_window_pointer_interest(),
        PointerInterest::Nothing
    );
    assert_eq!(
        idle.main_window_pointer_interest(),
        PointerInterest::Nothing
    );

    let mut engine = engine_with_video(PreviewLoadSurface::StandaloneWindow);
    engine.preview_window = Some(window::Id::unique());
    let bottom = crate::window_shell::HEADER_BAR_HEIGHT + engine.preview_size.height;
    assert_eq!(
        engine.preview_window_pointer_interest(),
        PointerInterest::Band {
            top: bottom - crate::preview::PreviewBottomControlsState::REVEAL_HEIGHT,
            bottom,
        }
    );

    // 进全屏时控件先亮着；藏起来以后才要每次移动，一动亮起来就又不要了。
    drop(engine.toggle_video_fullscreen());
    assert_eq!(
        engine.preview_window_pointer_interest(),
        PointerInterest::Nothing
    );
    engine.fullscreen_idle.deadline = Some(Instant::now());
    drop(engine.video_fullscreen_idle_elapsed());
    assert_eq!(
        engine.preview_window_pointer_interest(),
        PointerInterest::Moves
    );
    drop(engine.preview_pointer_moved(10.0));
    assert_eq!(
        engine.preview_window_pointer_interest(),
        PointerInterest::Nothing
    );

    let mut panel = engine_with_video(PreviewLoadSurface::RightDockedPanel);
    assert_eq!(
        panel.main_window_pointer_interest(),
        PointerInterest::Nothing
    );
    drop(panel.toggle_video_fullscreen());
    panel.fullscreen_idle.deadline = Some(Instant::now());
    drop(panel.video_fullscreen_idle_elapsed());
    assert_eq!(panel.main_window_pointer_interest(), PointerInterest::Moves);
}

/// 全屏控件亮着时移动不再发消息：到点时按宿主记下的最后移动时刻顺延，鼠标还在动就不藏。
#[tokio::test]
async fn a_recent_move_recorded_by_the_host_keeps_the_controls() {
    let mut engine = engine_with_video(PreviewLoadSurface::StandaloneWindow);
    engine.preview_window = Some(window::Id::unique());
    let motion = bennu_theme::pointer_motion::PointerMotion::default();
    engine.share_pointer_motion(motion.clone());
    drop(engine.toggle_video_fullscreen());
    drop(engine.preview_pointer_moved(10.0));

    motion.record(Instant::now());
    engine.fullscreen_idle.deadline = Some(Instant::now());
    drop(engine.video_fullscreen_idle_elapsed());
    assert!(!engine.video_preview.as_ref().expect("video").cursor_hidden);
    assert!(engine.preview_window_bottom_controls.target_is_visible());

    motion.record(Instant::now() - FULLSCREEN_IDLE * 2);
    engine.fullscreen_idle.deadline = Some(Instant::now());
    drop(engine.video_fullscreen_idle_elapsed());
    assert!(engine.video_preview.as_ref().expect("video").cursor_hidden);
}
