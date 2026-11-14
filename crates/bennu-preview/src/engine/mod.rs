//! PreviewEngine：预览子系统的共享状态机宿主。聚合原先散在 app-ui
//! FileBrowser 上的预览字段（任务 design.md「解耦核心」），方法与消息
//! 处理以 [`crate::preview_message::PreviewMessage`] 为输出契约；宿主
//! （app-ui / portal-backend）组合本引擎并经 `Task::map` 包装进各自顶层
//! Message。窗口命令（window::open/resize/close）由引擎直接产出（iced
//! Task 与宿主无关）。

use iced::window::Id;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::document_preview::PendingDocumentPreview;
use crate::image_preview_viewport::ImagePreviewViewport;
use crate::preview::{
    AudioAnalysis, AudioAnalysisCache, AudioPreviewPlayback, PreviewBottomControlsState,
    PreviewSize, PreviewState, PreviewWindowProfile, VideoPreviewPlayback,
};
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeLimits};
use crate::preview_message::PreviewMessage;
use crate::text_document::TextViewerEvent;
use crate::text_preview::TextPreviewDocument;

/// 引擎的宿主配置快照：预览域行为参数由宿主（app-ui 的 UserConfig /
/// portal 的等价配置）填充，引擎不读宿主持久化层。宿主在构造引擎与
/// 预览偏好变更落盘时刷新，保证快照与持久化值一致。
#[derive(Debug, Clone)]
pub struct PreviewEngineConfig {
    /// 预览树目录自动展开层数（目录/归档预览树共用）。
    pub directory_expand_levels: u8,
    /// 自定义后缀 → 预览类型分派规则。
    pub extension_rules: PreviewExtensionRules,
    /// 分类型预览大小上限。
    pub size_limits: PreviewFileSizeLimits,
}

/// 预览状态机聚合体。字段为 pub：两个宿主的视图直接读状态，宿主在少数
/// 窗口/指针入口直接改状态。
pub struct PreviewEngine {
    /// 当前无引擎内消费者：宿主全部实时读 user_config 传参（测试直改
    /// user_config 必须立即生效，快照做不到）。字段保留是给 portal 接入
    /// （子任务 09-21-filechooser-space-preview）的构造入口，接入前禁止
    /// 引擎内部读它——实时值与快照并存会造成"改设置未落盘不生效"。
    pub config: PreviewEngineConfig,
    pub preview: Option<PreviewState>,
    pub pending_document_preview: Option<PendingDocumentPreview>,
    pub document_preview_generation: u64,
    pub remote_preview_download_cancel: Option<CancellationToken>,
    pub text_preview_document: Option<TextPreviewDocument>,
    pub animated_image_preview_generation: u64,
    pub original_image_preview_generation: u64,
    pub original_image_preview_cancel: Option<CancellationToken>,
    pub remote_preview_download_generation: u64,
    pub text_preview_generation: u64,
    pub sqlite_preview: Option<crate::engine::sqlite::SqlitePreviewState>,
    pub sqlite_preview_generation: u64,
    pub sqlite_tables_resize_drag: Option<crate::engine::sqlite::SqliteTablesResizeDrag>,
    pub audio_preview: Option<AudioPreviewPlayback>,
    /// 当前这首的波形与声谱图；换内容时丢弃（丢弃即取消计算）。
    pub audio_analysis: Option<AudioAnalysis>,
    pub audio_analysis_cache: AudioAnalysisCache,
    pub video_preview: Option<VideoPreviewPlayback>,
    /// 音频、视频预览共用的音量（0.0–1.0），唯一来源：宿主加载设置时注入，
    /// 改动经 `MediaVolumeChanged` 交宿主记住后由 `set_media_volume` 生效。
    pub media_volume: f32,
    pub preview_size: PreviewSize,
    pub pending_preview_resize: Option<PreviewSize>,
    pub preview_window_profile: PreviewWindowProfile,
    pub preview_window_pinned: bool,
    pub preview_shown_path: Option<std::path::PathBuf>,
    pub preview_window_bottom_controls: PreviewBottomControlsState,
    pub preview_window_pointer_y: Option<f32>,
    pub preview_image_viewport: ImagePreviewViewport,
    pub preview_window_initial_controls_generation: u64,
    pub preview_window: Option<Id>,
    /// 预览档位缩略图的展示占位标记（原图已并行加载，标记只决定磁盘
    /// 缓存探测回流是否替换 Loading 展示）；入队/回流入口在宿主缩略图
    /// 管线，状态归预览会话所有。
    pub pending_preview_thumbnail_display: Option<original_image::PendingPreviewThumbnailDisplay>,
    /// 当前预览会话的呈现面：加载管线的窗口尺寸/聚焦动作只归独立窗口
    /// 会话所有；面板会话绝不弹出、缩放或抢占独立窗口（见
    /// [`PreviewLoadSurface`]）。
    pub preview_load_surface: PreviewLoadSurface,
    /// 预览窗口的宿主进程身份（应用 id 与启动图标）：开窗 Task 由引擎
    /// 产出，但窗口归属宿主进程，身份由宿主构造引擎时注入。
    pub window_identity: PreviewWindowIdentity,
}

