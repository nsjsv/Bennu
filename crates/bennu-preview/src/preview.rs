use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use file_core::{ExternalTool, FileKind};
use iced::widget::{image, svg};

// 原 app-ui 内 `use super::SqliteDatabasePreview`（model.rs 门面）；下沉后
// sqlite_preview 是本 crate 顶层模块，改为 crate 路径。
use crate::animated_image_preview::AnimatedImagePreview;
use crate::audio_preview::{AudioLyrics, AudioPreviewRuntime, AudioProbe, AudioWaveform};
use crate::document_preview::PagedDocumentPreview;
use crate::sqlite_preview::SqliteDatabasePreview;
use crate::text_document::TextOpened;
use crate::video_preview::{FramePlan, PlaybackClock, SubtitleBurn, SubtitleChoice, VideoProbe};
use crate::video_surface::VideoPicture;
use file_core::byte_progress::active_byte_fraction;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum PreviewState {
    Loading(PathBuf),
    DownloadingRemoteFile(RemotePreviewDownload),
    Ready(PreviewContent),
    ImageError { path: PathBuf, error: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct RemotePreviewDownload {
    pub source_path: PathBuf,
    pub generation: u64,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

impl RemotePreviewDownload {
    pub fn new(source_path: PathBuf, generation: u64) -> Self {
        Self {
            source_path,
            generation,
            bytes_done: 0,
            bytes_total: None,
        }
    }

    pub fn accept_progress(&mut self, progress: &RemotePreviewCacheProgress) {
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

    pub fn fraction(&self) -> Option<f32> {
        active_byte_fraction(self.bytes_done, self.bytes_total?)
    }
}

#[derive(Debug, Clone)]
pub enum RemotePreviewCacheMessage {
    Progress(RemotePreviewCacheProgress),
    Finished(RemotePreviewCacheFinished),
}

#[derive(Debug, Clone)]
pub struct RemotePreviewCacheProgress {
    pub source_path: PathBuf,
    pub generation: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone)]
pub struct RemotePreviewCacheFinished {
    pub source_path: PathBuf,
    pub generation: u64,
    pub outcome: Result<PathBuf, String>,
}

#[derive(Debug, Clone)]
pub enum PreviewContent {
    Folder(Box<crate::folder_preview::FolderPreview>),
    /// 打开的文本：句柄、解码方式、第一个窗口；引擎据此建按需读取的文档。
    Text {
        path: PathBuf,
        opened: Arc<TextOpened>,
    },
    Archive {
        entries: Vec<PreviewTreeEntry>,
        summary: ArchiveSummary,
        /// 滚到哪了（滚动回流写进来）：几万条只排看得见的行。
        viewport: TreeViewport,
    },
    PagedDocument(Box<PagedDocumentPreview>),
    Sqlite(SqliteDatabasePreview),
    Image(ImagePreviewContent),
    AnimatedImage(AnimatedImagePreview),
    Audio {
        path: PathBuf,
        // 装箱：标签、技术参数让它比其余内容大好几倍，整个枚举不该跟着变大。
        details: Box<AudioPreviewDetails>,
        /// 打开后在后台取；没有封面就一直是 None。
        cover: Option<image::Handle>,
        /// 同上，没有歌词就一直是 None。
        lyrics: Option<Arc<AudioLyrics>>,
    },
    /// 预览要靠的外部工具（音视频靠 ffmpeg，zip/7z/rar 靠 7-Zip）没装；面板给出安装命令。
    ToolRequired {
        path: PathBuf,
        tool: ExternalTool,
    },
    Video {
        path: PathBuf,
        details: Arc<VideoPreviewDetails>,
        /// 正在显示的那一帧；第一帧解出来之前是 None。
        frame: Option<Arc<VideoPicture>>,
    },
}

#[derive(Debug, Clone)]
pub enum ImagePreviewContent {
    Thumbnail {
        path: PathBuf,
        handle: image::Handle,
        width: u32,
        height: u32,
        max_edge: u32,
    },
    OriginalRaster {
        path: PathBuf,
        raster_handle: image::Handle,
        placeholder_handle: image::Handle,
        /// 转正后的原图大小。
        width: u32,
        height: u32,
        /// 解出来的像素大小：先按屏幕大小解，放大到超过它才换成原图。
        pixel_width: u32,
        has_alpha: bool,
    },
    OriginalSvg {
        path: PathBuf,
        handle: svg::Handle,
        width: u32,
        height: u32,
    },
}

/// 视图画图片要的引擎状态：缩放视口、开着的信息层、加载下一张时垫着的上一张。
#[derive(Debug, Clone, Copy)]
pub struct ImageView<'a> {
    pub viewport: &'a crate::image_preview_viewport::ImagePreviewViewport,
    /// 逻辑像素换物理像素的倍数（比例按原始像素算）。
    pub display_scale: f32,
    pub info: Option<&'a ImageInfo>,
    pub loading_backdrop: Option<&'a ImagePreviewContent>,
    /// 超大图放大后要画的块（切好了、正放大时才有）。
    pub zoom_tiles: Option<&'a crate::zoom_tiles::ZoomTiles>,
}

/// 图片信息层的内容（后台读文件头得到）；跟着路径走，换了图旧的就不显示。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub path: PathBuf,
    pub facts: file_core::image_decode::ImageFacts,
    pub byte_len: u64,
}

impl ImagePreviewContent {
    /// 显示的是哪个文件、按什么方式画（信息层、上一张/下一张用）。缩略图占位时还不算。
    pub fn shown_source(&self) -> Option<(&Path, file_core::ImageKind)> {
        match self {
            ImagePreviewContent::Thumbnail { .. } => None,
            ImagePreviewContent::OriginalRaster { path, .. } => Some((
                path,
                file_core::media_by_name(path).image_kind().unwrap_or(
                    file_core::ImageKind::Raster(file_core::RasterSource::Picture),
                ),
            )),
            ImagePreviewContent::OriginalSvg { path, .. } => {
                Some((path, file_core::ImageKind::Vector))
            }
        }
    }

    /// 原始像素尺寸；缩略图形态取缩略图尺寸，fit 计算只看纵横比时二者等价。
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            ImagePreviewContent::Thumbnail { width, height, .. } => (*width, *height),
            ImagePreviewContent::OriginalRaster { width, height, .. } => (*width, *height),
            ImagePreviewContent::OriginalSvg { width, height, .. } => (*width, *height),
        }
    }
}

