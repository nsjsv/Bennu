//! ffprobe 读音频信息：时长、标签、技术参数、内嵌封面在哪条流。
//!
//! 时长有两种来源：容器/流里写好的（准），和 ffmpeg 按码率估的（无 Xing 头的 VBR MP3、
//! ADTS AAC 等，可能差几十秒）；后者另跑一遍只拆包不解码的扫描拿准确值。

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
    pub tags: AudioTags,
    pub stream: AudioStreamFacts,
    /// 内嵌封面（标成 attached_pic 的图片流）的流序号。
    pub embedded_cover_stream: Option<usize>,
    /// 内嵌歌词标签（ID3 USLT 出来是 `lyrics-<语言>`，Vorbis 注释是 LYRICS/UNSYNCEDLYRICS）。
    pub embedded_lyrics: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// 音轨的技术参数；有损编码没有位深，写 0 的字段当作没有。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioStreamFacts {
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub bit_rate: Option<u64>,
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
        .args(["-show_format", "-show_streams"])
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
    let streams = document["streams"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let stream = streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio")
        .ok_or_else(|| AudioProbeError::Failed("the file has no audio stream".to_owned()))?;
    let format = &document["format"];
    let duration =
        seconds_field(&format["duration"]).or_else(|| seconds_field(&stream["duration"]));
    Ok(AudioProbe {
        duration,
        duration_is_estimate: stderr.contains(ESTIMATED_DURATION_WARNING),
        // MP3/MP4/FLAC 的标签挂在容器上，Ogg（Vorbis/Opus）的挂在流上。
        tags: repair_mislabeled_tags(AudioTags {
            title: tag(format, stream, "title"),
            artist: tag(format, stream, "artist"),
            album: tag(format, stream, "album"),
        }),
        stream: AudioStreamFacts {
            codec: stream["codec_name"].as_str().map(codec_display_name),
            sample_rate: positive_number(&stream["sample_rate"])
                .and_then(|rate| u32::try_from(rate).ok()),
            bit_depth: positive_number(&stream["bits_per_raw_sample"])
                .or_else(|| positive_number(&stream["bits_per_sample"]))
                .and_then(|bits| u32::try_from(bits).ok()),
            channels: positive_number(&stream["channels"])
                .and_then(|count| u32::try_from(count).ok()),
            bit_rate: positive_number(&stream["bit_rate"])
                .or_else(|| positive_number(&format["bit_rate"])),
        },
        embedded_cover_stream: streams
            .iter()
            .find(|stream| stream["disposition"]["attached_pic"] == 1)
            .and_then(|stream| stream["index"].as_u64())
            .and_then(|index| usize::try_from(index).ok()),
        embedded_lyrics: lyrics_tag(format, stream).map(repair_mislabeled_text),
    })
}

fn lyrics_tag(format: &Value, stream: &Value) -> Option<String> {
    let is_lyrics = |name: &str| {
        let name = name.to_ascii_lowercase();
        name == "lyrics" || name == "unsyncedlyrics" || name.starts_with("lyrics-")
    };
    [format, stream].into_iter().find_map(|owner| {
        owner["tags"].as_object()?.iter().find_map(|(name, value)| {
            let value = value.as_str()?.trim();
            (is_lyrics(name) && !value.is_empty()).then(|| value.to_owned())
        })
    })
}

/// 一段较长的文字（歌词）单独就够判断编码，规则同 `repair_mislabeled_tags`。
fn repair_mislabeled_text(text: String) -> String {
    let Some(bytes) = latin1_bytes(&text) else {
        return text;
    };
    let Some(sniffed) = file_core::text_encoding::sniff_text_encoding(&bytes, true) else {
        return text;
    };
    if sniffed.encoding == encoding_rs::WINDOWS_1252 {
        return text;
    }
    let (decoded, _, had_errors) = sniffed.encoding.decode(&bytes);
    if had_errors {
        text
    } else {
        decoded.into_owned()
    }
}

/// 标签键大小写不定（ID3 小写，Vorbis 注释常见大写），容器上的优先。
fn tag(format: &Value, stream: &Value, key: &str) -> Option<String> {
    [format, stream].into_iter().find_map(|owner| {
        owner["tags"].as_object()?.iter().find_map(|(name, value)| {
            let value = value.as_str()?.trim();
            (name.eq_ignore_ascii_case(key) && !value.is_empty()).then(|| value.to_owned())
        })
    })
}

/// ffprobe 的数字有的写成字符串（"44100"），有的写成数字（2）；0 表示不知道。
fn positive_number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|number| *number > 0)
}

/// 编码名给人看的写法：PCM 各种采样格式统一叫 PCM，WMA 各版本统一叫 WMA。
fn codec_display_name(codec: &str) -> String {
    let display = match codec {
        name if name.starts_with("pcm_") => "PCM",
        name if name.starts_with("wma") => "WMA",
        name if name.starts_with("dsd_") => "DSD",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "wavpack" => "WavPack",
        "mp3float" | "mp3" => "MP3",
        other => return other.to_ascii_uppercase(),
    };
    display.to_owned()
}

