use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

#[derive(Clone)]
pub(crate) struct AudioPreviewRuntime {
    handles: Arc<AudioPreviewHandles>,
}

impl AudioPreviewRuntime {
    fn new(device_sink: MixerDeviceSink, player: Player) -> Self {
        Self {
            handles: Arc::new(AudioPreviewHandles {
                player,
                _device_sink: device_sink,
            }),
        }
    }

    pub(crate) fn play(&self) {
        self.handles.player.play();
    }

    pub(crate) fn pause(&self) {
        self.handles.player.pause();
    }

    pub(crate) fn stop(&self) {
        self.handles.player.stop();
    }

    pub(crate) fn seek_to(&self, position: Duration) -> Result<(), String> {
        self.handles
            .player
            .try_seek(position)
            .map_err(|error| format!("could not seek audio preview: {error}"))
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        self.handles.player.set_volume(volume.clamp(0.0, 1.0));
    }

    pub(crate) fn position(&self) -> Duration {
        self.handles.player.get_pos()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.handles.player.empty()
    }
}

impl fmt::Debug for AudioPreviewRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AudioPreviewRuntime").finish()
    }
}

struct AudioPreviewHandles {
    player: Player,
    // rodio 需要输出设备句柄与 Player 同寿命，否则播放会被立即停止。
    _device_sink: MixerDeviceSink,
}

pub(crate) struct AudioPreviewMetadata {
    pub(crate) duration: Option<Duration>,
    pub(crate) len: u64,
}

pub(crate) async fn inspect_audio_preview_metadata(
    path: PathBuf,
) -> Result<AudioPreviewMetadata, String> {
    tokio::task::spawn_blocking(move || inspect_audio_preview_metadata_blocking(path.as_path()))
        .await
        .map_err(|error| format!("could not inspect audio preview: {error}"))?
}

fn inspect_audio_preview_metadata_blocking(path: &Path) -> Result<AudioPreviewMetadata, String> {
    let file =
        File::open(path).map_err(|error| format!("could not open audio preview: {error}"))?;
    let len = file
        .metadata()
        .map_err(|error| format!("could not inspect audio preview: {error}"))?
        .len();
    let decoder = Decoder::try_from(file)
        .map_err(|error| format!("could not decode audio preview: {error}"))?;

    Ok(AudioPreviewMetadata {
        duration: decoder.total_duration(),
        len,
    })
}

pub(crate) async fn start_audio_preview(path: PathBuf) -> Result<AudioPreviewRuntime, String> {
    start_audio_preview_at(path, Duration::ZERO).await
}

pub(crate) async fn start_audio_preview_at(
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
    let file =
        File::open(&path).map_err(|error| format!("could not open audio preview: {error}"))?;
    let source = Decoder::try_from(file)
        .map_err(|error| format!("could not decode audio preview: {error}"))?;
    let mut device_sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|error| format!("could not open audio output: {error}"))?;
    device_sink.log_on_drop(false);
    let player = Player::connect_new(device_sink.mixer());
    player.append(source);
    if position > Duration::ZERO {
        player
            .try_seek(position)
            .map_err(|error| format!("could not seek audio preview: {error}"))?;
    }
    player.play();

    Ok(AudioPreviewRuntime::new(device_sink, player))
}