/// 压缩包顶上那一行：多少文件、多少文件夹、解压后多大、包本身多大（算压缩率）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchiveSummary {
    pub files: usize,
    pub folders: usize,
    pub unpacked_bytes: u64,
    pub archive_bytes: u64,
}

/// 目录/压缩包树滚到哪：压缩包几万条只排看得见的那些行。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeViewport {
    pub offset: f32,
    pub height: f32,
}

impl Default for TreeViewport {
    /// 还没收到滚动回流时按一个高屏排，第一屏不缺行。
    fn default() -> Self {
        Self {
            offset: 0.0,
            height: 2_000.0,
        }
    }
}

/// 预览所在屏幕：物理像素大小（图片、视频按它定解多大）和缩放倍数（逻辑像素换物理像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayMetrics {
    pub pixels: iced::Size<u32>,
    pub scale_factor: f32,
}

/// 图片在预览媒体区内按纵横比缩放后的适应窗口（fit）显示尺寸。
/// 视图层定位与视口钳制共用这一个来源。
pub fn image_preview_size(size: PreviewSize, width: u32, height: u32) -> (f32, f32) {
    let max_width = size.width.max(1.0);
    let max_height = size.height.max(1.0);
    scaled_media_size(max_width, max_height, width, height)
}

pub fn scaled_media_size(max_width: f32, max_height: f32, width: u32, height: u32) -> (f32, f32) {
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

/// 打开音频预览时读到的信息；`probe.duration_is_estimate` 为真时打开后后台再量一次准确值替换。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioPreviewDetails {
    pub len: u64,
    pub probe: AudioProbe,
}

/// 打开时一次 ffprobe 读到的信息（解码参数、音轨字幕轨、信息层都用它）、文件大小和能选的字幕
/// （同目录同名外挂在前，然后是视频里的字幕轨）。
#[derive(Debug, Clone, PartialEq)]
pub struct VideoPreviewDetails {
    pub len: u64,
    pub probe: VideoProbe,
    pub subtitles: Vec<SubtitleChoice>,
}

/// 进度区显示哪一种图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioTimelineView {
    #[default]
    Waveform,
    Spectrogram,
}

#[derive(Debug, Clone)]
pub enum AudioAnalysisState<T> {
    /// 还没算：网络挂载上的波形等用户点「生成波形」，声谱图等用户切过去。
    NotRequested,
    Computing,
    Ready(T),
    Failed(String),
}

