use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use file_core::{DirectoryEntry, FileKind};
use iced::widget::{image, svg};

use super::sqlite_preview::SqliteDatabasePreview;
use crate::animated_image_preview::AnimatedImagePreview;
use crate::audio_preview::AudioPreviewRuntime;
use crate::document_preview::PagedDocumentPreview;
use crate::operation_progress::active_byte_fraction;
use crate::text_preview::{TextPreviewFormat, TextPreviewLineLimitNotice};

#[derive(Debug, Clone)]
pub(crate) enum PreviewState {
    Loading(PathBuf),
    DownloadingRemoteFile(RemotePreviewDownload),
    Ready(PreviewContent),
    ImageError { path: PathBuf, error: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RemotePreviewDownload {
    pub(crate) source_path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: Option<u64>,
}

impl RemotePreviewDownload {
    pub(crate) fn new(source_path: PathBuf, generation: u64) -> Self {
        Self {
            source_path,
            generation,
            bytes_done: 0,
            bytes_total: None,
        }
    }

    pub(crate) fn accept_progress(&mut self, progress: &RemotePreviewCacheProgress) {
        match self.bytes_total {
            Some(bytes_total) if bytes_total == progress.bytes_total => {
                self.bytes_done = self.bytes_done.max(progress.bytes_done.min(bytes_total));
            }
            Some(_) => {}
            None => {
                self.bytes_done = progress.bytes_done.min(progress.bytes_total);
                self.bytes_total = Some(progress.bytes_total);
            }
        }
    }

    pub(crate) fn fraction(&self) -> Option<f32> {
        active_byte_fraction(self.bytes_done, self.bytes_total?)
    }
}

#[derive(Debug, Clone)]
pub(crate) enum RemotePreviewCacheMessage {
    Progress(RemotePreviewCacheProgress),
    Finished(RemotePreviewCacheFinished),
}

#[derive(Debug, Clone)]
pub(crate) struct RemotePreviewCacheProgress {
    pub(crate) source_path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct RemotePreviewCacheFinished {
    pub(crate) source_path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) outcome: Result<PathBuf, String>,
}

#[derive(Debug, Clone)]
pub(crate) enum PreviewContent {
    Directory {
        entries: Vec<PreviewTreeEntry>,
    },
    Text {
        path: PathBuf,
        rendered: Arc<str>,
        format: TextPreviewFormat,
        next_offset: Option<u64>,
        loaded_line_count: usize,
        line_limit_notice: Option<TextPreviewLineLimitNotice>,
    },
    Archive {
        entries: Vec<PreviewTreeEntry>,
    },
    PagedDocument(Box<PagedDocumentPreview>),
    Sqlite(SqliteDatabasePreview),
    Image(ImagePreviewContent),
    AnimatedImage(AnimatedImagePreview),
    Audio {
        path: PathBuf,
        duration: Option<Duration>,
        len: u64,
    },
    Video {
        path: PathBuf,
        frame: Option<image::Handle>,
        width: u32,
        height: u32,
        duration: Option<Duration>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ImagePreviewContent {
    Thumbnail {
        path: PathBuf,
        handle: image::Handle,
        width: u32,
        height: u32,
        max_edge: u32,
    },
    OriginalRaster {
        raster_handle: image::Handle,
        placeholder_handle: image::Handle,
        width: u32,
        height: u32,
    },
    OriginalSvg {
        handle: svg::Handle,
        width: u32,
        height: u32,
    },
}

impl ImagePreviewContent {
    /// 原始像素尺寸；缩略图形态取缩略图尺寸，fit 计算只看纵横比时二者等价。
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        match self {
            ImagePreviewContent::Thumbnail { width, height, .. } => (*width, *height),
            ImagePreviewContent::OriginalRaster { width, height, .. } => (*width, *height),
            ImagePreviewContent::OriginalSvg { width, height, .. } => (*width, *height),
        }
    }
}

/// 图片在预览媒体区内按纵横比缩放后的适应窗口（fit）显示尺寸。
/// 视图层定位与视口钳制共用这一个来源。
pub(crate) fn image_preview_size(size: PreviewSize, width: u32, height: u32) -> (f32, f32) {
    let max_width = size.width.max(1.0);
    let max_height = size.height.max(1.0);
    scaled_media_size(max_width, max_height, width, height)
}

pub(crate) fn scaled_media_size(
    max_width: f32,
    max_height: f32,
    width: u32,
    height: u32,
) -> (f32, f32) {
    if width == 0 || height == 0 {
        return (max_width, max_height);
    }

    let aspect_ratio = width as f32 / height as f32;
    let mut frame_width = max_width;
    let mut frame_height = frame_width / aspect_ratio;
    if frame_height > max_height {
        frame_height = max_height;
        frame_width = frame_height * aspect_ratio;
    }

    (frame_width.max(1.0), frame_height.max(1.0))
}

#[derive(Debug, Clone)]
pub(crate) struct AudioPreviewPlayback {
    pub(crate) path: PathBuf,
    pub(crate) runtime: Option<AudioPreviewRuntime>,
    pub(crate) status: AudioPreviewPlaybackStatus,
    pub(crate) position: Duration,
    pub(crate) volume: f32,
    pub(crate) error: Option<String>,
}

impl AudioPreviewPlayback {
    pub(crate) fn loading(path: PathBuf) -> Self {
        Self {
            path,
            runtime: None,
            status: AudioPreviewPlaybackStatus::Loading,
            position: Duration::ZERO,
            volume: 1.0,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioPreviewPlaybackStatus {
    Loading,
    Playing,
    Paused,
    Stopped,
    Finished,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct VideoPreviewPlayback {
    pub(crate) path: PathBuf,
    pub(crate) audio_runtime: Option<AudioPreviewRuntime>,
    pub(crate) status: VideoPreviewPlaybackStatus,
    pub(crate) position: Duration,
    // Subscription 的身份必须固定；播放进度 tick 只更新 position，不能重建 ffmpeg 流。
    pub(crate) stream_start_position: Duration,
    pub(crate) duration: Option<Duration>,
    pub(crate) volume: f32,
    pub(crate) generation: u64,
    pub(crate) seek_completion: Option<VideoPreviewSeekCompletion>,
    pub(crate) seek_frame_in_flight: Option<Duration>,
    pub(crate) pending_seek_frame: Option<Duration>,
    pub(crate) started_at: Option<Instant>,
    pub(crate) error: Option<String>,
}

impl VideoPreviewPlayback {
    pub(crate) fn playing(path: PathBuf, duration: Option<Duration>) -> Self {
        Self {
            path,
            audio_runtime: None,
            status: VideoPreviewPlaybackStatus::Playing,
            position: Duration::ZERO,
            stream_start_position: Duration::ZERO,
            duration,
            volume: 1.0,
            generation: 1,
            seek_completion: None,
            seek_frame_in_flight: None,
            pending_seek_frame: None,
            started_at: Some(Instant::now()),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPreviewSeekCompletion {
    ResumePlayback,
    StayPaused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPreviewPlaybackStatus {
    Playing,
    Paused,
    Finished,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct VideoPreviewFrame {
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) position: Duration,
    pub(crate) handle: image::Handle,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// 右侧面板信息区的单条目元数据快照:按目标路径异步读自文件系统,
/// 与目录列表快照解耦(列视图深层选中、搜索结果同样有完整元数据)。
#[derive(Debug, Clone)]
pub(crate) struct RightPreviewPanelInfoSnapshot {
    pub(crate) path: std::path::PathBuf,
    pub(crate) kind: file_core::FileKind,
    pub(crate) type_label: String,
    pub(crate) name: std::ffi::OsString,
    pub(crate) location: std::path::PathBuf,
    pub(crate) size_bytes: u64,
    pub(crate) created: Option<std::time::SystemTime>,
    pub(crate) modified: Option<std::time::SystemTime>,
    pub(crate) accessed: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewWindowProfile {
    Regular,
    Image,
    Audio,
    Video,
}
const PREVIEW_WINDOW_CHROME_REVEAL_DURATION: Duration = Duration::from_millis(160);
pub(crate) const PREVIEW_WINDOW_CHROME_HIDE_DURATION: Duration = Duration::from_millis(220);
pub(crate) const PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewWindowChromeTarget {
    Hidden,
    Visible,
}

impl PreviewWindowChromeTarget {
    fn from_cursor_y(cursor_y: f32) -> Self {
        if cursor_y <= PreviewWindowChromeState::REVEAL_HEIGHT {
            Self::Visible
        } else {
            Self::Hidden
        }
    }

    fn opacity(self) -> f32 {
        match self {
            Self::Hidden => 0.0,
            Self::Visible => 1.0,
        }
    }

    fn duration(self) -> Duration {
        match self {
            Self::Hidden => PREVIEW_WINDOW_CHROME_HIDE_DURATION,
            Self::Visible => PREVIEW_WINDOW_CHROME_REVEAL_DURATION,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PreviewWindowChromeAnimation {
    started_at: Instant,
    initial_opacity: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewWindowChromeState {
    target: PreviewWindowChromeTarget,
    opacity: f32,
    animation: Option<PreviewWindowChromeAnimation>,
}

impl Default for PreviewWindowChromeState {
    fn default() -> Self {
        Self {
            target: PreviewWindowChromeTarget::Hidden,
            opacity: 0.0,
            animation: None,
        }
    }
}

impl PreviewWindowChromeState {
    pub(crate) const REVEAL_HEIGHT: f32 = 64.0;

    pub(crate) fn start_reveal(&mut self) {
        self.retarget_at(PreviewWindowChromeTarget::Visible, Instant::now());
    }

    pub(crate) fn start_hide(&mut self) {
        self.retarget_at(PreviewWindowChromeTarget::Hidden, Instant::now());
    }

    pub(crate) fn update_for_cursor_y(&mut self, cursor_y: f32) {
        self.retarget_at(
            PreviewWindowChromeTarget::from_cursor_y(cursor_y),
            Instant::now(),
        );
    }
    pub(crate) fn update_for_bottom_cursor_y(&mut self, cursor_y: f32, window_height: f32) {
        let reveal_start = (window_height - Self::REVEAL_HEIGHT).max(0.0);
        let target = if cursor_y >= reveal_start && cursor_y <= window_height {
            PreviewWindowChromeTarget::Visible
        } else {
            PreviewWindowChromeTarget::Hidden
        };
        self.retarget_at(target, Instant::now());
    }

    pub(crate) fn reset_hidden(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub(crate) fn target_is_visible(self) -> bool {
        self.target == PreviewWindowChromeTarget::Visible
    }

    pub(crate) fn opacity(self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }

    pub(crate) fn is_animating(self) -> bool {
        self.animation.is_some()
    }

    pub(crate) fn advance(&mut self) {
        self.advance_at(Instant::now());
    }

    fn retarget_at(&mut self, target: PreviewWindowChromeTarget, now: Instant) {
        if self.target == target {
            return;
        }

        self.opacity = self.opacity_at(now);
        self.target = target;
        if (self.opacity - target.opacity()).abs() <= f32::EPSILON {
            self.animation = None;
        } else {
            self.animation = Some(PreviewWindowChromeAnimation {
                started_at: now,
                initial_opacity: self.opacity,
            });
        }
    }

    fn advance_at(&mut self, now: Instant) {
        let Some(animation) = self.animation else {
            return;
        };
        let progress = crate::animation::elapsed_fraction_at(
            animation.started_at,
            now,
            self.target.duration(),
        );
        self.opacity = animation.initial_opacity
            + (self.target.opacity() - animation.initial_opacity)
                * crate::animation::ease_out_cubic(progress);
        if progress >= 1.0 {
            self.opacity = self.target.opacity();
            self.animation = None;
        }
    }

    fn opacity_at(self, now: Instant) -> f32 {
        let Some(animation) = self.animation else {
            return self.opacity;
        };
        let progress = crate::animation::elapsed_fraction_at(
            animation.started_at,
            now,
            self.target.duration(),
        );
        animation.initial_opacity
            + (self.target.opacity() - animation.initial_opacity)
                * crate::animation::ease_out_cubic(progress)
    }
}

#[cfg(test)]
mod preview_window_chrome_tests {
    use super::*;

    #[test]
    fn cursor_y_targets_only_the_top_reveal_region() {
        assert_eq!(
            PreviewWindowChromeTarget::from_cursor_y(0.0),
            PreviewWindowChromeTarget::Visible
        );
        assert_eq!(
            PreviewWindowChromeTarget::from_cursor_y(64.0),
            PreviewWindowChromeTarget::Visible
        );
        assert_eq!(
            PreviewWindowChromeTarget::from_cursor_y(64.1),
            PreviewWindowChromeTarget::Hidden
        );
    }
    #[test]
    fn bottom_cursor_y_targets_only_the_bottom_reveal_region() {
        let mut chrome = PreviewWindowChromeState::default();

        chrome.update_for_bottom_cursor_y(636.0, 700.0);
        assert!(chrome.target_is_visible());

        chrome.update_for_bottom_cursor_y(635.9, 700.0);
        assert!(!chrome.target_is_visible());

        chrome.update_for_bottom_cursor_y(700.1, 700.0);
        assert!(!chrome.target_is_visible());
    }

    #[test]
    fn bottom_cursor_y_reveals_the_entire_small_window() {
        let mut chrome = PreviewWindowChromeState::default();

        chrome.update_for_bottom_cursor_y(0.0, 40.0);

        assert!(chrome.target_is_visible());
    }

    #[test]
    fn chrome_fades_in_and_out_over_the_configured_durations() {
        let started_at = Instant::now();
        let mut chrome = PreviewWindowChromeState::default();

        chrome.retarget_at(PreviewWindowChromeTarget::Visible, started_at);
        chrome.advance_at(started_at + Duration::from_millis(80));
        assert!(chrome.opacity() > 0.0 && chrome.opacity() < 1.0);
        chrome.advance_at(started_at + PREVIEW_WINDOW_CHROME_REVEAL_DURATION);
        assert_eq!(chrome.opacity(), 1.0);
        assert!(!chrome.is_animating());

        let hiding_at = started_at + PREVIEW_WINDOW_CHROME_REVEAL_DURATION;
        chrome.retarget_at(PreviewWindowChromeTarget::Hidden, hiding_at);
        chrome.advance_at(hiding_at + Duration::from_millis(110));
        assert!(chrome.opacity() > 0.0 && chrome.opacity() < 1.0);
        chrome.advance_at(hiding_at + PREVIEW_WINDOW_CHROME_HIDE_DURATION);
        assert_eq!(chrome.opacity(), 0.0);
        assert!(!chrome.is_animating());
    }

    #[test]
    fn reversing_a_fade_keeps_the_current_opacity() {
        let started_at = Instant::now();
        let mut chrome = PreviewWindowChromeState::default();
        chrome.retarget_at(PreviewWindowChromeTarget::Visible, started_at);

        let reversed_at = started_at + Duration::from_millis(80);
        chrome.advance_at(reversed_at);
        let opacity_before_reverse = chrome.opacity();
        chrome.retarget_at(PreviewWindowChromeTarget::Hidden, reversed_at);

        assert_eq!(chrome.opacity(), opacity_before_reverse);
        chrome.advance_at(reversed_at + Duration::from_millis(20));
        assert!(chrome.opacity() < opacity_before_reverse);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewTreeEntry {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) kind: FileKind,
    pub(crate) depth: usize,
    pub(crate) parent: Option<usize>,
    pub(crate) filesystem_path: Option<PathBuf>,
    pub(crate) directory_children: Option<PreviewTreeDirectoryChildren>,
    pub(crate) is_expanded: bool,
    pub(crate) toggle_rotation_progress: f32,
}

impl PreviewTreeEntry {
    pub(crate) fn from_directory_entry(
        id: usize,
        entry: DirectoryEntry,
        depth: usize,
        parent: Option<usize>,
    ) -> Self {
        let kind = entry.kind;
        Self {
            id,
            name: entry.name().to_string_lossy().into_owned(),
            kind,
            depth,
            parent,
            filesystem_path: Some(entry.path),
            directory_children: preview_tree_directory_children(kind),
            is_expanded: false,
            toggle_rotation_progress: 0.0,
        }
    }

    pub(crate) fn is_directory(&self) -> bool {
        self.kind == FileKind::Directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreviewTreeDirectoryChildren {
    Pending,
    Loading,
    Loaded,
    Error(String),
}

fn preview_tree_directory_children(kind: FileKind) -> Option<PreviewTreeDirectoryChildren> {
    (kind == FileKind::Directory).then_some(PreviewTreeDirectoryChildren::Pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(bytes_done: u64, bytes_total: u64) -> RemotePreviewCacheProgress {
        RemotePreviewCacheProgress {
            source_path: PathBuf::from("/remote/file.bin"),
            generation: 1,
            bytes_done,
            bytes_total,
        }
    }

    #[test]
    fn remote_preview_progress_is_unknown_until_total_arrives() {
        let download = RemotePreviewDownload::new(PathBuf::from("/remote/file.bin"), 1);

        assert_eq!(download.fraction(), None);
    }

    #[test]
    fn remote_preview_progress_keeps_a_stable_total_and_never_regresses() {
        let mut download = RemotePreviewDownload::new(PathBuf::from("/remote/file.bin"), 1);
        download.accept_progress(&progress(500, 1_000));
        download.accept_progress(&progress(250, 1_000));
        download.accept_progress(&progress(900, 2_000));

        assert_eq!(download.bytes_done, 500);
        assert_eq!(download.bytes_total, Some(1_000));
        assert_eq!(download.fraction(), Some(0.5));
    }

    #[test]
    fn active_empty_remote_preview_does_not_report_terminal_completion() {
        let mut download = RemotePreviewDownload::new(PathBuf::from("/remote/empty.bin"), 1);
        download.accept_progress(&progress(0, 0));

        assert_eq!(download.fraction(), None);
    }
}

