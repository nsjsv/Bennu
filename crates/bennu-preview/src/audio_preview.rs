//! 音频预览的播放运行时与信息读取，音频文件和视频文件的声音共用。解码全部交给
//! ffmpeg（能解的格式远多于纯 Rust 解码库），rodio 只负责把 PCM 送进声卡。

use std::fmt;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

mod analysis;
mod cover;
mod lyrics;
mod pcm_stream;
mod probe;
#[cfg(test)]
pub(crate) mod test_fixtures;

pub use analysis::{
    compute_audio_spectrogram, compute_audio_waveform, AudioWaveform, SPECTROGRAM_HEIGHT,
    SPECTROGRAM_WIDTH,
};
pub use cover::load_audio_cover;
pub use lyrics::{load_audio_lyrics, AudioLyrics, LyricLine};
use pcm_stream::{spawn_pcm_stream, FfmpegPcmSource, PcmFormat, PlaybackExchange};
pub use probe::{AudioProbe, AudioProbeError, AudioStreamFacts, AudioTags};

/// 一个文件一次预览会话的播放：声卡输出与播放器只开一次，播放、暂停、跳转、
/// 播完重播都复用；随最后一个克隆释放（播放器停止 → 音源被丢 → ffmpeg 被杀）。
#[derive(Clone)]
pub struct AudioPreviewRuntime {
    handles: Arc<AudioPreviewHandles>,
}

struct AudioPreviewHandles {
    path: PathBuf,
    format: PcmFormat,
    exchange: Arc<PlaybackExchange>,
    player: Player,
    // rodio 需要输出设备句柄与 Player 同寿命，否则播放会被立即停止。
    _device_sink: MixerDeviceSink,
}

impl AudioPreviewRuntime {
    pub fn play(&self) {
        self.handles.player.play();
    }

    pub fn pause(&self) {
        self.handles.player.pause();
    }

    pub fn stop(&self) {
        self.handles.player.stop();
    }

    /// 从 `position` 起换一条新的解码流；不等新流出数据，界面线程不被卡住。
    pub fn seek_to(&self, position: Duration) -> Result<(), String> {
        let handles = &self.handles;
        let stream = spawn_pcm_stream(
            &handles.path,
            position,
            handles.format,
            Arc::clone(handles.exchange.failure()),
        )
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
        handles.exchange.replace_stream(stream);
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) {
        self.handles.player.set_volume(volume.clamp(0.0, 1.0));
    }

    pub fn position(&self) -> Duration {
        self.handles.exchange.position()
    }

    pub fn is_finished(&self) -> bool {
        self.handles.exchange.has_ended()
    }

    /// 播放途中 ffmpeg 自己失败（文件后半段损坏、网络挂载断开等）时的说明，读一次就清掉。
    pub fn take_failure(&self) -> Option<String> {
        self.handles.exchange.failure().take()
    }
}

impl fmt::Debug for AudioPreviewRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AudioPreviewRuntime").finish()
    }
}

pub async fn probe_audio_preview(path: PathBuf) -> Result<AudioProbe, AudioProbeError> {
    tokio::task::spawn_blocking(move || probe::probe_audio_blocking(&path))
        .await
        .map_err(|error| AudioProbeError::Failed(format!("could not inspect audio: {error}")))?
}

/// ffprobe 只能估时长时的补救：读完整个文件拿准确值，拿不到就是 `None`。
pub async fn measure_audio_preview_duration(path: PathBuf) -> Option<Duration> {
    tokio::task::spawn_blocking(move || probe::measure_audio_duration_blocking(&path))
        .await
        .ok()
        .flatten()
}

pub async fn start_audio_preview_at(
    path: PathBuf,
    position: Duration,
) -> Result<AudioPreviewRuntime, String> {
    tokio::task::spawn_blocking(move || start_audio_preview_blocking(path, position))
        .await
        .map_err(|error| format!("could not start audio preview: {error}"))?
}

fn start_audio_preview_blocking(
    path: PathBuf,
    position: Duration,
) -> Result<AudioPreviewRuntime, String> {
    let mut device_sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|error| format!("could not open audio output: {error}"))?;
    device_sink.log_on_drop(false);
    let format = pcm_format_for_output(&device_sink);
    let exchange = Arc::new(PlaybackExchange::new(format.sample_rate));
    let mut stream = spawn_pcm_stream(&path, position, format, Arc::clone(exchange.failure()))
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    stream.wait_for_audio(exchange.failure())?;
    exchange.replace_stream(stream);

    let player = Player::connect_new(device_sink.mixer());
    player.append(FfmpegPcmSource::new(Arc::clone(&exchange), format));
    player.play();

    Ok(AudioPreviewRuntime {
        handles: Arc::new(AudioPreviewHandles {
            path,
            format,
            exchange,
            player,
            _device_sink: device_sink,
        }),
    })
}

/// 让 ffmpeg 直接输出声卡的采样率，rodio 就不用再做一次（质量更差的）重采样；
/// 多声道源由 ffmpeg 下混到立体声，单声道声卡给单声道。
fn pcm_format_for_output(device_sink: &MixerDeviceSink) -> PcmFormat {
    let config = device_sink.config();
    let channels = if config.channel_count().get() >= 2 {
        NonZero::new(2).expect("two channels")
    } else {
        NonZero::new(1).expect("one channel")
    };
    PcmFormat {
        channels,
        sample_rate: config.sample_rate(),
    }
}
