use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;
use iced::widget::image as iced_image;
use iced::Subscription;
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageDecoder};
use tokio::time;

use crate::model::Message;

const MIN_ANIMATED_IMAGE_FRAME_DELAY: Duration = Duration::from_millis(10);
const ANIMATED_IMAGE_PREVIEW_DURATION_FRAME_LIMIT: usize = 20_000;
const ANIMATED_IMAGE_PREVIEW_MAX_RGBA_BYTES: usize = 128 * 1024 * 1024;
const ANIMATED_IMAGE_DECODE_QUEUE_CAPACITY: usize = 2;

#[derive(Debug, Clone)]
pub(crate) struct AnimatedImageFrame {
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) position: Duration,
    pub(crate) delay: Duration,
    pub(crate) handle: iced_image::Handle,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct AnimatedImagePreview {
    path: PathBuf,
    current_frame: iced_image::Handle,
    previous_frame: Option<iced_image::Handle>,
    generation: u64,
    position: Duration,
    stream_start_position: Duration,
    duration: Option<Duration>,
    playback: AnimatedImagePlayback,
    is_seeking: bool,
    is_finished: bool,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnimatedImagePlayback {
    Static,
    Animated,
}

impl AnimatedImagePreview {
    pub(crate) fn new(
        path: PathBuf,
        first_frame: AnimatedImageFrame,
        generation: u64,
        duration: Option<Duration>,
        playback: AnimatedImagePlayback,
    ) -> Result<Self, String> {
        if first_frame.width == 0 || first_frame.height == 0 {
            return Err("Animated image preview has invalid dimensions".to_owned());
        }

        Ok(Self {
            path,
            current_frame: first_frame.handle,
            previous_frame: None,
            generation,
            position: first_frame.position,
            stream_start_position: first_frame.position,
            duration,
            playback,
            is_seeking: false,
            is_finished: false,
            width: first_frame.width,
            height: first_frame.height,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn current_frame_handle(&self) -> &iced_image::Handle {
        &self.current_frame
    }

    pub(crate) fn previous_frame_handle(&self) -> Option<&iced_image::Handle> {
        self.previous_frame.as_ref()
    }

    pub(crate) fn playback_position(&self) -> Duration {
        self.position
    }

    pub(crate) fn stream_start_position(&self) -> Duration {
        self.stream_start_position
    }

    pub(crate) fn playback_duration(&self) -> Option<Duration> {
        self.duration
    }

    pub(crate) fn is_playing(&self) -> bool {
        self.playback == AnimatedImagePlayback::Animated && !self.is_seeking && !self.is_finished
    }
    pub(crate) fn is_seeking(&self) -> bool {
        self.is_seeking
    }

    pub(crate) fn accept_frame(&mut self, frame: AnimatedImageFrame) {
        self.previous_frame = Some(self.current_frame.clone());
        self.current_frame = frame.handle;
        self.position = frame.position;
        self.width = frame.width;
        self.height = frame.height;
        self.is_finished = false;
    }

    pub(crate) fn seek_to_position(&mut self, position: Duration) {
        let position = self
            .duration
            .map(|duration| position.min(duration))
            .unwrap_or(position);
        self.position = position;
        self.is_seeking = true;
        self.is_finished = false;
    }
    pub(crate) fn commit_seek(&mut self, generation: u64) {
        self.generation = generation;
        self.stream_start_position = self.position;
        self.previous_frame = None;
        self.is_seeking = false;
        self.is_finished = false;
    }

    pub(crate) fn finish(&mut self) {
        self.is_finished = true;
        if let Some(duration) = self.duration {
            self.position = duration;
        }
    }
}

pub(crate) fn is_animated_image_preview_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
}

pub(crate) async fn load_animated_image_preview(
    path: PathBuf,
    generation: u64,
    max_file_bytes: u64,
) -> Result<AnimatedImagePreview, String> {
    tokio::task::spawn_blocking(move || {
        decode_animated_image_first_frame(path, generation, max_file_bytes)
    })
    .await
    .map_err(|error| format!("could not decode animated image preview: {error}"))?
}

pub(crate) fn animated_image_preview_subscription(
    path: PathBuf,
    generation: u64,
    start_position: Duration,
) -> Subscription<Message> {
    Subscription::run_with(
        AnimatedImagePreviewStream {
            path,
            generation,
            start_position,
        },
        animated_image_preview_stream,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AnimatedImagePreviewStream {
    path: PathBuf,
    generation: u64,
    start_position: Duration,
}

fn animated_image_preview_stream(
    stream: &AnimatedImagePreviewStream,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let path = stream.path.clone();
    let generation = stream.generation;
    let start_position = stream.start_position;
    iced::stream::channel(2, async move |mut output| {
        let outcome =
            stream_animated_image_preview(path.clone(), generation, start_position, &mut output)
                .await;
        let message = match outcome {
            Ok(()) => Message::AnimatedImagePreviewFinished(path.clone(), generation),
            Err(error) => Message::AnimatedImagePreviewFailed(path.clone(), generation, error),
        };
        let _ = output.send(message).await;
        iced::futures::future::pending().await
    })
}

fn decode_animated_image_first_frame(
    path: PathBuf,
    generation: u64,
    max_file_bytes: u64,
) -> Result<AnimatedImagePreview, String> {
    if max_file_bytes != 0 {
        let byte_len = std::fs::metadata(&path)
            .map_err(|error| format!("could not inspect animated image {path:?}: {error}"))?
            .len();
        if byte_len > max_file_bytes {
            return Err(format!(
                "File is too large to preview ({}). Maximum preview size is {}.",
                crate::formatting::format_file_size(byte_len),
                crate::formatting::format_file_size(max_file_bytes)
            ));
        }
    }
    let metadata = inspect_animated_image_metadata(path.as_path())?;
    let first_frame = decode_animated_image_frame_at(path.as_path(), generation, Duration::ZERO)?;
    AnimatedImagePreview::new(
        path,
        first_frame,
        generation,
        metadata.duration,
        metadata.playback,
    )
}

fn animated_image_frame_deadline(
    started_at: time::Instant,
    cycle_start_position: Duration,
    frame: &AnimatedImageFrame,
) -> time::Instant {
    started_at + frame.position.saturating_sub(cycle_start_position) + frame.delay
}

async fn stream_animated_image_preview(
    path: PathBuf,
    generation: u64,
    start_position: Duration,
    output: &mut IcedSender<Message>,
) -> Result<(), String> {
    let mut cycle_start_position = start_position;
    loop {
        let (frame_sender, mut frame_receiver) =
            tokio::sync::mpsc::channel(ANIMATED_IMAGE_DECODE_QUEUE_CAPACITY);
        let decoder_task = tokio::task::spawn_blocking({
            let path = path.clone();
            move || {
                decode_animated_image_frames_into_channel(
                    path.as_path(),
                    generation,
                    cycle_start_position,
                    frame_sender,
                )
            }
        });

        let started_at = time::Instant::now();
        let mut sent_any_frame = false;
        let mut last_frame_deadline = None;
        while let Some(frame) = frame_receiver.recv().await {
            sent_any_frame = true;
            let frame_started_at = started_at + frame.position.saturating_sub(cycle_start_position);
            let frame_deadline =
                animated_image_frame_deadline(started_at, cycle_start_position, &frame);

            time::sleep_until(frame_started_at).await;
            output
                .send(Message::AnimatedImageFrameLoaded(frame))
                .await
                .map_err(|_| "animated image preview window was closed".to_owned())?;
            last_frame_deadline = Some(frame_deadline);
        }

        decoder_task
            .await
            .map_err(|error| format!("could not decode animated image preview: {error}"))??;

        if let Some(deadline) = last_frame_deadline {
            time::sleep_until(deadline).await;
        }
        if !sent_any_frame {
            return Ok(());
        }
        cycle_start_position = Duration::ZERO;
    }
}

fn decode_animated_image_frame_at(
    path: &Path,
    generation: u64,
    position: Duration,
) -> Result<AnimatedImageFrame, String> {
    decode_first_animated_image_frame_at(path, generation, position)
}

fn decode_first_animated_image_frame_at(
    path: &Path,
    generation: u64,
    start_position: Duration,
) -> Result<AnimatedImageFrame, String> {
    let file = File::open(path)
        .map_err(|error| format!("could not open animated image preview: {error}"))?;
    let decoder = GifDecoder::new(BufReader::new(file))
        .map_err(|error| format!("could not decode animated image preview: {error}"))?;
    let (canvas_width, canvas_height) = decoder.dimensions();
    validate_animated_image_frame_decode_budget(canvas_width, canvas_height)?;

    let mut position = Duration::ZERO;
    let mut first_frame = None;
    for decoded_frame in decoder.into_frames() {
        let decoded_frame = decoded_frame
            .map_err(|error| format!("could not decode animated image frames: {error}"))?;
        let delay = normalized_frame_delay(decoded_frame.delay());
        let frame_position = position;
        position += delay;
        if frame_position < start_position {
            continue;
        }

        first_frame = Some(AnimatedImageFrame::from_decoded_frame(
            path.to_path_buf(),
            generation,
            frame_position,
            delay,
            decoded_frame,
        )?);
        break;
    }

    if first_frame.is_none() && start_position > Duration::ZERO {
        return decode_first_animated_image_frame_at(path, generation, Duration::ZERO);
    }

    first_frame.ok_or_else(|| "Animated image preview has no frames".to_owned())
}

fn decode_animated_image_frames_into_channel(
    path: &Path,
    generation: u64,
    start_position: Duration,
    frame_sender: tokio::sync::mpsc::Sender<AnimatedImageFrame>,
) -> Result<(), String> {
    let file = File::open(path)
        .map_err(|error| format!("could not open animated image preview: {error}"))?;
    let decoder = GifDecoder::new(BufReader::new(file))
        .map_err(|error| format!("could not decode animated image preview: {error}"))?;
    let (canvas_width, canvas_height) = decoder.dimensions();
    validate_animated_image_frame_decode_budget(canvas_width, canvas_height)?;

    let mut position = Duration::ZERO;
    let mut sent_any_frame = false;
    for decoded_frame in decoder.into_frames() {
        let decoded_frame = decoded_frame
            .map_err(|error| format!("could not decode animated image frames: {error}"))?;
        let delay = normalized_frame_delay(decoded_frame.delay());
        let frame_position = position;
        position += delay;
        if frame_position < start_position {
            if frame_sender.is_closed() {
                // 接收端已随旧 generation 丢弃：seek 叠加出的旧任务立即停止前缀解码，
                // 不再把整段前缀扫完，消除重复 seek 的 CPU 与内存叠加。
                return Ok(());
            }
            continue;
        }

        let frame = AnimatedImageFrame::from_decoded_frame(
            path.to_path_buf(),
            generation,
            frame_position,
            delay,
            decoded_frame,
        )?;
        if frame_sender.blocking_send(frame).is_err() {
            return Ok(());
        }
        sent_any_frame = true;
    }

    if !sent_any_frame && start_position > Duration::ZERO {
        return decode_animated_image_frames_into_channel(
            path,
            generation,
            Duration::ZERO,
            frame_sender,
        );
    }

    Ok(())
}

fn inspect_animated_image_metadata(path: &Path) -> Result<AnimatedImageMetadata, String> {
    let file = File::open(path)
        .map_err(|error| format!("could not open animated image preview: {error}"))?;
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    options.skip_frame_decoding(true);
    let mut decoder = options
        .read_info(BufReader::new(file))
        .map_err(|error| format!("could not decode animated image preview: {error}"))?;
    let width = u32::from(decoder.width());
    let height = u32::from(decoder.height());
    validate_animated_image_frame_decode_budget(width, height)?;

    let mut frame_count = 0_usize;
    let mut duration = Duration::ZERO;
    while let Some(frame) = decoder
        .read_next_frame()
        .map_err(|error| format!("could not inspect animated image frames: {error}"))?
    {
        frame_count = frame_count
            .checked_add(1)
            .ok_or_else(|| "This GIF has too many frames to inspect safely".to_owned())?;
        if frame_count > ANIMATED_IMAGE_PREVIEW_DURATION_FRAME_LIMIT {
            return Ok(AnimatedImageMetadata {
                duration: None,
                playback: AnimatedImagePlayback::Animated,
            });
        }
        duration += normalized_gif_frame_delay(frame.delay);
    }

    if frame_count == 0 {
        return Err("Animated image preview has no frames".to_owned());
    }

    Ok(AnimatedImageMetadata {
        duration: (frame_count > 1).then_some(duration),
        playback: if frame_count > 1 {
            AnimatedImagePlayback::Animated
        } else {
            AnimatedImagePlayback::Static
        },
    })
}

struct AnimatedImageMetadata {
    duration: Option<Duration>,
    playback: AnimatedImagePlayback,
}

impl AnimatedImageFrame {
    fn from_decoded_frame(
        path: PathBuf,
        generation: u64,
        position: Duration,
        delay: Duration,
        decoded_frame: image::Frame,
    ) -> Result<Self, String> {
        let rgba_buffer = decoded_frame.into_buffer();
        let width = rgba_buffer.width();
        let height = rgba_buffer.height();
        validate_animated_image_frame_decode_budget(width, height)?;

        Ok(Self {
            path,
            generation,
            position,
            delay,
            handle: iced_image::Handle::from_rgba(width, height, rgba_buffer.into_raw()),
            width,
            height,
        })
    }
}

fn animated_image_frame_rgba_byte_len(width: u32, height: u32) -> Result<usize, String> {
    let bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(animated_image_preview_too_many_rgba_bytes_message)?;
    usize::try_from(bytes).map_err(|_| animated_image_preview_too_many_rgba_bytes_message())
}

fn validate_animated_image_frame_decode_budget(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Animated image preview has invalid dimensions".to_owned());
    }

    let frame_bytes = animated_image_frame_rgba_byte_len(width, height)?;
    if frame_bytes > ANIMATED_IMAGE_PREVIEW_MAX_RGBA_BYTES {
        return Err(animated_image_preview_too_many_rgba_bytes_message());
    }

    Ok(())
}

fn animated_image_preview_too_many_rgba_bytes_message() -> String {
    format!(
        "This GIF is too large to preview safely: decoded RGBA frames are limited to {} MiB",
        ANIMATED_IMAGE_PREVIEW_MAX_RGBA_BYTES / 1024 / 1024
    )
}

fn normalized_frame_delay(delay: image::Delay) -> Duration {
    let (numerator, denominator) = delay.numer_denom_ms();
    normalized_delay(Duration::from_nanos(
        (u128::from(numerator) * 1_000_000 / u128::from(denominator)) as u64,
    ))
}

fn normalized_gif_frame_delay(delay_centiseconds: u16) -> Duration {
    normalized_delay(Duration::from_millis(u64::from(delay_centiseconds) * 10))
}

fn normalized_delay(duration: Duration) -> Duration {
    duration.max(MIN_ANIMATED_IMAGE_FRAME_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_image_frame_delay_is_clamped() {
        let delay = image::Delay::from_numer_denom_ms(0, 1);

        assert_eq!(
            normalized_frame_delay(delay),
            MIN_ANIMATED_IMAGE_FRAME_DELAY
        );
    }

    #[test]
    fn zero_gif_frame_delay_is_clamped() {
        assert_eq!(
            normalized_gif_frame_delay(0),
            MIN_ANIMATED_IMAGE_FRAME_DELAY
        );
    }

    #[test]
    fn animated_image_last_frame_deadline_includes_its_delay() {
        let started_at = time::Instant::now();
        let frame = AnimatedImageFrame {
            path: PathBuf::from("/tmp/sample.gif"),
            generation: 1,
            position: Duration::from_millis(40),
            delay: Duration::from_millis(30),
            handle: iced_image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };

        assert_eq!(
            animated_image_frame_deadline(started_at, Duration::ZERO, &frame),
            started_at + Duration::from_millis(70)
        );
    }

    #[test]
    fn animated_image_preview_accepts_streamed_frame_without_storing_history() {
        let first_frame = AnimatedImageFrame {
            path: PathBuf::from("/tmp/sample.gif"),
            generation: 1,
            position: Duration::ZERO,
            delay: Duration::from_millis(20),
            handle: iced_image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let second_frame = AnimatedImageFrame {
            path: PathBuf::from("/tmp/sample.gif"),
            generation: 1,
            position: Duration::from_millis(20),
            delay: Duration::from_millis(20),
            handle: iced_image::Handle::from_rgba(1, 1, vec![255, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let mut preview = AnimatedImagePreview::new(
            PathBuf::from("/tmp/sample.gif"),
            first_frame,
            1,
            Some(Duration::from_millis(40)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");

        preview.accept_frame(second_frame);

        assert_eq!(preview.playback_position(), Duration::from_millis(20));
        assert!(preview.previous_frame_handle().is_some());
    }

    #[test]
    fn animated_image_preview_seek_consumes_external_stream_generation() {
        let first_frame = AnimatedImageFrame {
            path: PathBuf::from("/tmp/sample.gif"),
            generation: 1,
            position: Duration::ZERO,
            delay: Duration::from_millis(20),
            handle: iced_image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let mut preview = AnimatedImagePreview::new(
            PathBuf::from("/tmp/sample.gif"),
            first_frame,
            1,
            Some(Duration::from_millis(100)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");

        preview.seek_to_position(Duration::from_millis(60));
        assert!(!preview.is_playing());

        preview.commit_seek(2);

        assert_eq!(preview.generation(), 2);
        assert_eq!(preview.stream_start_position(), Duration::from_millis(60));
        assert!(preview.is_playing());
    }

    #[test]
    fn animated_image_frame_keeps_decoded_canvas_dimensions() {
        let buffer = image::RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255])
            .expect("rgba buffer");
        let frame = image::Frame::new(buffer);

        let animated_frame = AnimatedImageFrame::from_decoded_frame(
            PathBuf::from("/tmp/sample.gif"),
            1,
            Duration::ZERO,
            Duration::from_millis(20),
            frame,
        )
        .expect("decoded frame");

        assert_eq!(animated_frame.width, 2);
        assert_eq!(animated_frame.height, 1);
    }

    #[test]
    fn animated_image_decode_budget_rejects_single_oversized_canvas() {
        let over_budget_width = (ANIMATED_IMAGE_PREVIEW_MAX_RGBA_BYTES / 4 + 1) as u32;

        let error = validate_animated_image_frame_decode_budget(over_budget_width, 1)
            .expect_err("oversized canvas should be rejected before frame decode");

        assert!(error.contains("decoded RGBA frames are limited"));
    }
}
