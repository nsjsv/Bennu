//! 预览子系统自有消息类型：两个宿主（app-ui / portal-backend）的顶层
//! Message 体系不同，共享状态机与命令层必须以本枚举为输出契约，宿主经
//! `Task::map` 包进各自 Message（iced 官方组合手段，见任务 design.md
//! 「PreviewMessage 独立枚举」）。只放两个宿主可能共有的预览消息；仅 app-ui
//! 设置窗口/停靠面板发出的输入域消息直接定义在 app-ui 的 Message 上。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use file_core::DirectoryEntry;
use iced::widget::image;
use iced::window;

use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePreview};
use crate::audio_preview::{AudioLyrics, AudioPreviewRuntime, AudioWaveform};
use crate::document_preview::DocumentPreviewMessage;
use crate::image_preview_viewport::PreviewImageViewportMessage;
use crate::original_image_preview::OriginalImagePreview;
use crate::preview::{
    AudioTimelineView, PreviewContent, RemotePreviewCacheMessage, RightPreviewPanelInfoSnapshot,
    VideoPreviewFrame,
};
use crate::sqlite_preview::SqlitePreviewMessage;
use crate::text_document::{TextReadReply, TextViewerEvent};
use crate::text_preview::MarkdownPreviewMode;

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
    /// 文本查看器的可见范围、跳转、超出窗口的复制。
    TextViewer(TextViewerEvent),
    /// 文本文档的后台读取回来了；`document` 不是当前文档就丢弃。
    TextRead {
        document: u64,
        reply: TextReadReply,
    },
    /// 预览中的文本文件可能变了（文件监视）。
    TextFileChanged {
        document: u64,
    },
    MarkdownPreviewModeSelected(MarkdownPreviewMode),
    /// 用户为当前文本预览指定编码；只对这次预览有效。
    TextPreviewEncodingSelected(&'static encoding_rs::Encoding),
    ImagePreviewDimensionsLoaded(PathBuf, u64, Result<(u32, u32), String>),
    PreviewImageViewport(PreviewImageViewportMessage),
    AnimatedImageFrameLoaded(AnimatedImageFrame),
    AnimatedImagePreviewFinished(PathBuf, u64),
    AnimatedImagePreviewFailed(PathBuf, u64, String),
    AnimatedImageSeekRequested(f32),
    AnimatedImageSeekCommitted,
    AudioPreviewPlaybackToggled,
    AudioPreviewStarted(PathBuf, Result<AudioPreviewRuntime, String>),
    /// 拖进度条途中：只改显示位置。
    AudioPreviewSeekRequested(f32),
    /// 松手（或键盘、点击这类一次到位的操作）：真正跳转。
    AudioPreviewSeekCommitted,
    /// ffprobe 只能估时长时，后台量出的准确时长。
    AudioPreviewDurationMeasured(PathBuf, Duration),
    AudioPreviewCoverLoaded(PathBuf, image::Handle),
    /// 「生成波形」按钮（网络挂载上的文件默认不算）。
    AudioWaveformRequested,
    AudioWaveformComputed(PathBuf, Result<Arc<AudioWaveform>, String>),
    AudioTimelineViewSelected(AudioTimelineView),
    AudioSpectrogramComputed(PathBuf, Result<image::Handle, String>),
    AudioLyricsLoaded(PathBuf, Arc<AudioLyrics>),
    /// 在带时间轴的歌词上滚滚轮：正数往后翻。
    AudioLyricsBrowsed(i32),
    /// 点了第几句（带时间轴的歌词），从这句开始放。
    AudioLyricsLineSelected(usize),
    CopyFfmpegInstallCommand,
    AudioPreviewTick,
    VideoPreviewPlaybackToggled,
    VideoPreviewAudioStarted(PathBuf, u64, Result<AudioPreviewRuntime, String>),
    VideoPreviewMetadataLoaded(PathBuf, Result<Option<Duration>, String>),
    VideoPreviewSeekRequested(f32),
    VideoPreviewSeekCommitted,
    /// 音频、视频共用的音量（0.0–1.0）；交给宿主记住后再由 `set_media_volume` 生效。
    MediaVolumeChanged(f32),
    /// 预览区聚焦时的播放快捷键，引擎分给当前的音频或视频。
    MediaKey(MediaKeyCommand),
    VideoPreviewTick,
    VideoPreviewFrameLoaded(VideoPreviewFrame),
    VideoPreviewSeekFrameFailed(PathBuf, u64, Duration, String),
    VideoPreviewFinished(PathBuf, u64),
    VideoPreviewFailed(PathBuf, u64, String),
    PreviewTreeDirectoryToggled(usize),
    PreviewTreeAnimationTick,
    RightPreviewPanelInfoLoaded {
        path: PathBuf,
        snapshot: Result<Box<RightPreviewPanelInfoSnapshot>, String>,
    },
    SqliteTablesResizeStarted,
    PreviewWindowPinToggled,
    /// 引擎呈现（开启或复用）预览窗口时随任务发出：宿主的焦点簿记
    /// （focused_window）不归引擎所有，由宿主认领。
    PreviewWindowPresented(window::Id),
    PreviewWindowInitialControlsElapsed(u64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaKeyCommand {
    TogglePlayback,
    SeekForward(Duration),
    SeekBackward(Duration),
    /// 音量增减量（正数调大）。
    ChangeVolume(f32),
}
