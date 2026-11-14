//! 音频预览里给人看的歌曲文字：标题、歌手 · 专辑、技术参数一行。

use std::path::Path;

use bennu_localization::tr;

use crate::audio_preview::{AudioStreamFacts, AudioTags};

/// 没有歌名标签时用文件名（去掉后缀），和播放器的习惯一样。
pub(crate) fn track_title(path: &Path, tags: &AudioTags) -> String {
    tags.title.clone().unwrap_or_else(|| {
        path.file_stem()
            .or_else(|| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

pub(crate) fn track_subtitle(tags: &AudioTags) -> Option<String> {
    match (&tags.artist, &tags.album) {
        (Some(artist), Some(album)) => Some(format!("{artist} · {album}")),
        (Some(single), None) | (None, Some(single)) => Some(single.clone()),
        (None, None) => None,
    }
}

/// 如「FLAC · 44.1 kHz · 16 位 · 立体声 · 1411 kbps」的各项；读不到的项不写。
pub(crate) fn stream_facts(facts: &AudioStreamFacts) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(codec) = &facts.codec {
        parts.push(codec.clone());
    }
    if let Some(rate) = facts.sample_rate {
        parts.push(sample_rate_text(rate));
    }
    if let Some(bits) = facts.bit_depth {
        parts.push(tr!("%1-bit", bits));
    }
    if let Some(channels) = facts.channels {
        parts.push(match channels {
            1 => tr!("Mono"),
            2 => tr!("Stereo"),
            count => tr!("%1 channels", count),
        });
    }
    if let Some(bit_rate) = facts.bit_rate {
        parts.push(format!("{} kbps", (bit_rate + 500) / 1000));
    }
    parts
}

/// 把几项用「 · 」连成一行。窄面板会折行：项里的空格（44.1 kHz）和分隔点前的空格都换成
/// 不换行空格，折行只能落在分隔点后面，新的一行不会以「·」开头，也不会把一项拆成两半。
pub(crate) fn fact_line(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect::<Vec<_>>()
        .join("\u{a0}· ")
}

/// 44100 → 44.1 kHz，48000 → 48 kHz。
fn sample_rate_text(rate: u32) -> String {
    let kilohertz = f64::from(rate) / 1000.0;
    if rate.is_multiple_of(1000) {
        format!("{kilohertz:.0} kHz")
    } else {
        format!("{kilohertz:.1} kHz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_falls_back_to_the_file_name() {
        let tags = AudioTags::default();
        assert_eq!(
            track_title(Path::new("/m/01 Intro.flac"), &tags),
            "01 Intro"
        );
        let tags = AudioTags {
            title: Some("Intro".to_owned()),
            ..AudioTags::default()
        };
        assert_eq!(track_title(Path::new("/m/01 Intro.flac"), &tags), "Intro");
    }

    #[test]
    fn subtitle_joins_whatever_is_known() {
        let mut tags = AudioTags {
            artist: Some("A".to_owned()),
            album: Some("B".to_owned()),
            ..AudioTags::default()
        };
        assert_eq!(track_subtitle(&tags).as_deref(), Some("A · B"));
        tags.artist = None;
        assert_eq!(track_subtitle(&tags).as_deref(), Some("B"));
        tags.album = None;
        assert_eq!(track_subtitle(&tags), None);
    }

    #[test]
    fn stream_summary_skips_unknown_facts() {
        let facts = AudioStreamFacts {
            codec: Some("FLAC".to_owned()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bit_rate: Some(1_411_200),
        };
        assert_eq!(
            fact_line(stream_facts(&facts)),
            "FLAC\u{a0}· 44.1\u{a0}kHz\u{a0}· 16-bit\u{a0}· Stereo\u{a0}· 1411\u{a0}kbps"
        );
        let facts = AudioStreamFacts {
            codec: Some("Opus".to_owned()),
            sample_rate: Some(48_000),
            channels: Some(6),
            ..AudioStreamFacts::default()
        };
        assert_eq!(
            stream_facts(&facts),
            vec![
                "Opus".to_owned(),
                "48 kHz".to_owned(),
                "6 channels".to_owned()
            ]
        );
    }
}