/// 老的中文 MP3 常把 GBK 字节写进标成 Latin-1 的 ID3 帧，ffprobe 照 Latin-1 转出来就成了
/// 「Çà»¨´É」。这类字段的每个字符都在 U+00FF 以内，能原样还原成字节；几个字段拼在一起
/// 交给文本预览同一套编码识别（字段太短时单独猜不准），认出不是西欧编码才改写，
/// 所以正常的「Café」不会被动。
fn repair_mislabeled_tags(tags: AudioTags) -> AudioTags {
    let fields = [&tags.title, &tags.artist, &tags.album];
    let mislabeled: Vec<Vec<u8>> = fields
        .iter()
        .filter_map(|field| field.as_deref().and_then(latin1_bytes))
        .collect();
    if mislabeled.is_empty() {
        return tags;
    }
    let sample = mislabeled.join(&b'\n');
    let Some(sniffed) = file_core::text_encoding::sniff_text_encoding(&sample, true) else {
        return tags;
    };
    let encoding = sniffed.encoding;
    if encoding == encoding_rs::WINDOWS_1252 {
        return tags;
    }
    let repair = |field: Option<String>| {
        field.map(|text| match latin1_bytes(&text) {
            Some(bytes) => {
                let (decoded, _, had_errors) = encoding.decode(&bytes);
                if had_errors {
                    text
                } else {
                    decoded.into_owned()
                }
            }
            None => text,
        })
    };
    AudioTags {
        title: repair(tags.title),
        artist: repair(tags.artist),
        album: repair(tags.album),
    }
}

/// 全是 Latin-1 范围且至少有一个非 ASCII 字符时，还原出原始字节。
fn latin1_bytes(text: &str) -> Option<Vec<u8>> {
    let bytes: Option<Vec<u8>> = text
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect();
    bytes.filter(|bytes| bytes.iter().any(|byte| !byte.is_ascii()))
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
            r#"{"streams":[{"codec_type":"audio","duration":"10.0"}],"format":{"duration":"215.640000"}}"#,
            "",
        ))
        .expect("probe");
        assert_eq!(probe.duration, Some(Duration::from_secs_f64(215.64)));
        assert!(!probe.duration_is_estimate);

        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{"codec_type":"audio","duration":"12.5"}],"format":{}}"#,
            "",
        ))
        .expect("probe");
        assert_eq!(probe.duration, Some(Duration::from_secs_f64(12.5)));
    }

    #[test]
    fn bitrate_estimate_warning_marks_the_duration_as_estimated() {
        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"300.0"}}"#,
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
    fn tags_stream_facts_and_cover_come_from_the_right_places() {
        let probe = audio_probe_from_output(&output(
            0,
            r#"{
                "streams": [
                    {"index": 0, "codec_type": "audio", "codec_name": "flac",
                     "sample_rate": "96000", "channels": 2, "bits_per_raw_sample": "24",
                     "tags": {"TITLE": "Stream Title", "ARTIST": "Stream Artist", "lyrics-eng": "  "}},
                    {"index": 1, "codec_type": "video", "disposition": {"attached_pic": 1}}
                ],
                "format": {"duration": "1.0", "bit_rate": "2500000",
                           "tags": {"title": "Format Title", "album": " "}}
            }"#,
            "",
        ))
        .expect("probe");
        assert_eq!(
            probe.tags,
            AudioTags {
                title: Some("Format Title".to_owned()),
                artist: Some("Stream Artist".to_owned()),
                album: None,
            }
        );
        assert_eq!(
            probe.stream,
            AudioStreamFacts {
                codec: Some("FLAC".to_owned()),
                sample_rate: Some(96_000),
                bit_depth: Some(24),
                channels: Some(2),
                bit_rate: Some(2_500_000),
            }
        );
        assert_eq!(probe.embedded_cover_stream, Some(1));
        assert_eq!(probe.embedded_lyrics, None);
    }

    #[test]
    fn lossy_codecs_have_no_bit_depth_and_get_readable_names() {
        let probe = audio_probe_from_output(&output(
            0,
            r#"{"streams":[{"codec_type":"audio","codec_name":"opus","bits_per_sample":0,"sample_rate":"48000"}],"format":{}}"#,
            "",
        ))
        .expect("probe");
        assert_eq!(probe.stream.codec.as_deref(), Some("Opus"));
        assert_eq!(probe.stream.bit_depth, None);
        assert_eq!(codec_display_name("pcm_s16le"), "PCM");
        assert_eq!(codec_display_name("wmav2"), "WMA");
        assert_eq!(codec_display_name("alac"), "ALAC");
    }

    fn as_latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    }

    #[test]
    fn gbk_bytes_labelled_latin1_are_repaired_but_real_latin1_is_kept() {
        let tags = repair_mislabeled_tags(AudioTags {
            title: Some(as_latin1(&[0xC7, 0xE0, 0xBB, 0xA8, 0xB4, 0xC9])),
            artist: Some(as_latin1(&[0xD6, 0xDC, 0xBD, 0xDC, 0xC2, 0xD7])),
            album: Some(as_latin1(&[0xCE, 0xD2, 0xBA, 0xDC, 0xC3, 0xA6])),
        });
        assert_eq!(tags.title.as_deref(), Some("青花瓷"));
        assert_eq!(tags.artist.as_deref(), Some("周杰伦"));
        assert_eq!(tags.album.as_deref(), Some("我很忙"));

        let western = AudioTags {
            title: Some("Café del Mar".to_owned()),
            artist: Some("Mädchen".to_owned()),
            album: None,
        };
        assert_eq!(repair_mislabeled_tags(western.clone()), western);

        // UTF-8 字节被标成 Latin-1 也能还原。
        let tags = repair_mislabeled_tags(AudioTags {
            title: Some(as_latin1("晴天".as_bytes())),
            ..AudioTags::default()
        });
        assert_eq!(tags.title.as_deref(), Some("晴天"));
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