/// 当前预览会话由哪个呈现面发起。加载管线的窗口尺寸/聚焦动作只归
/// 独立窗口会话所有;面板会话绝不弹出、缩放或抢占 Space 独立窗口。
/// 异步加载回流(图片尺寸、视频帧、加载失败等)无法从消息参数得知
/// 发起方,因此必须在会话开始时记入状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewLoadSurface {
    StandaloneWindow,
    RightDockedPanel,
}

/// accept_preview / accept_animated_image_preview_loaded 的验收结论：
/// ContentReady 表示内容已上屏（宿主需按原语义清除全局错误通知），
/// NotDisplayed 表示未上屏（过期丢弃或错误态），宿主不动全局错误状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewAcceptance {
    ContentReady,
    NotDisplayed,
}

impl PreviewEngine {
    /// 初始预览窗口尺寸由宿主传入（尺寸常量归各宿主的窗口管理层），
    /// 窗口身份（应用 id/图标）同为宿主进程属性一并注入。
    pub fn new(
        config: PreviewEngineConfig,
        initial_preview_size: PreviewSize,
        window_identity: PreviewWindowIdentity,
    ) -> Self {
        Self {
            config,
            preview: None,
            pending_document_preview: None,
            document_preview_generation: 0,
            remote_preview_download_cancel: None,
            text_preview_document: None,
            animated_image_preview_generation: 0,
            original_image_preview_generation: 0,
            original_image_preview_cancel: None,
            remote_preview_download_generation: 0,
            text_preview_generation: 0,
            sqlite_preview: None,
            sqlite_preview_generation: 0,
            sqlite_tables_resize_drag: None,
            audio_preview: None,
            audio_analysis: None,
            audio_analysis_cache: AudioAnalysisCache::default(),
            video_preview: None,
            media_volume: 1.0,
            preview_size: initial_preview_size,
            pending_preview_resize: None,
            preview_window_profile: PreviewWindowProfile::Regular,
            preview_window_pinned: false,
            preview_shown_path: None,
            preview_window_bottom_controls: PreviewBottomControlsState::default(),
            preview_window_pointer_y: None,
            preview_image_viewport: ImagePreviewViewport::default(),
            preview_window_initial_controls_generation: 0,
            preview_window: None,
            pending_preview_thumbnail_display: None,
            preview_load_surface: PreviewLoadSurface::StandaloneWindow,
            window_identity,
        }
    }

