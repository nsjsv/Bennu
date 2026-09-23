//! 预览子系统自有消息类型：两个宿主（app-ui / portal-backend）的顶层
//! Message 体系不同，共享状态机与命令层必须以本枚举为输出契约，宿主经
//! `Task::map` 包进各自 Message（iced 官方组合手段，见任务 design.md
//! 「PreviewMessage 独立枚举」）。变体与 app-ui 既有散装 preview 变体
//! 一一对应（design.md 附录 A 基线 62 个中的 61 个；
//! `ContextMenuPreviewExpansionChanged` 携带 app-ui 右键菜单词表
//! `FileAreaMenuItem`，属设置窗口右键菜单域而非预览域，不随迁）。

use std::path::PathBuf;
use std::time::Duration;

use file_core::DirectoryEntry;

use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePreview};
use crate::audio_preview::AudioPreviewRuntime;
use crate::document_preview::DocumentPreviewMessage;
use crate::image_preview_viewport::PreviewImageViewportMessage;
use crate::original_image_preview::OriginalImagePreview;
use crate::preview::{
    PreviewContent, RemotePreviewCacheMessage, RightPreviewPanelInfoSnapshot, VideoPreviewFrame,
};
use crate::sqlite_preview::SqlitePreviewMessage;
use crate::text_preview::{MarkdownPreviewMode, TextPreviewChunk};

#[derive(Debug, Clone)]
pub enum PreviewMessage {
    PreviewLoaded(PathBuf, Result<PreviewContent, String>),
    DocumentPreview(DocumentPreviewMessage),
    SqlitePreview(SqlitePreviewMessage),
    RemotePreviewCache(RemotePreviewCacheMessage),
    AnimatedImagePreviewLoaded(PathBuf, u64, Result<AnimatedImagePreview, String>),
    OriginalImagePreviewLoaded(PathBuf, u64, Result<OriginalImagePreview, String>),
    RetryImagePreview(PathBuf),
    PreviewDirectoryChildrenLoaded(PathBuf, Result<Vec<DirectoryEntry>, String>),
    TextPreviewContentScrolled {
        lines: i32,
        viewport_height: f32,
    },
    TextPreviewViewerScrolled {
        lines: i32,
        offset_y: f32,
        viewport_height: f32,
    },
    TextPreviewViewportSynced {
        offset_y: f32,
        viewport_height: f32,
    },
    TextPreviewContentHeightChanged(f32),
    TextPreviewChunkLoaded {
        path: PathBuf,
        generation: u64,
        start_offset: u64,
        outcome: Result<TextPreviewChunk, String>,
    },
    MarkdownPreviewScrolled {
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    },
    MarkdownPreviewModeSelected(MarkdownPreviewMode),
    ImagePreviewDimensionsLoaded(PathBuf, u64, Result<(u32, u32), String>),
    PreviewImageViewport(PreviewImageViewportMessage),
    AnimatedImageFrameLoaded(AnimatedImageFrame),
    AnimatedImagePreviewFinished(PathBuf, u64),
    AnimatedImagePreviewFailed(PathBuf, u64, String),
    AnimatedImageSeekRequested(f32),
    AnimatedImageSeekCommitted,
    AudioPreviewPlaybackToggled,
    AudioPreviewStarted(PathBuf, Result<AudioPreviewRuntime, String>),
    AudioPreviewSeekRequested(f32),
    AudioPreviewVolumeChanged(f32),
    AudioPreviewTick,
    VideoPreviewPlaybackToggled,
    VideoPreviewAudioStarted(PathBuf, u64, Result<AudioPreviewRuntime, String>),
    VideoPreviewMetadataLoaded(PathBuf, Result<Option<Duration>, String>),
    VideoPreviewSeekRequested(f32),
    VideoPreviewSeekCommitted,
    VideoPreviewVolumeChanged(f32),
    VideoPreviewTick,
    VideoPreviewFrameLoaded(VideoPreviewFrame),
    VideoPreviewSeekFrameFailed(PathBuf, u64, Duration, String),
    VideoPreviewFinished(PathBuf, u64),
    VideoPreviewFailed(PathBuf, u64, String),
    PreviewTreeDirectoryToggled(usize),
    PreviewTreeAnimationTick,
    RightPreviewPanelResizeStarted,
    RightPreviewPanelRatioResizeStarted,
    RightPreviewPanelInfoLoaded {
        path: PathBuf,
        snapshot: Result<Box<RightPreviewPanelInfoSnapshot>, String>,
    },
    SqliteTablesResizeStarted,
    ToggleRightPreviewPanel,
    PreviewWindowPinToggled,
    PreviewSizeLimitInputChanged(usize, String),
    PreviewSizeLimitInputCommitted(usize),
    PreviewDirectoryExpandLevelsInputChanged(String),
    PreviewDirectoryExpandLevelsInputCommitted,
    PreviewExtensionInputChanged(usize, String),
    PreviewExtensionInputCommitted(usize),
    PreviewExtensionExpandToggled(usize),
    PreviewExtensionRemoved(usize, String),
    PreviewExtensionResetRequested(usize),
    PreviewExtensionResetConfirmed(usize),
    PreviewWindowInitialChromeElapsed(u64),
    SqlitePreviewTablesScrolled,
    SqlitePreviewDataScrolled,
    PreviewDirectoryScrolled,
    PreviewArchiveScrolled,
}
