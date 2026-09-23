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
    AudioPreviewPlayback, PreviewSize, PreviewState, PreviewWindowChromeState,
    PreviewWindowProfile, VideoPreviewPlayback,
};
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeLimits};
use crate::preview_message::PreviewMessage;
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

/// 预览状态机聚合体。字段可见性放宽为 pub 是迁移期需要：宿主 FileBrowser
/// 经 Deref 垫片访问字段（未迁 impl 块/视图/测试零改动），剩余 impl 块
/// 迁入后随垫片一并收紧。
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
    pub video_preview: Option<VideoPreviewPlayback>,
    pub preview_size: PreviewSize,
    pub text_preview_content_height: f32,
    pub pending_preview_resize: Option<PreviewSize>,
    pub preview_window_profile: PreviewWindowProfile,
    pub preview_window_pinned: bool,
    pub preview_shown_path: Option<std::path::PathBuf>,
    pub preview_window_chrome: PreviewWindowChromeState,
    pub preview_window_bottom_controls: PreviewWindowChromeState,
    pub preview_window_drag_active: bool,
    pub preview_window_pointer_y: Option<f32>,
    pub preview_image_viewport: ImagePreviewViewport,
    pub preview_window_initial_chrome_generation: u64,
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
    /// 引擎呈现预览窗口后把它登记为待同步焦点；宿主焦点簿记
    /// （focused_window）不归引擎所有，由宿主在转发层或 update 出口
    /// take 走并写入自己的焦点状态。
    pending_focus_window: Option<Id>,
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
            video_preview: None,
            preview_size: initial_preview_size,
            text_preview_content_height: 0.0,
            pending_preview_resize: None,
            preview_window_profile: PreviewWindowProfile::Regular,
            preview_window_pinned: false,
            preview_shown_path: None,
            preview_window_chrome: PreviewWindowChromeState::default(),
            preview_window_bottom_controls: PreviewWindowChromeState::default(),
            preview_window_drag_active: false,
            preview_window_pointer_y: None,
            preview_image_viewport: ImagePreviewViewport::default(),
            preview_window_initial_chrome_generation: 0,
            preview_window: None,
            pending_preview_thumbnail_display: None,
            preview_load_surface: PreviewLoadSurface::StandaloneWindow,
            window_identity,
            pending_focus_window: None,
        }
    }

    /// 取走「预览窗口应成为应用焦点」的登记（一次性）。宿主在预览
    /// 窗口呈现转发后与 update 出口各同步一次，保证引擎内部路径
    /// （消息直达引擎）与宿主转发路径的焦点簿记行为一致。
    pub fn take_pending_preview_window_focus(&mut self) -> Option<Id> {
        self.pending_focus_window.take()
    }

    /// 处理已迁入引擎的 [`PreviewMessage`] 变体。返回 `None` 表示该变体
    /// 的处理仍在宿主散装分支（迁移期回退路径），宿主负责经
    /// `From<PreviewMessage>` 机械映射后走既有分支；随 impl 块分批迁入，
    /// None 的变体逐渐减少直至清零。
    pub fn handle(&mut self, message: PreviewMessage) -> Option<Task<PreviewMessage>> {
        match message {
            PreviewMessage::SqlitePreview(inner) => Some(self.handle_sqlite_preview_message(inner)),
            PreviewMessage::TextPreviewContentScrolled {
                lines,
                viewport_height,
            } => Some(self.handle_text_preview_content_scrolled(lines, viewport_height)),
            PreviewMessage::TextPreviewChunkLoaded {
                path,
                generation,
                start_offset,
                outcome,
            } => Some(self.accept_text_preview_chunk(path, generation, start_offset, outcome)),
            PreviewMessage::PreviewTreeAnimationTick => Some(self.advance_preview_tree_animation()),
            PreviewMessage::PreviewWindowPinToggled => Some(self.toggle_preview_window_pin()),
            PreviewMessage::PreviewWindowInitialChromeElapsed(generation) => {
                self.hide_preview_window_initial_chrome(generation);
                Some(Task::none())
            }
            PreviewMessage::AnimatedImageFrameLoaded(frame) => {
                Some(self.accept_animated_image_frame(frame))
            }
            PreviewMessage::AnimatedImagePreviewFinished(path, generation) => {
                Some(self.accept_animated_image_preview_finished(path, generation))
            }
            PreviewMessage::AnimatedImagePreviewFailed(path, generation, error) => {
                Some(self.accept_animated_image_preview_error(path, generation, error))
            }
            PreviewMessage::AnimatedImageSeekRequested(position) => {
                Some(self.seek_animated_image_preview(position))
            }
            PreviewMessage::AnimatedImageSeekCommitted => {
                Some(self.commit_animated_image_preview_seek())
            }
            PreviewMessage::AudioPreviewPlaybackToggled => {
                Some(self.toggle_audio_preview_playback())
            }
            PreviewMessage::AudioPreviewStarted(path, playback_outcome) => {
                Some(self.accept_audio_preview_started(path, playback_outcome))
            }
            PreviewMessage::AudioPreviewVolumeChanged(volume) => {
                Some(self.change_audio_preview_volume(volume))
            }
            PreviewMessage::AudioPreviewTick => Some(self.update_audio_preview_playback()),
            PreviewMessage::VideoPreviewPlaybackToggled => {
                Some(self.toggle_video_preview_playback())
            }
            PreviewMessage::VideoPreviewAudioStarted(path, generation, audio_outcome) => {
                Some(self.accept_video_preview_audio_started(path, generation, audio_outcome))
            }
            PreviewMessage::VideoPreviewMetadataLoaded(path, metadata_outcome) => {
                Some(self.accept_video_preview_metadata(path, metadata_outcome))
            }
            PreviewMessage::VideoPreviewSeekRequested(position) => {
                Some(self.seek_video_preview_playback(position))
            }
            PreviewMessage::VideoPreviewSeekCommitted => Some(self.commit_video_preview_seek()),
            PreviewMessage::VideoPreviewVolumeChanged(volume) => {
                Some(self.change_video_preview_volume(volume))
            }
            PreviewMessage::VideoPreviewTick => Some(self.update_video_preview_playback()),
            PreviewMessage::VideoPreviewFrameLoaded(frame) => {
                Some(self.accept_video_preview_frame(frame))
            }
            PreviewMessage::VideoPreviewSeekFrameFailed(path, generation, position, error) => {
                Some(self.accept_video_preview_seek_frame_error(path, generation, position, error))
            }
            PreviewMessage::VideoPreviewFinished(path, generation) => {
                Some(self.accept_video_preview_finished(path, generation))
            }
            PreviewMessage::VideoPreviewFailed(path, generation, error) => {
                Some(self.accept_video_preview_error(path, generation, error))
            }
            _ => None,
        }
    }
}

mod animated_image;
mod document;
mod image_viewport;
mod media;
mod original_image;
mod remote_cache;
mod session;
mod sqlite;
mod text;
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
