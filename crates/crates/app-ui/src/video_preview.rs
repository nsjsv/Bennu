use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;
use iced::widget::image;
use iced::Subscription;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command as TokioCommand;
use tokio::time;

use crate::model::{Message, VideoPreviewFrame};

pub(crate) const VIDEO_PREVIEW_MAX_EDGE: u32 = 720;
const VIDEO_PREVIEW_FPS: u32 = 15;
const VIDEO_PREVIEW_STDERR_LIMIT: usize = 16 * 1024;

pub(crate) struct VideoPreviewMetadata {
    pub(crate) duration: Option<Duration>,
}

pub(crate) async fn inspect_video_preview_metadata(
    path: PathBuf,
) -> Result<VideoPreviewMetadata, String> {
    tokio::task::spawn_blocking(move || inspect_video_preview_metadata_blocking(path.as_path()))
        .await
        .map_err(|error| format!("could not inspect video preview: {error}"))?
}

fn inspect_video_preview_metadata_blocking(path: &Path) -> Result<VideoPreviewMetadata, String> {
    Ok(VideoPreviewMetadata {
        duration: ffprobe_video_duration(path),
    })
}

fn ffprobe_video_duration(path: &Path) -> Option<Duration> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let seconds = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f32>()
        .ok()?;
    if seconds.is_finite() && seconds > 0.0 {
        Some(Duration::from_secs_f32(seconds))
    } else {
        None
    }
}

pub(crate) fn video_preview_subscription(
    path: PathBuf,
    generation: u64,
    start_position: Duration,
) -> Subscription<Message> {
    Subscription::run_with(
        VideoPreviewStream {
            path,
            generation,
            start_position,
        },
        video_preview_stream,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VideoPreviewStream {
    path: PathBuf,
    generation: u64,
    start_position: Duration,
}

fn video_preview_stream(
    stream: &VideoPreviewStream,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let path = stream.path.clone();
    let generation = stream.generation;
    let start_position = stream.start_position;
    iced::stream::channel(2, async move |mut output| {
        let outcome =
            stream_video_preview(path.clone(), generation, start_position, &mut output).await;
        let message = match outcome {
            Ok(()) => Message::VideoPreviewFinished(path.clone(), generation),
            Err(error) => Message::VideoPreviewFailed(path.clone(), generation, error),
        };
        let _ = output.send(message).await;
        iced::futures::future::pending().await
    })
}

pub(crate) async fn load_video_preview_frame(
    path: PathBuf,
    generation: u64,
    position: Duration,
) -> Result<VideoPreviewFrame, String> {
    let frame = decode_video_preview_frame(path.as_path(), position).await?;
    Ok(VideoPreviewFrame {
        path,
        generation,
        position,
        handle: image::Handle::from_rgba(frame.width, frame.height, frame.pixels),
        width: frame.width,
        height: frame.height,
    })
}

async fn decode_video_preview_frame(path: &Path, position: Duration) -> Result<PpmFrame, String> {
    let mut command = TokioCommand::new("ffmpeg");
    command
        .kill_on_drop(true)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error");
    if position > Duration::ZERO {
        command.arg("-ss").arg(ffmpeg_position(position));
    }
    let mut child = command
        .arg("-i")
        .arg(path)
        .arg("-an")
        .arg("-vf")
        .arg(video_seek_frame_filter())
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg("ppm")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| video_preview_spawn_error(path, error))?;

    let Some(mut stdout) = child.stdout.take() else {
        return Err("could not read video preview seek frame from ffmpeg".to_owned());
    };
    let stderr_task = child.stderr.take().map(limited_ffmpeg_stderr_task);
    let frame = read_ppm_frame(&mut stdout).await?;
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not finish video preview seek: {error}"))?;
    let stderr = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };

    if let Some(frame) = frame {
        return Ok(frame);
    }
    if status.success() {
        return Err("could not decode video preview frame at selected position".to_owned());
    }
    Err(video_preview_failure_message(status, &stderr))
}

async fn stream_video_preview(
    path: PathBuf,
    generation: u64,
    start_position: Duration,
    output: &mut IcedSender<Message>,
) -> Result<(), String> {
    let mut command = TokioCommand::new("ffmpeg");
    command
        .kill_on_drop(true)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error");
    if start_position > Duration::ZERO {
        command.arg("-ss").arg(ffmpeg_position(start_position));
    }
    let mut child = command
        .arg("-i")
        .arg(&path)
        .arg("-an")
        .arg("-vf")
        .arg(video_stream_filter())
        .arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg("ppm")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| video_preview_spawn_error(&path, error))?;

    let Some(mut stdout) = child.stdout.take() else {
        return Err("could not read video preview frames from ffmpeg".to_owned());
    };
    let stderr_task = child.stderr.take().map(limited_ffmpeg_stderr_task);

    let mut sent_any_frame = false;
    let stream_started_at = time::Instant::now();
    let mut frame_index = 0_u64;
    while let Some(frame) = read_ppm_frame(&mut stdout).await? {
        sent_any_frame = true;
        let position = start_position + video_stream_frame_offset(frame_index);
        output
            .send(Message::VideoPreviewFrameLoaded(VideoPreviewFrame {
                path: path.clone(),
                generation,
                position,
                handle: image::Handle::from_rgba(frame.width, frame.height, frame.pixels),
                width: frame.width,
                height: frame.height,
            }))
            .await
            .map_err(|_| "video preview window was closed".to_owned())?;
        frame_index = frame_index.saturating_add(1);
        time::sleep_until(stream_started_at + video_stream_frame_offset(frame_index)).await;
    }

    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not finish video preview: {error}"))?;
    let stderr = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    if status.success() || sent_any_frame {
        return Ok(());
    }

    Err(video_preview_failure_message(status, &stderr))
}

