//! 视频预览的解码：ffprobe 打开时读一次信息（`probe`），ffmpeg 按屏幕大小出 YUV 帧
//! （`frames`），画面交给 `video_surface` 在显卡上换颜色、缩放。

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::{SinkExt, Stream};
use iced::Subscription;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time;

use crate::preview::VideoPreviewFrame;
use crate::preview_message::PreviewMessage;
use crate::video_surface::VideoPicture;

mod clock;
mod frames;
mod hwaccel;
mod probe;
mod subtitles;
#[cfg(test)]
pub(crate) mod test_fixtures;

pub use clock::PlaybackClock;
pub use frames::FramePlan;
pub(crate) use frames::{SeekPrecision, FALLBACK_DISPLAY};
pub(crate) use hwaccel::start_detection as start_hwaccel_detection;
pub use probe::{
    AudioTrack, SubtitleTrack, VideoColor, VideoPrimaries, VideoProbe, VideoProbeError,
    VideoStreamFacts, VideoTransfer, YuvMatrix,
};
pub(crate) use subtitles::{
    default_subtitle, find_external_subtitles, prepare_subtitle, subtitle_choices,
};
pub use subtitles::{ExternalSubtitle, SubtitleBurn, SubtitleChoice};

/// ffmpeg 出错时只留 stderr 末尾这么多字节，防止管道写满把它卡死。
const STDERR_TAIL_BYTES: usize = 16 * 1024;
/// 等下一帧时最多睡这么久就再看一眼时钟：暂停、声音卡顿都能及时反应。
const MAX_WAIT_SLICE: Duration = Duration::from_millis(50);

pub async fn probe_video_preview(path: PathBuf) -> Result<VideoProbe, VideoProbeError> {
    tokio::task::spawn_blocking(move || probe::probe_video_blocking(&path))
        .await
        .map_err(|error| VideoProbeError::Failed(format!("could not inspect video: {error}")))?
}

/// `position` 处的一帧（静止画面、拖动时的预览）。
pub(crate) async fn load_video_still(
    path: PathBuf,
    plan: Arc<FramePlan>,
    position: Duration,
    precision: SeekPrecision,
) -> Result<Arc<VideoPicture>, String> {
    let mut child = frames::still_command(&path, &plan, position, precision)
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not read the video frame from ffmpeg".to_owned())?;
    let stderr_tail = child.stderr.take().map(stderr_tail_task);
    let picture = frames::read_picture(&mut stdout, &plan)
        .await
        .map_err(|error| format!("could not read the video frame: {error}"))?;
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not finish ffmpeg: {error}"))?;
    match picture {
        Some(picture) => Ok(Arc::new(picture)),
        None => Err(ffmpeg_failure(status, stderr_tail).await),
    }
}

/// 一段连续解码：从 `start` 起，直到播完或被跳转换掉。暂停不结束这一段。
#[derive(Debug, Clone)]
pub struct VideoSegment {
    pub path: PathBuf,
    pub plan: Arc<FramePlan>,
    pub clock: Arc<PlaybackClock>,
    pub generation: u64,
    pub start: Duration,
    /// 要烧进画面的字幕；换字幕会开新的一段（代数变），不参与订阅身份。
    pub subtitle: Option<Arc<SubtitleBurn>>,
}

// 订阅身份是路径 + 代数 + 起点：暂停、继续、进度刷新都不重建 ffmpeg。
impl Hash for VideoSegment {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.generation.hash(state);
        self.start.hash(state);
    }
}

/// 帧流订阅：第一帧马上送（跳转后暂停着也能看到那一帧），之后按时钟到点再送。
pub fn video_preview_subscription(segment: VideoSegment) -> Subscription<PreviewMessage> {
    Subscription::run_with(segment, video_stream)
}

fn video_stream(segment: &VideoSegment) -> impl Stream<Item = PreviewMessage> + 'static {
    let segment = segment.clone();
    iced::stream::channel(2, async move |mut output| {
        let path = segment.path.clone();
        let generation = segment.generation;
        let message = match stream_frames(&segment, &mut output).await {
            Ok(()) => PreviewMessage::VideoPreviewFinished(path, generation),
            Err(error) => PreviewMessage::VideoPreviewFailed(path, generation, error),
        };
        let _ = output.send(message).await;
        iced::futures::future::pending().await
    })
}

async fn stream_frames(
    segment: &VideoSegment,
    output: &mut IcedSender<PreviewMessage>,
) -> Result<(), String> {
    let VideoSegment {
        path,
        plan,
        clock,
        generation,
        start,
        subtitle,
    } = segment;
    let hwaccel = hwaccel::render_node();
    let mut child = frames::stream_command(path, plan, *start, hwaccel, subtitle.as_deref())
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not read video frames from ffmpeg".to_owned())?;
    let stderr_tail = child.stderr.take().map(stderr_tail_task);

    let frame_duration = plan.frame_offset(1);
    let mut index = 0_u64;
    while let Some(picture) = frames::read_picture(&mut stdout, plan)
        .await
        .map_err(|error| format!("could not read video frames: {error}"))?
    {
        let due = *start + plan.frame_offset(index);
        index += 1;
        if index > 1 && !wait_until_due(clock, due, frame_duration).await {
            continue;
        }
        output
            .send(PreviewMessage::VideoPreviewFrameLoaded(VideoPreviewFrame {
                path: path.clone(),
                generation: *generation,
                position: due,
                picture: Arc::new(picture),
            }))
            .await
            .map_err(|_| "video preview window was closed".to_owned())?;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not finish ffmpeg: {error}"))?;
    if status.success() || index > 0 {
        return Ok(());
    }
    Err(ffmpeg_failure(status, stderr_tail).await)
}

/// 等到时钟走到 `due`；已经晚了一帧以上就返回 false，这帧不显示（解码跟不上时声音照常，
/// 画面掉帧，而不是越拖越慢）。
async fn wait_until_due(clock: &PlaybackClock, due: Duration, frame_duration: Duration) -> bool {
    loop {
        clock.wait_until_running().await;
        let now = clock.position();
        if now >= due {
            return now < due + frame_duration;
        }
        time::sleep((due - now).min(MAX_WAIT_SLICE)).await;
    }
}

fn stderr_tail_task<R>(mut stderr: R) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes).await;
        let keep_from = bytes.len().saturating_sub(STDERR_TAIL_BYTES);
        bytes.split_off(keep_from)
    })
}

async fn ffmpeg_failure(
    status: std::process::ExitStatus,
    stderr_tail: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> String {
    let stderr = match stderr_tail {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = String::from_utf8_lossy(&stderr);
    match stderr.lines().rev().find(|line| !line.trim().is_empty()) {
        Some(line) => format!("could not decode the video: {}", line.trim()),
        None => format!("could not decode the video: {status}"),
    }
}

#[cfg(test)]
#[path = "video_preview/stream_tests.rs"]
mod stream_tests;