/// 当前这首的波形与声谱图。被替换或丢弃时取消还在算的 ffmpeg。
#[derive(Debug)]
pub struct AudioAnalysis {
    pub path: PathBuf,
    pub len: u64,
    pub view: AudioTimelineView,
    pub waveform: AudioAnalysisState<Arc<AudioWaveform>>,
    pub spectrogram: AudioAnalysisState<image::Handle>,
    cancel: AnalysisCancel,
}

impl AudioAnalysis {
    pub fn new(path: PathBuf, len: u64) -> Self {
        Self {
            path,
            len,
            view: AudioTimelineView::default(),
            waveform: AudioAnalysisState::NotRequested,
            spectrogram: AudioAnalysisState::NotRequested,
            cancel: AnalysisCancel(CancellationToken::new()),
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.0.clone()
    }
}

#[derive(Debug)]
struct AnalysisCancel(CancellationToken);

impl Drop for AnalysisCancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// 最近算过的几首（用户定的 3 首），只在内存里；键是路径加文件大小，文件被改写多半
/// 大小也变了。
#[derive(Debug, Default)]
pub struct AudioAnalysisCache {
    entries: VecDeque<CachedAudioAnalysis>,
}

#[derive(Debug)]
struct CachedAudioAnalysis {
    path: PathBuf,
    len: u64,
    waveform: Option<Arc<AudioWaveform>>,
    spectrogram: Option<image::Handle>,
}

const AUDIO_ANALYSIS_CACHE_SONGS: usize = 3;

impl AudioAnalysisCache {
    pub fn waveform(&self, path: &Path, len: u64) -> Option<Arc<AudioWaveform>> {
        self.find(path, len)?.waveform.clone()
    }

    pub fn spectrogram(&self, path: &Path, len: u64) -> Option<image::Handle> {
        self.find(path, len)?.spectrogram.clone()
    }

    pub fn store_waveform(&mut self, path: &Path, len: u64, waveform: Arc<AudioWaveform>) {
        self.entry(path, len).waveform = Some(waveform);
    }

    pub fn store_spectrogram(&mut self, path: &Path, len: u64, spectrogram: image::Handle) {
        self.entry(path, len).spectrogram = Some(spectrogram);
    }

    fn find(&self, path: &Path, len: u64) -> Option<&CachedAudioAnalysis> {
        self.entries
            .iter()
            .find(|entry| entry.path == path && entry.len == len)
    }

    /// 取出（或新建）这首的条目并挪到最前；超出数量丢最旧的。
    fn entry(&mut self, path: &Path, len: u64) -> &mut CachedAudioAnalysis {
        let existing = self
            .entries
            .iter()
            .position(|entry| entry.path == path && entry.len == len)
            .and_then(|index| self.entries.remove(index));
        let entry = existing.unwrap_or_else(|| CachedAudioAnalysis {
            path: path.to_path_buf(),
            len,
            waveform: None,
            spectrogram: None,
        });
        self.entries.push_front(entry);
        self.entries.truncate(AUDIO_ANALYSIS_CACHE_SONGS);
        self.entries.front_mut().expect("just inserted")
    }
}

#[derive(Debug, Clone)]
pub struct AudioPreviewPlayback {
    pub path: PathBuf,
    pub runtime: Option<AudioPreviewRuntime>,
    pub status: AudioPreviewPlaybackStatus,
    pub position: Duration,
    /// 正在拖进度条：显示拖到的位置，tick 不拿播放进度覆盖它，松手才真正跳转。
    pub seeking: bool,
    pub lyrics_browse: LyricsBrowse,
    pub error: Option<String>,
}

/// 用滚轮翻带时间轴的歌词：显示窗口偏离当前句几行，以及什么时候回到当前句。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LyricsBrowse {
    pub offset_lines: i32,
    pub resume_at: Option<Instant>,
}

impl AudioPreviewPlayback {
    pub fn loading(path: PathBuf) -> Self {
        Self::with_status(path, AudioPreviewPlaybackStatus::Loading)
    }

    pub fn ready(path: PathBuf) -> Self {
        Self::with_status(path, AudioPreviewPlaybackStatus::Ready)
    }

