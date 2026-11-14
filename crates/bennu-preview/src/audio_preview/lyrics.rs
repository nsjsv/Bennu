//! 歌词：同名 .lrc 优先（带时间轴的多半在它里面；ffprobe 读不到内嵌的同步歌词），没有再用
//! 内嵌歌词标签。.lrc 常见 GBK，编码按文本预览同一套识别。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// .lrc 正常几 KB；再大多半不是歌词，不读。
const LYRICS_FILE_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum AudioLyrics {
    /// 带时间轴，按时间排好。
    Synced(Vec<LyricLine>),
    Plain(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    pub time: Duration,
    pub text: String,
}

impl AudioLyrics {
    /// 播到 `position` 时该高亮的那句：开始时间不晚于它的最后一句；第一句之前没有。
    pub fn current_line(&self, position: Duration) -> Option<usize> {
        let AudioLyrics::Synced(lines) = self else {
            return None;
        };
        lines
            .partition_point(|line| line.time <= position)
            .checked_sub(1)
    }
}

pub async fn load_audio_lyrics(path: PathBuf, embedded: Option<String>) -> Option<AudioLyrics> {
    tokio::task::spawn_blocking(move || {
        sidecar_lyrics(&path).or_else(|| embedded.as_deref().and_then(parse_lyrics))
    })
    .await
    .ok()
    .flatten()
}

fn sidecar_lyrics(audio: &Path) -> Option<AudioLyrics> {
    let text = ["lrc", "LRC"].iter().find_map(|extension| {
        let candidate = audio.with_extension(extension);
        let metadata = std::fs::metadata(&candidate).ok()?;
        if !metadata.is_file() || metadata.len() > LYRICS_FILE_MAX_BYTES {
            return None;
        }
        decode_text(&std::fs::read(candidate).ok()?)
    })?;
    parse_lyrics(&text)
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    let sniffed = file_core::text_encoding::sniff_text_encoding(bytes, true)?;
    let (text, _) = sniffed
        .encoding
        .decode_without_bom_handling(&bytes[sniffed.bom_length..]);
    Some(text.into_owned())
}

/// LRC：行首一个或多个 `[mm:ss.xx]`（同一句出现多次就写多个），`[offset:±毫秒]` 整体挪动，
/// `[ar:…]` 之类的信息标签忽略，句中逐字时间 `<mm:ss.xx>` 去掉。一个时间标签都没有就是纯文本歌词。
pub fn parse_lyrics(text: &str) -> Option<AudioLyrics> {
    let mut offset_millis: i64 = 0;
    let mut synced = Vec::new();
    let mut plain = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let (tags, rest) = leading_tags(line);
        let mut times = Vec::new();
        for tag in &tags {
            match parse_tag(tag) {
                LrcTag::Time(time) => times.push(time),
                LrcTag::Offset(millis) => offset_millis = millis,
                LrcTag::Other => {}
            }
        }
        if !times.is_empty() {
            let text = strip_word_timings(rest).trim().to_owned();
            synced.extend(times.into_iter().map(|time| LyricLine {
                time,
                text: text.clone(),
            }));
        } else if tags.is_empty() {
            plain.push(line.to_owned());
        }
    }
    if !synced.is_empty() {
        // 正的 offset 让歌词提前出现（LRC 约定）。
        for line in &mut synced {
            line.time = shift(line.time, -offset_millis);
        }
        synced.sort_by_key(|line| line.time);
        return Some(AudioLyrics::Synced(synced));
    }
    let first = plain.iter().position(|line| !line.is_empty())?;
    let last = plain.iter().rposition(|line| !line.is_empty())?;
    Some(AudioLyrics::Plain(plain[first..=last].to_vec()))
}

enum LrcTag {
    Time(Duration),
    Offset(i64),
    Other,
}

/// 行首连续的 `[...]`，返回标签内容和剩下的正文。
fn leading_tags(line: &str) -> (Vec<&str>, &str) {
    let mut tags = Vec::new();
    let mut rest = line;
    while let Some(inner) = rest.strip_prefix('[') {
        let Some(close) = inner.find(']') else {
            break;
        };
        tags.push(&inner[..close]);
        rest = &inner[close + 1..];
    }
    (tags, rest)
}