fn limited_ffmpeg_stderr_task<R>(mut stderr: R) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes).await;
        if bytes.len() > VIDEO_PREVIEW_STDERR_LIMIT {
            bytes.split_off(bytes.len() - VIDEO_PREVIEW_STDERR_LIMIT)
        } else {
            bytes
        }
    })
}

fn ffmpeg_position(position: Duration) -> String {
    format!("{:.3}", position.as_secs_f64())
}

fn video_stream_frame_offset(frame_index: u64) -> Duration {
    Duration::from_secs_f64(frame_index as f64 / VIDEO_PREVIEW_FPS as f64)
}

fn video_stream_filter() -> String {
    format!(
        "fps={VIDEO_PREVIEW_FPS},scale={VIDEO_PREVIEW_MAX_EDGE}:{VIDEO_PREVIEW_MAX_EDGE}:force_original_aspect_ratio=decrease,format=rgb24"
    )
}

fn video_seek_frame_filter() -> String {
    format!(
        "scale={VIDEO_PREVIEW_MAX_EDGE}:{VIDEO_PREVIEW_MAX_EDGE}:force_original_aspect_ratio=decrease,format=rgb24"
    )
}

fn video_preview_spawn_error(path: &Path, error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        return format!("Install ffmpeg to preview video files: {path:?}");
    }
    format!("could not start video preview for {path:?}: {error}")
}

fn video_preview_failure_message(status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    if !stderr.is_empty() {
        return format!("could not decode video preview: {stderr}");
    }
    format!("could not decode video preview: exit status {status}")
}

struct PpmFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

async fn read_ppm_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<PpmFrame>, String> {
    let Some(magic) = read_ppm_token(reader).await? else {
        return Ok(None);
    };
    if magic != "P6" {
        return Err("video preview decoder returned an unsupported frame format".to_owned());
    }

    let width = read_required_ppm_number(reader, "width").await?;
    let height = read_required_ppm_number(reader, "height").await?;
    let max_value = read_required_ppm_number(reader, "color depth").await?;
    if max_value != 255 {
        return Err("video preview decoder returned unsupported color depth".to_owned());
    }

    let rgb_len = frame_rgb_len(width, height)?;
    let mut rgb = vec![0; rgb_len];
    reader
        .read_exact(&mut rgb)
        .await
        .map_err(|error| format!("could not read video preview frame: {error}"))?;

    Ok(Some(PpmFrame {
        width,
        height,
        pixels: rgb_to_rgba(rgb),
    }))
}

async fn read_required_ppm_number<R: AsyncRead + Unpin>(
    reader: &mut R,
    label: &str,
) -> Result<u32, String> {
    let token = read_ppm_token(reader)
        .await?
        .ok_or_else(|| format!("missing video preview frame {label}"))?;
    token
        .parse::<u32>()
        .map_err(|_| format!("invalid video preview frame {label}: {token}"))
}

async fn read_ppm_token<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<String>, String> {
    let mut token = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let read = reader
            .read(&mut byte)
            .await
            .map_err(|error| format!("could not read video preview frame header: {error}"))?;
        if read == 0 {
            return if token.is_empty() {
                Ok(None)
            } else {
                Err("truncated video preview frame header".to_owned())
            };
        }

        if token.is_empty() && byte[0] == b'#' {
            skip_ppm_comment(reader).await?;
            continue;
        }

        if byte[0].is_ascii_whitespace() {
            if token.is_empty() {
                continue;
            }
            break;
        }

        token.push(byte[0]);
    }

    String::from_utf8(token)
        .map(Some)
        .map_err(|_| "invalid video preview frame header".to_owned())
}

async fn skip_ppm_comment<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), String> {
    let mut byte = [0_u8; 1];
    loop {
        let read = reader
            .read(&mut byte)
            .await
            .map_err(|error| format!("could not skip video preview frame comment: {error}"))?;
        if read == 0 || byte[0] == b'\n' {
            return Ok(());
        }
    }
}

fn frame_rgb_len(width: u32, height: u32) -> Result<usize, String> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .map(|len| len as usize)
        .ok_or_else(|| "video preview frame is too large".to_owned())
}

fn rgb_to_rgba(rgb: Vec<u8>) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for pixel in rgb.chunks_exact(3) {
        rgba.extend_from_slice(pixel);
        rgba.push(255);
    }
    rgba
}