    fn with_status(path: PathBuf, status: AudioPreviewPlaybackStatus) -> Self {
        Self {
            path,
            runtime: None,
            status,
            position: Duration::ZERO,
            seeking: false,
            lyrics_browse: LyricsBrowse::default(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPreviewPlaybackStatus {
    /// 打开了但还没播（右侧面板不自动播放）；按播放从 `position` 开始。
    Ready,
    Loading,
    Playing,
    Paused,
    Stopped,
    Finished,
    Error,
}

/// 一次视频预览的播放状态。画面、声音、进度都跟 `clock` 走。
#[derive(Debug, Clone)]
pub struct VideoPreviewPlayback {
    pub path: PathBuf,
    pub status: VideoPreviewPlaybackStatus,
    /// 进度条和时间显示的位置；播放中由刷新节拍从时钟取。
    pub position: Duration,
    pub duration: Option<Duration>,
    pub clock: Arc<PlaybackClock>,
    /// 量好屏幕之后才有（出帧大小按屏幕定）。
    pub plan: Option<Arc<FramePlan>>,
    /// 当前这段连续解码的起点；暂停时这段还在（不重开），拖进度条、播完时没有。
    pub segment_start: Option<Duration>,
    /// 每换一段解码、每次拖动 +1：旧的帧、静止画面回来时认得出来丢掉。
    pub generation: u64,
    pub sound: VideoSound,
    /// 正在拖进度条。
    pub seek_drag: Option<VideoSeekDrag>,
    /// 拖动时的关键帧画面：同时只解一张，拖得快时只记最后的位置。
    pub still_in_flight: Option<Duration>,
    pub pending_still: Option<Duration>,
    /// 单击要等双击判定：待定那一下的编号。
    pub pending_click: Option<u64>,
    pub clicks: u64,
    /// 放第几条音轨（`-map 0:a:N`）。
    pub audio_track: usize,
    pub subtitle: VideoSubtitle,
    /// 控制条上展开着的字幕/音轨菜单。
    pub track_menu: Option<VideoTrackMenu>,
    /// 全屏时鼠标停了一会儿：画面上不显示鼠标。
    pub cursor_hidden: bool,
    /// 画面上盖着信息层（按钮或 I 键开关，只对这次预览有效）。
    pub info_visible: bool,
    /// 量好屏幕之后怎么开始。
    pub opening: VideoOpening,
    pub error: Option<String>,
}

impl VideoPreviewPlayback {
    /// 打开时：停在开头，还没量屏幕。
    pub fn ready(path: PathBuf, duration: Option<Duration>, has_audio: bool) -> Self {
        Self {
            path,
            status: VideoPreviewPlaybackStatus::Ready,
            position: Duration::ZERO,
            duration,
            clock: PlaybackClock::new(has_audio),
            plan: None,
            segment_start: None,
            generation: 1,
            sound: if has_audio {
                VideoSound::NotStarted
            } else {
                VideoSound::Silent
            },
            seek_drag: None,
            still_in_flight: None,
            pending_still: None,
            pending_click: None,
            clicks: 0,
            audio_track: 0,
            subtitle: VideoSubtitle::Off,
            track_menu: None,
            cursor_hidden: false,
            info_visible: false,
            opening: VideoOpening::ShowStill,
            error: None,
        }
    }

    pub fn audio_runtime(&self) -> Option<&AudioPreviewRuntime> {
        match &self.sound {
            VideoSound::Running(runtime) => Some(runtime),
            _ => None,
        }
    }
}

/// 视频的声音走到哪一步了。
#[derive(Debug, Clone)]
pub enum VideoSound {
    /// 文件没有音轨：时钟按墙钟走。
    Silent,
    /// 有音轨，还没开始放（右侧面板没点播放、跳转后暂停着）。
    NotStarted,
    Starting,
    Running(AudioPreviewRuntime),
    /// 声音起不来或放坏了：时钟改按墙钟，画面照放。
    Failed,
}

/// 字幕显示到哪一步了；序号指 `VideoPreviewDetails::subtitles`。
#[derive(Debug, Clone)]
pub enum VideoSubtitle {
    Off,
    /// 选了，正在后台准备（文字字幕要先抽成小文件）；换选择或预览结束时随句柄丢掉而取消。
    Preparing {
        choice: usize,
        work: iced::task::Handle,
    },
    Showing {
        choice: usize,
        burn: Arc<SubtitleBurn>,
    },
}

impl VideoSubtitle {
    pub fn choice(&self) -> Option<usize> {
        match self {
            VideoSubtitle::Off => None,
            VideoSubtitle::Preparing { choice, .. } | VideoSubtitle::Showing { choice, .. } => {
                Some(*choice)
            }
        }
    }
}

/// 量好屏幕之后：直接播（独立窗口、对话框、「播下一个」接上的），还是先显示静止画面、
/// 点了才播（右侧面板）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoOpening {
    Play,
    ShowStill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTrackMenu {
    Subtitles,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSeekDrag {
    /// 松手后接着播（按下时在播）。
    pub resume_after: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPreviewPlaybackStatus {
    /// 打开了还没播（右侧面板点了才播）；显示的是静止画面。
    Ready,
    Playing,
    Paused,
    Finished,
    Error,
}

#[derive(Debug, Clone)]
pub struct VideoPreviewFrame {
    pub path: PathBuf,
    pub generation: u64,
    pub position: Duration,
    pub picture: Arc<VideoPicture>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewWindowProfile {
    Regular,
    Image,
    Audio,
    Video,
}
const PREVIEW_WINDOW_CHROME_REVEAL_DURATION: Duration = Duration::from_millis(160);
pub const PREVIEW_WINDOW_CHROME_HIDE_DURATION: Duration = Duration::from_millis(220);
pub const PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewWindowChromeTarget {
    Hidden,
    Visible,
}

impl PreviewWindowChromeTarget {
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
/// 视频 / 动图底部播放控制条的淡入淡出：指针移到底部一带才出现，离开后淡出。
pub struct PreviewBottomControlsState {
    target: PreviewWindowChromeTarget,
    opacity: f32,
    animation: Option<PreviewWindowChromeAnimation>,
}

impl Default for PreviewBottomControlsState {
    fn default() -> Self {
        Self {
            target: PreviewWindowChromeTarget::Hidden,
            opacity: 0.0,
            animation: None,
        }
    }
}

impl PreviewBottomControlsState {
    pub const REVEAL_HEIGHT: f32 = 64.0;

    pub fn start_reveal(&mut self) {
        self.retarget_at(PreviewWindowChromeTarget::Visible, Instant::now());
    }

    pub fn start_hide(&mut self) {
        self.retarget_at(PreviewWindowChromeTarget::Hidden, Instant::now());
    }

    pub fn update_for_bottom_cursor_y(&mut self, cursor_y: f32, window_height: f32) {
        let reveal_start = (window_height - Self::REVEAL_HEIGHT).max(0.0);
        let target = if cursor_y >= reveal_start && cursor_y <= window_height {
            PreviewWindowChromeTarget::Visible
        } else {
            PreviewWindowChromeTarget::Hidden
        };
        self.retarget_at(target, Instant::now());
    }

    pub fn reset_hidden(&mut self) {
        *self = Self::default();
    }

    // 原 #[cfg(test)] 门控在跨 crate 后失效（app-ui 测试编译不会打开依赖
    // crate 的 cfg(test)），改为常驻 pub 供宿主测试断言 chrome 目标态。
    pub fn target_is_visible(self) -> bool {
        self.target == PreviewWindowChromeTarget::Visible
    }

    pub fn opacity(self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }

    pub fn is_animating(self) -> bool {
        self.animation.is_some()
    }

    pub fn advance(&mut self) {
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
    fn bottom_cursor_y_targets_only_the_bottom_reveal_region() {
        let mut chrome = PreviewBottomControlsState::default();

        chrome.update_for_bottom_cursor_y(636.0, 700.0);
        assert!(chrome.target_is_visible());

        chrome.update_for_bottom_cursor_y(635.9, 700.0);
        assert!(!chrome.target_is_visible());

        chrome.update_for_bottom_cursor_y(700.1, 700.0);
        assert!(!chrome.target_is_visible());
    }

    #[test]
    fn bottom_cursor_y_reveals_the_entire_small_window() {
        let mut chrome = PreviewBottomControlsState::default();

        chrome.update_for_bottom_cursor_y(0.0, 40.0);

        assert!(chrome.target_is_visible());
    }

    #[test]
    fn chrome_fades_in_and_out_over_the_configured_durations() {
        let started_at = Instant::now();
        let mut chrome = PreviewBottomControlsState::default();

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
        let mut chrome = PreviewBottomControlsState::default();
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
pub struct PreviewTreeEntry {
    pub id: usize,
    pub name: String,
    pub kind: FileKind,
    pub depth: usize,
    pub parent: Option<usize>,
    pub is_expanded: bool,
    pub toggle_rotation_progress: f32,
    /// 压缩包里的大小（文件夹是里面文件加起来）和修改时间。
    pub len: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

impl PreviewTreeEntry {
    pub fn is_directory(&self) -> bool {
        self.kind == FileKind::Directory
    }
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