    /// 处理只依赖引擎自身状态的 [`PreviewMessage`] 变体。需要宿主参数
    /// （扫描选项、视口几何、滚动管线）或宿主通知域的变体原样经 `Err`
    /// 还给宿主：每个变体只有一个处理者，且无需为转交而克隆消息。消息体
    /// 较大，装箱后 `Err` 不会撑大每次调用的返回值。
    pub fn handle(
        &mut self,
        message: PreviewMessage,
    ) -> Result<Task<PreviewMessage>, Box<PreviewMessage>> {
        let task = match message {
            PreviewMessage::SqlitePreview(inner) => self.handle_sqlite_preview_message(inner),
            // 字号与换行是宿主的设置（主程序要写设置库，对话框只当次有效）。
            PreviewMessage::TextViewer(TextViewerEvent::ChangeView(change)) => {
                return Err(Box::new(PreviewMessage::TextViewer(
                    TextViewerEvent::ChangeView(change),
                )));
            }
            PreviewMessage::TextViewer(event) => self.handle_text_viewer_event(event),
            PreviewMessage::TextRead { document, reply } => self.accept_text_read(document, reply),
            PreviewMessage::TextFileChanged { document } => self.handle_text_file_changed(document),
            PreviewMessage::TextPreviewEncodingSelected(encoding) => {
                self.select_text_preview_encoding(encoding)
            }
            PreviewMessage::MarkdownPreviewModeSelected(mode) => self.select_markdown_mode(mode),
            PreviewMessage::PreviewTreeAnimationTick => self.advance_preview_tree_animation(),
            PreviewMessage::PreviewWindowPinToggled => self.toggle_preview_window_pin(),
            PreviewMessage::PreviewWindowInitialControlsElapsed(generation) => {
                self.hide_preview_window_initial_controls(generation);
                Task::none()
            }
            PreviewMessage::AnimatedImageFrameLoaded(frame) => {
                self.accept_animated_image_frame(frame)
            }
            PreviewMessage::AnimatedImagePreviewFinished(path, generation) => {
                self.accept_animated_image_preview_finished(path, generation)
            }
            PreviewMessage::AnimatedImagePreviewFailed(path, generation, error) => {
                self.accept_animated_image_preview_error(path, generation, error)
            }
            PreviewMessage::AnimatedImageSeekRequested(position) => {
                self.seek_animated_image_preview(position)
            }
            PreviewMessage::AnimatedImageSeekCommitted => self.commit_animated_image_preview_seek(),
            PreviewMessage::AudioPreviewPlaybackToggled => self.toggle_audio_preview_playback(),
            PreviewMessage::AudioPreviewStarted(path, playback_outcome) => {
                self.accept_audio_preview_started(path, playback_outcome)
            }
            PreviewMessage::AudioPreviewTick => self.update_audio_preview_playback(),
            PreviewMessage::AudioPreviewSeekRequested(position) => {
                self.drag_audio_preview_position(position)
            }
            PreviewMessage::AudioPreviewSeekCommitted => self.commit_audio_preview_seek(),
            PreviewMessage::AudioPreviewDurationMeasured(path, duration) => {
                self.accept_audio_preview_duration(&path, duration)
            }
            PreviewMessage::AudioPreviewCoverLoaded(path, cover) => {
                self.accept_audio_preview_cover(&path, cover)
            }
            PreviewMessage::AudioWaveformRequested => self.request_audio_waveform(),
            PreviewMessage::AudioWaveformComputed(path, outcome) => {
                self.accept_audio_waveform(&path, outcome)
            }
            PreviewMessage::AudioTimelineViewSelected(view) => {
                self.select_audio_timeline_view(view)
            }
            PreviewMessage::AudioSpectrogramComputed(path, outcome) => {
                self.accept_audio_spectrogram(&path, outcome)
            }
            PreviewMessage::AudioLyricsLoaded(path, lyrics) => {
                self.accept_audio_lyrics(&path, lyrics)
            }
            PreviewMessage::AudioLyricsBrowsed(lines) => self.browse_audio_lyrics(lines),
            PreviewMessage::AudioLyricsLineSelected(index) => self.seek_to_lyric_line(index),
            PreviewMessage::CopyFfmpegInstallCommand => {
                match crate::ffmpeg_install::ffmpeg_install_command() {
                    Some(command) => iced::clipboard::write(command.to_owned()),
                    None => Task::none(),
                }
            }
            PreviewMessage::VideoPreviewPlaybackToggled => self.toggle_video_preview_playback(),
            PreviewMessage::VideoPreviewAudioStarted(path, generation, audio_outcome) => {
                self.accept_video_preview_audio_started(path, generation, audio_outcome)
            }
            PreviewMessage::VideoPreviewMetadataLoaded(path, metadata_outcome) => {
                self.accept_video_preview_metadata(path, metadata_outcome)
            }
            PreviewMessage::VideoPreviewSeekRequested(position) => {
                self.seek_video_preview_playback(position)
            }
            PreviewMessage::VideoPreviewSeekCommitted => self.commit_video_preview_seek(),
            PreviewMessage::MediaKey(command) => self.handle_media_key(command),
            PreviewMessage::VideoPreviewTick => self.update_video_preview_playback(),
            PreviewMessage::VideoPreviewFrameLoaded(frame) => {
                self.accept_video_preview_frame(frame)
            }
            PreviewMessage::VideoPreviewSeekFrameFailed(path, generation, position, error) => {
                self.accept_video_preview_seek_frame_error(path, generation, position, error)
            }
            PreviewMessage::VideoPreviewFinished(path, generation) => {
                self.accept_video_preview_finished(path, generation)
            }
            PreviewMessage::VideoPreviewFailed(path, generation, error) => {
                self.accept_video_preview_error(path, generation, error)
            }
            host_message => return Err(Box::new(host_message)),
        };
        Ok(task)
    }
}

