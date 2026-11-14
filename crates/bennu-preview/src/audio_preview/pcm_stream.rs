//! ffmpeg 解出的 PCM 怎么送进声卡：一个播放会话只往播放器里放一个音源，跳转时
//! 换的是音源背后的 ffmpeg 流，不用会阻塞调用方的 `Player::clear`。
//!
//! 线程分工：每条流一个读线程（ffmpeg stdout → 块）和一个 stderr 线程（只留尾部，
//! 防止管道写满把 ffmpeg 卡死）；声卡回调线程只做 `try_recv`，拿不到数据就补静音，
//! 永远不阻塞。

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rodio::{ChannelCount, SampleRate, Source};

/// 每块的帧数与通道里最多压几块：48 kHz 立体声约半秒。暂停时音源不取数据，
/// 通道满了读线程停下，ffmpeg 再被管道背压挂起，不占 CPU。
const FRAMES_PER_CHUNK: usize = 4096;
const BUFFERED_CHUNKS: usize = 6;
const STDERR_TAIL_BYTES: usize = 4 * 1024;
const F32_BYTES: usize = std::mem::size_of::<f32>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PcmFormat {
    pub channels: ChannelCount,
    pub sample_rate: SampleRate,
}

impl PcmFormat {
    fn channel_count(self) -> usize {
        usize::from(self.channels.get())
    }
}

/// 从 `start` 开始的一条解码流。`first_chunk` 是启动时为确认能解码而先取出的一块。
pub(crate) struct PcmStream {
    start: Duration,
    first_chunk: Option<Vec<f32>>,
    chunks: Receiver<Vec<f32>>,
}

impl PcmStream {
    /// 等到第一块数据或 ffmpeg 退出。解不了的文件（没有音轨、MIDI 之类）在这里就报错，
    /// 不会先进入「播放中」再变成错误。
    pub(crate) fn wait_for_audio(&mut self, failure: &StreamFailure) -> Result<(), String> {
        match self.chunks.recv() {
            Ok(chunk) => {
                self.first_chunk = Some(chunk);
                Ok(())
            }
            Err(_) => Err(failure
                .take()
                .unwrap_or_else(|| "the file has no playable audio".to_owned())),
        }
    }
}

/// 当前流的 ffmpeg 自己失败时留下的说明；被我们换掉而杀死的流不写。
#[derive(Default)]
pub(crate) struct StreamFailure(Mutex<Option<String>>);

