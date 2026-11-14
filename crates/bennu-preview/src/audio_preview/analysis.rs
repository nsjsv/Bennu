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
/// 每 400 个采样一个峰值：每秒 20 个，放大到整个窗口宽也不糊。
const SAMPLES_PER_PEAK: usize = 400;
const READ_CHUNK_BYTES: usize = 64 * 1024;
pub const SPECTROGRAM_WIDTH: u32 = 1024;
pub const SPECTROGRAM_HEIGHT: u32 = 256;

#[derive(Debug, PartialEq)]
pub struct AudioWaveform {
    /// 每段的绝对值最大者，0.0–1.0。
    pub peaks: Vec<f32>,
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
    let mut peaks = PeakCollector::default();
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
        peaks.feed(&carry[..whole]);
        carry.drain(..whole);
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("ffmpeg did not finish: {error}"))?;
    if !status.success() && peaks.sample_count == 0 {
        return Err(format!("ffmpeg exited with {status}"));
    }
    Ok(peaks.finish())
}

#[derive(Default)]
struct PeakCollector {
    peaks: Vec<f32>,
    current: f32,
    in_current: usize,
    sample_count: u64,
}

impl PeakCollector {
    fn feed(&mut self, bytes: &[u8]) {
        for sample in bytes.as_chunks::<4>().0 {
            let level = f32::from_le_bytes(*sample).abs().min(1.0);
            self.current = self.current.max(level);
            self.in_current += 1;
            self.sample_count += 1;
            if self.in_current == SAMPLES_PER_PEAK {
                self.peaks.push(self.current);
                self.current = 0.0;
                self.in_current = 0;
            }
        }
    }

    fn finish(mut self) -> AudioWaveform {
        if self.in_current > 0 {
            self.peaks.push(self.current);
        }
        AudioWaveform {
            peaks: self.peaks,
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
    fn peaks_keep_the_loudest_sample_of_each_slice() {
        let mut collector = PeakCollector::default();
        let mut levels = vec![0.1_f32; SAMPLES_PER_PEAK];
        levels[7] = -0.9;
        levels.extend([0.2, 0.3]);
        collector.feed(&samples(&levels));
        let waveform = collector.finish();
        assert_eq!(waveform.peaks, vec![0.9, 0.3]);
        assert_eq!(
            waveform.duration,
            Duration::from_secs_f64((SAMPLES_PER_PEAK + 2) as f64 / 8000.0)
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
            (58..=62).contains(&waveform.peaks.len()),
            "{}",
            waveform.peaks.len()
        );
        // ffmpeg 的 sine 源振幅 1/8。
        assert!(waveform.peaks.iter().all(|peak| (0.1..0.15).contains(peak)));

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