mod animated_image;
mod audio;
mod audio_analysis;
mod document;
mod image_viewport;
mod media;
mod original_image;
mod remote_cache;
mod session;
mod sqlite;
mod text;
mod text_find;
mod tree;
mod windows;

pub use media::{audio_preview_tick_subscription, video_preview_tick_subscription};
pub use original_image::PendingPreviewThumbnailDisplay;
pub use remote_cache::RemotePreviewCacheCompletion;
pub use sqlite::{SqlitePreviewState, SqliteTablesResizeDrag, SQLITE_DEFAULT_TABLES_WIDTH};
pub use tree::auto_expand_preview_tree_directories;
pub use windows::{
    animated_image_preview_size_from_dimensions, clamp_preview_size_to_minimum,
    default_preview_size, image_preview_initial_fit_max_size, image_preview_size_from_dimensions,
    preview_content_size_from_window, preview_min_size, preview_size_matches,
    preview_window_settings, preview_window_size_for_content, video_preview_initial_fit_max_size,
    video_preview_size_from_frame, PreviewWindowIdentity,
};

#[cfg(test)]
mod tests {
    use iced::futures::StreamExt;
    use std::path::PathBuf;

    use super::*;
    use crate::preview::{PreviewContent, PreviewWindowProfile};
    use crate::preview_message::MediaKeyCommand;

    fn engine() -> PreviewEngine {
        PreviewEngine::new(
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
        )
    }

    fn returned_to_host(engine: &mut PreviewEngine, message: PreviewMessage) -> PreviewMessage {
        match engine.handle(message) {
            Err(message) => *message,
            Ok(_) => panic!("the engine handled a host-owned message"),
        }
    }

    #[test]
    fn messages_that_need_the_host_come_back_unchanged() {
        let mut engine = engine();
        let window = Id::unique();
        let path = PathBuf::from("/tmp/photo.png");

        for message in [
            PreviewMessage::SqliteTablesResizeStarted,
            // 字号与换行是宿主设置，引擎原样还回去。
            PreviewMessage::TextViewer(TextViewerEvent::ChangeView(
                crate::text_view::TextViewChange::ZoomIn,
            )),
        ] {
            returned_to_host(&mut engine, message);
        }
        assert!(matches!(
            returned_to_host(&mut engine, PreviewMessage::PreviewWindowPresented(window)),
            PreviewMessage::PreviewWindowPresented(returned) if returned == window
        ));
        assert!(matches!(
            returned_to_host(&mut engine, PreviewMessage::RetryImagePreview(path.clone())),
            PreviewMessage::RetryImagePreview(returned) if returned == path
        ));
    }

    #[test]
    fn engine_owned_messages_are_handled_in_place() {
        let mut engine = engine();

        assert!(engine
            .handle(PreviewMessage::PreviewWindowPinToggled)
            .is_ok());
        assert!(engine.preview_window_pinned);
        assert!(engine.handle(PreviewMessage::AudioPreviewTick).is_ok());
    }