fn parse_tag(tag: &str) -> LrcTag {
    if let Some(value) = tag.strip_prefix("offset:") {
        return value.trim().parse().map_or(LrcTag::Other, LrcTag::Offset);
    }
    parse_timestamp(tag).map_or(LrcTag::Other, LrcTag::Time)
}

/// `mm:ss`、`mm:ss.xx`、`mm:ss.xxx`，也有人把小数点写成冒号；分钟可以超过 59。
fn parse_timestamp(tag: &str) -> Option<Duration> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds = seconds.trim().replacen(':', ".", 1);
    if !seconds.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let seconds: f64 = seconds.parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0)
        .then(|| Duration::from_secs(minutes * 60) + Duration::from_secs_f64(seconds))
}

fn strip_word_timings(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        let inner = &rest[open + 1..open + close];
        stripped.push_str(&rest[..open]);
        if parse_timestamp(inner).is_none() {
            stripped.push_str(&rest[open..=open + close]);
        }
        rest = &rest[open + close + 1..];
    }
    stripped.push_str(rest);
    stripped
}

fn shift(time: Duration, millis: i64) -> Duration {
    let delta = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        time.saturating_add(delta)
    } else {
        time.saturating_sub(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn timestamps_repeat_sort_and_shift_by_offset() {
        let lyrics = parse_lyrics(
            "[ti:晴天]\n[offset:500]\n[00:12.50][01:02.00]故事的小黄花\n[00:05.00]从出生那年就飘着\n[00:20]\n",
        )
        .expect("lyrics");
        let AudioLyrics::Synced(lines) = &lyrics else {
            panic!("synced");
        };
        let times: Vec<_> = lines.iter().map(|line| line.time).collect();
        assert_eq!(times, vec![at(4.5), at(12.0), at(19.5), at(61.5)]);
        assert_eq!(lines[1].text, "故事的小黄花");
        assert_eq!(lines[3].text, "故事的小黄花");
        assert_eq!(lines[2].text, "");
    }

    #[test]
    fn current_line_is_the_last_one_already_started() {
        let lyrics = parse_lyrics("[00:01.00]a\n[00:03.00]b\n").expect("lyrics");
        assert_eq!(lyrics.current_line(at(0.5)), None);
        assert_eq!(lyrics.current_line(at(1.0)), Some(0));
        assert_eq!(lyrics.current_line(at(2.9)), Some(0));
        assert_eq!(lyrics.current_line(at(10.0)), Some(1));
    }

    #[test]
    fn odd_timestamp_forms_and_word_timings() {
        let lyrics =
            parse_lyrics("[1:02:30]<01:02.30>逐<01:02.80>字 <b>\n[120:00.000]end").expect("lyrics");
        let AudioLyrics::Synced(lines) = lyrics else {
            panic!("synced");
        };
        assert_eq!(lines[0].time, at(62.3));
        assert_eq!(lines[0].text, "逐字 <b>");
        assert_eq!(lines[1].time, at(7200.0));
    }

    #[test]
    fn text_without_timestamps_is_plain_and_trimmed() {
        assert_eq!(
            parse_lyrics("\n\nfirst\n\nsecond\n\n"),
            Some(AudioLyrics::Plain(vec![
                "first".to_owned(),
                String::new(),
                "second".to_owned()
            ]))
        );
        assert_eq!(parse_lyrics("[ar:someone]\n\n"), None);
    }

    #[test]
    fn gbk_sidecar_wins_over_embedded_lyrics() {
        let directory = tempfile::tempdir().expect("temp dir");
        let song = directory.path().join("song.flac");
        // 真实的 .lrc 有好几十行；编码识别要靠足够多的字节，太短的样本谁也猜不准。
        let text = "[ti:晴天]\n[00:01.00]故事的小黄花\n[00:05.00]从出生那年就飘着\n\
                    [00:09.00]童年的荡秋千\n[00:13.00]随记忆一直晃到现在\n";
        let (gbk, _, _) = encoding_rs::GBK.encode(text);
        std::fs::write(directory.path().join("song.lrc"), &gbk).expect("write lrc");
        let lyrics = sidecar_lyrics(&song)
            .or_else(|| parse_lyrics("embedded"))
            .expect("lyrics");
        let AudioLyrics::Synced(lines) = lyrics else {
            panic!("synced");
        };
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "故事的小黄花");
        assert_eq!(lines[3].text, "随记忆一直晃到现在");
        assert!(sidecar_lyrics(&directory.path().join("other.flac")).is_none());
    }
}
