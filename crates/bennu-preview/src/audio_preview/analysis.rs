//! 波形与声谱图的计算。都要把整首解一遍，所以在 tokio 子进程里跑、可取消：调用方
//! 取消令牌时 future 被丢，`kill_on_drop` 杀掉 ffmpeg，不会在后台空转到结尾。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::image;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// 只看轮廓，8 kHz 单声道足够，也让一小时的文件很快解完。
const WAVEFORM_SAMPLE_RATE: u32 = 8_000;
/// 每 400 个采样一段：每秒 20 段，放大到整个窗口宽也不糊。
const SAMPLES_PER_SLICE: usize = 400;
const READ_CHUNK_BYTES: usize = 64 * 1024;
pub const SPECTROGRAM_WIDTH: u32 = 1024;
pub const SPECTROGRAM_HEIGHT: u32 = 256;

#[derive(Debug, PartialEq)]
pub struct AudioWaveform {
    /// 每段的平均响度（均方根），按整首最响的一段放大到 0.0–1.0。
    pub levels: Vec<f32>,
    /// 真正解出来的长度：ffprobe 只能估时长的文件拿它替换。
    pub duration: Duration,
}

pub async fn compute_audio_waveform(
    path: PathBuf,
    cancel: CancellationToken,
) -> Result<Arc<AudioWaveform>, String> {
    tokio::select! {
        () = cancel.cancelled() => Err("canceled".to_owned()),
        waveform = decode_waveform(&path) => waveform.map(Arc::new),
    }
}