    #[tokio::test]
    async fn presenting_an_existing_window_announces_it_to_the_host() {
        let mut engine = engine();
        let window = Id::unique();
        engine.preview_window = Some(window);

        let (_, task) = engine.ensure_preview_window(PreviewWindowProfile::Regular);
        let mut stream = iced_runtime::task::into_stream(task).expect("presentation task stream");
        let mut announced = false;
        while let Some(action) = stream.next().await {
            if matches!(
                action,
                iced_runtime::Action::Output(PreviewMessage::PreviewWindowPresented(presented))
                    if presented == window
            ) {
                announced = true;
                break;
            }
        }

        assert!(announced);
    }

    async fn task_outputs(task: Task<PreviewMessage>) -> Vec<PreviewMessage> {
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

    fn ready_audio(engine: &mut PreviewEngine, path: &std::path::Path, seconds: u64) {
        engine.preview = Some(PreviewState::Ready(PreviewContent::Audio {
            path: path.to_path_buf(),
            details: Box::new(crate::preview::AudioPreviewDetails {
                len: 1,
                probe: crate::audio_preview::AudioProbe {
                    duration: Some(std::time::Duration::from_secs(seconds)),
                    duration_is_estimate: false,
                    tags: Default::default(),
                    stream: Default::default(),
                    embedded_cover_stream: None,
                    embedded_lyrics: None,
                },
            }),
            cover: None,
            lyrics: None,
        }));
    }

    #[test]
    fn right_panel_audio_waits_for_play_but_the_window_starts_at_once() {
        let path = PathBuf::from("/music/song.opus");
        let mut panel = engine();
        panel.preview_load_surface = PreviewLoadSurface::RightDockedPanel;
        drop(panel.open_audio_preview(path.clone()));
        assert_eq!(
            panel.audio_preview.as_ref().map(|playback| playback.status),
            Some(crate::preview::AudioPreviewPlaybackStatus::Ready)
        );

        let mut window = engine();
        drop(window.open_audio_preview(path));
        assert_eq!(
            window
                .audio_preview
                .as_ref()
                .map(|playback| playback.status),
            Some(crate::preview::AudioPreviewPlaybackStatus::Loading)
        );
    }

    #[tokio::test]
    async fn volume_changes_go_to_the_host_and_keys_turn_into_volume_changes() {
        let mut engine = engine();
        assert!(matches!(
            returned_to_host(&mut engine, PreviewMessage::MediaVolumeChanged(0.4)),
            PreviewMessage::MediaVolumeChanged(volume) if volume == 0.4
        ));

        engine.set_media_volume(0.98);
        let task = engine
            .handle(PreviewMessage::MediaKey(MediaKeyCommand::ChangeVolume(
                0.05,
            )))
            .expect("engine handles media keys");
        let outputs = task_outputs(task).await;
        assert!(matches!(
            outputs.as_slice(),
            [PreviewMessage::MediaVolumeChanged(volume)] if *volume == 1.0
        ));
        engine.set_media_volume(7.0);
        assert_eq!(engine.media_volume, 1.0);
    }

    #[test]
    fn seek_keys_step_from_the_current_position_within_the_track() {
        let path = PathBuf::from("/music/song.flac");
        let mut engine = engine();
        engine.preview_load_surface = PreviewLoadSurface::RightDockedPanel;
        ready_audio(&mut engine, &path, 12);
        drop(engine.open_audio_preview(path));

        let step = std::time::Duration::from_secs(5);
        for _ in 0..3 {
            drop(engine.handle(PreviewMessage::MediaKey(MediaKeyCommand::SeekForward(step))));
        }
        let position = |engine: &PreviewEngine| {
            engine
                .audio_preview
                .as_ref()
                .map(|playback| playback.position)
        };
        // 停在结尾，不越过时长。
        assert_eq!(position(&engine), Some(std::time::Duration::from_secs(12)));
        drop(
            engine.handle(PreviewMessage::MediaKey(MediaKeyCommand::SeekBackward(
                std::time::Duration::from_secs(30),
            ))),
        );
        assert_eq!(position(&engine), Some(std::time::Duration::ZERO));
    }

    #[test]
    fn audio_and_video_previews_take_the_keyboard() {
        let mut engine = engine();
        assert!(!engine.preview_takes_keyboard());
        ready_audio(&mut engine, std::path::Path::new("/music/a.mp3"), 1);
        assert!(engine.preview_takes_keyboard());
    }

    #[test]
    fn waveform_starts_once_fills_the_cache_and_fixes_an_estimated_duration() {
        use crate::preview::AudioAnalysisState;
        let path = PathBuf::from("/music/vbr.mp3");
        let mut engine = engine();
        ready_audio(&mut engine, &path, 300);
        if let Some(PreviewState::Ready(PreviewContent::Audio { details, .. })) =
            &mut engine.preview
        {
            details.probe.duration_is_estimate = true;
        }
        engine.begin_audio_analysis(&path, 1);

        drop(engine.request_audio_waveform());
        assert!(matches!(
            engine
                .audio_analysis
                .as_ref()
                .map(|analysis| &analysis.waveform),
            Some(AudioAnalysisState::Computing)
        ));
        // 正在算时再要一次不会再起一个 ffmpeg。
        drop(engine.request_audio_waveform());

        let measured = std::sync::Arc::new(crate::audio_preview::AudioWaveform {
            peaks: vec![0.2; 10],
            duration: std::time::Duration::from_secs(287),
        });
        drop(engine.handle(PreviewMessage::AudioWaveformComputed(
            path.clone(),
            Ok(measured),
        )));
        let Some(PreviewState::Ready(PreviewContent::Audio { details, .. })) = &engine.preview
        else {
            panic!("audio preview");
        };
        assert_eq!(
            details.probe.duration,
            Some(std::time::Duration::from_secs(287))
        );
        assert!(!details.probe.duration_is_estimate);

        // 下次打开同一首直接就绪。
        engine.begin_audio_analysis(&path, 1);
        assert!(matches!(
            engine
                .audio_analysis
                .as_ref()
                .map(|analysis| &analysis.waveform),
            Some(AudioAnalysisState::Ready(_))
        ));
    }

    #[test]
    fn switching_to_the_spectrogram_starts_it_only_once() {
        use crate::preview::{AudioAnalysisState, AudioTimelineView};
        let path = PathBuf::from("/music/a.flac");
        let mut engine = engine();
        engine.begin_audio_analysis(&path, 1);
        drop(engine.handle(PreviewMessage::AudioTimelineViewSelected(
            AudioTimelineView::Spectrogram,
        )));
        let analysis = engine.audio_analysis.as_ref().expect("analysis");
        assert_eq!(analysis.view, AudioTimelineView::Spectrogram);
        assert!(matches!(
            analysis.spectrogram,
            AudioAnalysisState::Computing
        ));
        assert!(matches!(
            analysis.waveform,
            AudioAnalysisState::NotRequested
        ));
    }

    #[test]
    fn clicking_a_lyric_line_seeks_there_and_stops_browsing() {
        use crate::audio_preview::{AudioLyrics, LyricLine};
        let path = PathBuf::from("/music/song.flac");
        let mut engine = engine();
        engine.preview_load_surface = PreviewLoadSurface::RightDockedPanel;
        ready_audio(&mut engine, &path, 200);
        if let Some(PreviewState::Ready(PreviewContent::Audio { lyrics, .. })) = &mut engine.preview
        {
            *lyrics = Some(std::sync::Arc::new(AudioLyrics::Synced(vec![
                LyricLine {
                    time: std::time::Duration::from_secs(5),
                    text: "a".to_owned(),
                },
                LyricLine {
                    time: std::time::Duration::from_secs(42),
                    text: "b".to_owned(),
                },
            ])));
        }
        drop(engine.open_audio_preview(path));

        drop(engine.handle(PreviewMessage::AudioLyricsBrowsed(3)));
        let browse = engine
            .audio_preview
            .as_ref()
            .map(|playback| playback.lyrics_browse);
        assert_eq!(browse.map(|browse| browse.offset_lines), Some(3));
        assert!(browse.and_then(|browse| browse.resume_at).is_some());

        drop(engine.handle(PreviewMessage::AudioLyricsLineSelected(1)));
        let playback = engine.audio_preview.as_ref().expect("playback");
        assert_eq!(playback.position, std::time::Duration::from_secs(42));
        assert_eq!(
            playback.lyrics_browse,
            crate::preview::LyricsBrowse::default()
        );
        // 越界的句号什么都不做。
        drop(engine.handle(PreviewMessage::AudioLyricsLineSelected(9)));
    }
}