impl StreamFailure {
    fn record(&self, message: String) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
    }

    pub(crate) fn take(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

pub(crate) fn spawn_pcm_stream(
    path: &Path,
    start: Duration,
    format: PcmFormat,
    failure: Arc<StreamFailure>,
) -> std::io::Result<PcmStream> {
    let mut command = Command::new("ffmpeg");
    command.args(["-nostdin", "-hide_banner", "-v", "error"]);
    if start > Duration::ZERO {
        // 放在 -i 前是输入端跳转：先按索引快速定位，再解码丢弃到精确位置。
        command
            .arg("-ss")
            .arg(format!("{:.3}", start.as_secs_f64()));
    }
    command
        .arg("-i")
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn"])
        .arg("-ac")
        .arg(format.channels.get().to_string())
        .arg("-ar")
        .arg(format.sample_rate.get().to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn()?;
    let (sender, chunks) = sync_channel(BUFFERED_CHUNKS);
    let channels = format.channel_count();
    thread::Builder::new()
        .name("audio-preview-pcm".to_owned())
        .spawn(move || pump_pcm(child, sender, channels, &failure))?;
    Ok(PcmStream {
        start,
        first_chunk: None,
        chunks,
    })
}

fn pump_pcm(
    mut child: Child,
    sender: SyncSender<Vec<f32>>,
    channels: usize,
    failure: &StreamFailure,
) {
    let stderr_tail = child.stderr.take().and_then(|stderr| {
        thread::Builder::new()
            .name("audio-preview-stderr".to_owned())
            .spawn(move || read_stderr_tail(stderr))
            .ok()
    });
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let mut bytes = vec![0_u8; FRAMES_PER_CHUNK * channels * F32_BYTES];
    loop {
        let filled = read_until_full(&mut stdout, &mut bytes);
        let whole_frames = filled - filled % (channels * F32_BYTES);
        if whole_frames > 0 {
            let samples = bytes[..whole_frames]
                .as_chunks::<F32_BYTES>()
                .0
                .iter()
                .map(|sample| f32::from_le_bytes(*sample))
                .collect();
            if sender.send(samples).is_err() {
                // 接收端没了：被跳转换掉或会话结束。主动杀掉，不算失败。
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
        if filled < bytes.len() {
            break;
        }
    }
    drop(stdout);
    let status = child.wait();
    let tail = stderr_tail
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => failure.record(ffmpeg_failure_message(&tail, &status.to_string())),
        Err(error) => failure.record(ffmpeg_failure_message(&tail, &error.to_string())),
    }
    // sender 在这里才释放：失败说明先写好，接收端看到断开时一定读得到。
    drop(sender);
}

/// 读满缓冲区或读到结尾，返回读到的字节数；读出错按结尾处理（随后看退出码）。
fn read_until_full(reader: &mut impl Read, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    filled
}

fn read_stderr_tail(mut stderr: ChildStderr) -> String {
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 1024];
    while let Ok(read) = stderr.read(&mut buffer) {
        if read == 0 {
            break;
        }
        tail.extend_from_slice(&buffer[..read]);
        if tail.len() > STDERR_TAIL_BYTES {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
        }
    }
    String::from_utf8_lossy(&tail).trim().to_owned()
}

fn ffmpeg_failure_message(stderr_tail: &str, status: &str) -> String {
    // ffmpeg 的最后一行通常就是结论（如 "Stream map '0:a:0' matches no streams."）。
    match stderr_tail
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
    {
        Some(line) => line.trim().to_owned(),
        None => format!("ffmpeg exited with {status}"),
    }
}

/// 运行时（界面线程）与声卡回调线程之间的交接处：待换上的流、播放进度、是否播完。
pub(crate) struct PlaybackExchange {
    sample_rate: u64,
    pending: Mutex<Option<PcmStream>>,
    has_pending: AtomicBool,
    pending_start_nanos: AtomicU64,
    stream_start_nanos: AtomicU64,
    frames_played: AtomicU64,
    ended: AtomicBool,
    failure: Arc<StreamFailure>,
}

impl PlaybackExchange {
    pub(crate) fn new(sample_rate: SampleRate) -> Self {
        Self {
            sample_rate: u64::from(sample_rate.get()),
            pending: Mutex::new(None),
            has_pending: AtomicBool::new(false),
            pending_start_nanos: AtomicU64::new(0),
            stream_start_nanos: AtomicU64::new(0),
            frames_played: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            failure: Arc::new(StreamFailure::default()),
        }
    }

    pub(crate) fn failure(&self) -> &Arc<StreamFailure> {
        &self.failure
    }

    /// 新流排队，声卡回调线程下一帧边界换上；旧流随即被丢弃，它的 ffmpeg 被读线程杀掉。
    pub(crate) fn replace_stream(&self, stream: PcmStream) {
        self.pending_start_nanos
            .store(duration_nanos(stream.start), Ordering::Release);
        *self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stream);
        self.has_pending.store(true, Ordering::Release);
    }

    /// 还没换上的跳转按目标位置算，进度条不会先跳回旧位置。
    pub(crate) fn position(&self) -> Duration {
        if self.has_pending.load(Ordering::Acquire) {
            return Duration::from_nanos(self.pending_start_nanos.load(Ordering::Acquire));
        }
        let start = self.stream_start_nanos.load(Ordering::Acquire);
        let frames = self.frames_played.load(Ordering::Acquire);
        let played = frames.saturating_mul(1_000_000_000) / self.sample_rate.max(1);
        Duration::from_nanos(start.saturating_add(played))
    }

    pub(crate) fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire) && !self.has_pending.load(Ordering::Acquire)
    }

    fn take_pending(&self) -> Option<PcmStream> {
        if !self.has_pending.load(Ordering::Acquire) {
            return None;
        }
        // 界面线程只在交换瞬间持锁；拿不到锁就下一帧再试，回调线程不等。
        let mut pending = self.pending.try_lock().ok()?;
        let stream = pending.take()?;
        self.stream_start_nanos
            .store(duration_nanos(stream.start), Ordering::Release);
        self.frames_played.store(0, Ordering::Release);
        self.ended.store(false, Ordering::Release);
        self.has_pending.store(false, Ordering::Release);
        Some(stream)
    }
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// 播放器里唯一的音源：从当前流取块，流断了就标记播完并一直输出静音，
/// 直到下一次跳转换上新流。
pub(crate) struct FfmpegPcmSource {
    exchange: Arc<PlaybackExchange>,
    format: PcmFormat,
    chunks: Option<Receiver<Vec<f32>>>,
    chunk: Vec<f32>,
    next_sample: usize,
    silent_samples_left: usize,
}

impl FfmpegPcmSource {
    pub(crate) fn new(exchange: Arc<PlaybackExchange>, format: PcmFormat) -> Self {
        Self {
            exchange,
            format,
            chunks: None,
            chunk: Vec::new(),
            next_sample: 0,
            silent_samples_left: 0,
        }
    }

    fn at_frame_boundary(&self) -> bool {
        self.silent_samples_left == 0
            && self.next_sample.is_multiple_of(self.format.channel_count())
    }

    fn switch_to(&mut self, mut stream: PcmStream) {
        self.chunk = stream.first_chunk.take().unwrap_or_default();
        self.next_sample = 0;
        self.chunks = Some(stream.chunks);
    }

    /// 当前块用完后取下一块；成功返回 true。
    fn refill(&mut self) -> bool {
        let frames = (self.chunk.len() / self.format.channel_count()) as u64;
        self.exchange
            .frames_played
            .fetch_add(frames, Ordering::AcqRel);
        self.chunk.clear();
        self.next_sample = 0;
        let Some(chunks) = &self.chunks else {
            return false;
        };
        match chunks.try_recv() {
            Ok(chunk) => {
                self.chunk = chunk;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.chunks = None;
                self.exchange.ended.store(true, Ordering::Release);
                false
            }
        }
    }
}

impl Iterator for FfmpegPcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at_frame_boundary() {
            if let Some(stream) = self.exchange.take_pending() {
                self.switch_to(stream);
            }
        }
        if self.silent_samples_left > 0 {
            self.silent_samples_left -= 1;
            return Some(0.0);
        }
        if self.next_sample >= self.chunk.len() && !self.refill() {
            // 欠载或已播完：补一整帧静音，保持声道对齐。
            self.silent_samples_left = self.format.channel_count() - 1;
            return Some(0.0);
        }
        let sample = self.chunk[self.next_sample];
        self.next_sample += 1;
        Some(sample)
    }
}

impl Source for FfmpegPcmSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.format.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.format.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
#[path = "pcm_stream_tests.rs"]
mod tests;
