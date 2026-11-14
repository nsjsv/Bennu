//! ffprobe 读音频信息。时长有两种来源：容器/流里写好的（准），和 ffmpeg 按码率估的
//! （无 Xing 头的 VBR MP3、ADTS AAC 等，可能差几十秒）；后者另跑一遍只拆包不解码的
//! 扫描拿准确值。

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::Value;

/// ffmpeg 按码率估时长时打的警告（libavformat，warning 级别）。
const ESTIMATED_DURATION_WARNING: &str = "Estimating duration from bitrate";

#[derive(Debug, Clone, PartialEq)]
pub struct AudioProbe {
    pub duration: Option<Duration>,
    pub duration_is_estimate: bool,
}

#[derive(Debug)]
pub enum AudioProbeError {
    /// 没装 ffmpeg（ffprobe 启动报 NotFound）。
    FfmpegMissing,
    Failed(String),
}

pub fn probe_audio_blocking(path: &Path) -> Result<AudioProbe, AudioProbeError> {
    let output = Command::new("ffprobe")
        .args(["-v", "warning", "-print_format", "json"])
        .args(["-show_format", "-show_streams", "-select_streams", "a:0"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AudioProbeError::FfmpegMissing,
            _ => AudioProbeError::Failed(format!("could not start ffprobe: {error}")),
        })?;
    audio_probe_from_output(&output)
}

fn audio_probe_from_output(output: &Output) -> Result<AudioProbe, AudioProbeError> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("ffprobe failed");
        return Err(AudioProbeError::Failed(reason.trim().to_owned()));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| AudioProbeError::Failed(format!("unreadable ffprobe output: {error}")))?;
    let stream = document["streams"]
        .as_array()
        .and_then(|streams| streams.first())
        .ok_or_else(|| AudioProbeError::Failed("the file has no audio stream".to_owned()))?;
    let duration = seconds_field(&document["format"]["duration"])
        .or_else(|| seconds_field(&stream["duration"]));
    Ok(AudioProbe {
        duration,
        duration_is_estimate: stderr.contains(ESTIMATED_DURATION_WARNING),
    })
}

/// ffprobe 的 JSON 把秒数写成字符串（如 "215.640000"）。
fn seconds_field(value: &Value) -> Option<Duration> {
    let seconds = value.as_str()?.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// 只拆包不解码地过一遍音轨，取最后的时间戳；比解码快一个数量级，但要读完整个文件。
pub fn measure_audio_duration_blocking(path: &Path) -> Option<Duration> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-c", "copy", "-f", "null", "-"])
        .args(["-progress", "pipe:1", "-nostats"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    last_progress_time(&String::from_utf8_lossy(&output.stdout))
}

/// `-progress` 每隔一段输出一组 `key=value`，最后一组是结尾时的位置。
fn last_progress_time(progress: &str) -> Option<Duration> {
    progress
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("out_time_us="))
        .and_then(|micros| micros.trim().parse::<u64>().ok())
        .filter(|micros| *micros > 0)
        .map(Duration::from_micros)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::*;

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn duration_comes_from_the_format_then_the_stream() {
        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{"duration":"10.0"}],"format":{"duration":"215.640000"}}"#,
            "",
        ))
        .expect("probe");
        assert_eq!(probe.duration, Some(Duration::from_secs_f64(215.64)));
        assert!(!probe.duration_is_estimate);

        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{"duration":"12.5"}],"format":{}}"#,
            "",
        ))
        .expect("probe");
        assert_eq!(probe.duration, Some(Duration::from_secs_f64(12.5)));
    }

    #[test]
    fn bitrate_estimate_warning_marks_the_duration_as_estimated() {
        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{}],"format":{"duration":"300.0"}}"#,
            "[mp3 @ 0x1] Estimating duration from bitrate, this may be inaccurate\n",
        ))
        .expect("probe");
        assert!(probe.duration_is_estimate);
    }

    #[test]
    fn missing_audio_stream_and_ffprobe_failure_are_errors() {
        assert!(matches!(
            audio_probe_from_output(&output(0, r#"{"streams":[],"format":{}}"#, "")),
            Err(AudioProbeError::Failed(_))
        ));
        let Err(AudioProbeError::Failed(reason)) = audio_probe_from_output(&output(
            1,
            "",
            "x.opus: Invalid data found when processing input\n",
        )) else {
            panic!("ffprobe failure");
        };
        assert_eq!(reason, "x.opus: Invalid data found when processing input");
    }

    #[test]
    fn progress_output_yields_the_final_position() {
        let progress = "out_time_us=1000000\nprogress=continue\nout_time_us=N/A\nout_time_us=215640000\nprogress=end\n";
        assert_eq!(
            last_progress_time(progress),
            Some(Duration::from_micros(215_640_000))
        );
        assert_eq!(last_progress_time("progress=end\n"), None);
    }

    #[test]
    fn vbr_mp3_without_a_xing_header_is_flagged_and_measured_exactly() {
        let directory = tempfile::tempdir().expect("temp dir");
        let Some(path) = crate::audio_preview::test_fixtures::encoded_sine_fixture(
            directory.path(),
            "vbr.mp3",
            20,
            &["-c:a", "libmp3lame", "-q:a", "4", "-write_xing", "0"],
        ) else {
            return;
        };
        let probe = probe_audio_blocking(&path).expect("probe");
        assert!(probe.duration_is_estimate);
        let measured = measure_audio_duration_blocking(&path)
            .expect("scan")
            .as_secs_f64();
        assert!((measured - 20.0).abs() < 0.1, "{measured}");
    }

    #[test]
    fn real_ffprobe_and_scan_agree_on_a_generated_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let Some(path) =
            crate::audio_preview::test_fixtures::sine_fixture(directory.path(), "tone.mp3", 3)
        else {
            return;
        };
        let probe = probe_audio_blocking(&path).expect("probe");
        let probed = probe.duration.expect("duration").as_secs_f64();
        assert!((probed - 3.0).abs() < 0.1, "{probed}");
        let measured = measure_audio_duration_blocking(&path)
            .expect("scan")
            .as_secs_f64();
        assert!((measured - 3.0).abs() < 0.1, "{measured}");
    }
}