async fn decode_waveform(path: &Path) -> Result<AudioWaveform, String> {
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1"])
        .arg("-ar")
        .arg(WAVEFORM_SAMPLE_RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg has no output pipe".to_owned())?;
    let mut levels = LoudnessCollector::default();
    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut carry = Vec::new();
    loop {
        let read = stdout
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read ffmpeg output: {error}"))?;
        if read == 0 {
            break;
        }
        carry.extend_from_slice(&buffer[..read]);
        let whole = carry.len() - carry.len() % 4;
        levels.feed(&carry[..whole]);
        carry.drain(..whole);
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("ffmpeg did not finish: {error}"))?;
    if !status.success() && levels.sample_count == 0 {
        return Err(format!("ffmpeg exited with {status}"));
    }
    Ok(levels.finish())
}

/// 用每段的均方根而不是峰值：响度大的母带峰值几乎处处顶满，按峰值画就是一整块，
/// 看不出强弱；再按整首最响的一段放大，很轻的录音也不会只剩薄薄一条。
#[derive(Default)]
struct LoudnessCollector {
    levels: Vec<f32>,
    squares: f64,
    in_current: usize,
    sample_count: u64,
}

impl LoudnessCollector {
    fn feed(&mut self, bytes: &[u8]) {
        for sample in bytes.as_chunks::<4>().0 {
            let sample = f64::from(f32::from_le_bytes(*sample));
            self.squares += sample * sample;
            self.in_current += 1;
            self.sample_count += 1;
            if self.in_current == SAMPLES_PER_SLICE {
                self.close_slice();
            }
        }
    }

    fn close_slice(&mut self) {
        let root_mean_square = (self.squares / self.in_current as f64).sqrt();
        self.levels.push(root_mean_square as f32);
        self.squares = 0.0;
        self.in_current = 0;
    }

    fn finish(mut self) -> AudioWaveform {
        if self.in_current > 0 {
            self.close_slice();
        }
        let loudest = self.levels.iter().copied().fold(0.0_f32, f32::max);
        if loudest > 0.0 {
            for level in &mut self.levels {
                *level /= loudest;
            }
        }
        AudioWaveform {
            levels: self.levels,
            duration: Duration::from_secs_f64(
                self.sample_count as f64 / f64::from(WAVEFORM_SAMPLE_RATE),
            ),
        }
    }
}

/// 用 ffmpeg 的声谱图滤镜出一张图：不带它自己的刻度（面板自己标频率，跳转要整张图
/// 对齐时长），配色是滤镜默认的 intensity。
pub async fn compute_audio_spectrogram(
    path: PathBuf,
    cancel: CancellationToken,
) -> Result<image::Handle, String> {
    tokio::select! {
        () = cancel.cancelled() => Err("canceled".to_owned()),
        spectrogram = render_spectrogram(&path) => spectrogram,
    }
}

async fn render_spectrogram(path: &Path) -> Result<image::Handle, String> {
    // 直接要原始 RGBA：尺寸是自己定的，不用再编成 PNG 又解回来，也不依赖 PNG 编码器。
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(path)
        // 音轨写进滤镜图里，输出就只有这张图；另加 -map 会多出一路音频去找编码器。
        .arg("-lavfi")
        .arg(format!(
            "[0:a:0]showspectrumpic=s={SPECTROGRAM_WIDTH}x{SPECTROGRAM_HEIGHT}:legend=0,format=rgba"
        ))
        .args(["-frames:v", "1", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let expected = (SPECTROGRAM_WIDTH * SPECTROGRAM_HEIGHT * 4) as usize;
    if !output.status.success() || output.stdout.len() != expected {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.lines().last().unwrap_or("ffmpeg failed").trim();
        return Err(reason.to_owned());
    }
    Ok(image::Handle::from_rgba(
        SPECTROGRAM_WIDTH,
        SPECTROGRAM_HEIGHT,
        output.stdout,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(levels: &[f32]) -> Vec<u8> {
        levels
            .iter()
            .flat_map(|level| level.to_le_bytes())
            .collect()
    }

    #[test]
    fn levels_are_slice_loudness_scaled_to_the_loudest_slice() {
        let mut collector = LoudnessCollector::default();
        // 第一段：几乎都是 0.1，只有一个 -0.9 的尖峰；第二段（不满一段）：0.2、0.3。
        let mut levels = vec![0.1_f32; SAMPLES_PER_SLICE];
        levels[7] = -0.9;
        levels.extend([0.2, 0.3]);
        collector.feed(&samples(&levels));
        let waveform = collector.finish();
        let first = ((399.0 * 0.01 + 0.81) / 400.0_f64).sqrt();
        let second = ((0.04 + 0.09) / 2.0_f64).sqrt();
        assert_eq!(waveform.levels.len(), 2);
        // 一个尖峰不再把整段顶满：第一段比第二段轻。
        assert!((f64::from(waveform.levels[0]) - first / second).abs() < 1e-4);
        assert_eq!(waveform.levels[1], 1.0);
        assert_eq!(
            waveform.duration,
            Duration::from_secs_f64((SAMPLES_PER_SLICE + 2) as f64 / 8000.0)
        );
    }

    #[tokio::test]
    async fn real_ffmpeg_waveform_and_spectrogram() {
        let directory = tempfile::tempdir().expect("temp dir");
        let Some(path) =
            crate::audio_preview::test_fixtures::sine_fixture(directory.path(), "tone.opus", 3)
        else {
            return;
        };
        let waveform = compute_audio_waveform(path.clone(), CancellationToken::new())
            .await
            .expect("waveform");
        assert!((waveform.duration.as_secs_f64() - 3.0).abs() < 0.05);
        assert!(
            (58..=62).contains(&waveform.levels.len()),
            "{}",
            waveform.levels.len()
        );
        // 正弦波每段一样响，放大后都接近 1。
        assert!(waveform
            .levels
            .iter()
            .all(|level| (0.9..=1.0).contains(level)));

        let image::Handle::Rgba { width, height, .. } =
            compute_audio_spectrogram(path, CancellationToken::new())
                .await
                .expect("spectrogram")
        else {
            panic!("rgba spectrogram");
        };
        assert_eq!((width, height), (SPECTROGRAM_WIDTH, SPECTROGRAM_HEIGHT));
    }

    #[tokio::test]
    async fn canceled_analysis_returns_without_finishing() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = compute_audio_waveform(PathBuf::from("/nonexistent.flac"), cancel).await;
        assert!(outcome.is_err());
    }
}
