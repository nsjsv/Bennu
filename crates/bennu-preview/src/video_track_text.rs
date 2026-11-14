//! 视频的音轨、字幕在菜单和信息层里怎么写：语言名、标题、编码、声道。

use bennu_localization::tr;

use crate::video_preview::{AudioTrack, SubtitleChoice};

/// 常见语言代码（ISO 639-1/2 和中文字幕常用的 chs/cht）→ 名字；认不出的原样大写。
pub(crate) fn language_name(code: &str) -> String {
    let lower = code.to_ascii_lowercase();
    let primary = lower.split(['-', '_']).next().unwrap_or(&lower);
    match primary {
        "zh" | "chi" | "zho" | "cmn" | "yue" => tr!("Chinese"),
        "chs" | "sc" | "gb" => tr!("Simplified Chinese"),
        "cht" | "tc" | "big5" => tr!("Traditional Chinese"),
        "en" | "eng" => tr!("English"),
        "ja" | "jpn" => tr!("Japanese"),
        "ko" | "kor" => tr!("Korean"),
        "fr" | "fre" | "fra" => tr!("French"),
        "de" | "ger" | "deu" => tr!("German"),
        "es" | "spa" => tr!("Spanish"),
        "ru" | "rus" => tr!("Russian"),
        _ => code.to_ascii_uppercase(),
    }
}

/// 声道数的说法：单声道、立体声、5.1、7.1，其余写几声道。
pub(crate) fn channel_text(channels: u32) -> String {
    match channels {
        1 => tr!("Mono"),
        2 => tr!("Stereo"),
        6 => "5.1".to_owned(),
        8 => "7.1".to_owned(),
        count => tr!("%1 channels", count),
    }
}

/// 如「English · 导演评论 · AC3 5.1」；什么都没写的音轨叫「音轨 N」。
pub(crate) fn audio_track_label(track: &AudioTrack) -> String {
    let mut parts = Vec::new();
    if let Some(language) = &track.language {
        parts.push(language_name(language));
    }
    if let Some(title) = &track.title {
        parts.push(title.clone());
    }
    if parts.is_empty() {
        parts.push(tr!("Track %1", track.ordinal + 1));
    }
    let mut technical = track.codec.to_ascii_uppercase();
    if let Some(channels) = track.channels {
        technical = format!("{technical} {}", channel_text(channels));
    }
    parts.push(technical);
    parts.join(" · ")
}

/// 外挂字幕写文件名（看得出是哪个文件），视频里的写语言和标题。
pub(crate) fn subtitle_label(choice: &SubtitleChoice) -> String {
    match choice {
        SubtitleChoice::External(subtitle) => {
            let name = subtitle
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match &subtitle.language {
                Some(language) => format!("{} · {name}", language_name(language)),
                None => name,
            }
        }
        SubtitleChoice::Embedded(track) => {
            let mut parts = Vec::new();
            if let Some(language) = &track.language {
                parts.push(language_name(language));
            }
            if let Some(title) = &track.title {
                parts.push(title.clone());
            }
            if parts.is_empty() {
                parts.push(tr!("Track %1", track.ordinal + 1));
            }
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::video_preview::{ExternalSubtitle, SubtitleTrack};

    #[test]
    fn labels_name_language_title_and_codec() {
        let track = AudioTrack {
            ordinal: 1,
            codec: "ac3".to_owned(),
            channels: Some(6),
            sample_rate: Some(48_000),
            language: Some("eng".to_owned()),
            title: Some("Commentary".to_owned()),
            is_default: false,
        };
        assert_eq!(audio_track_label(&track), "English · Commentary · AC3 5.1");
        let bare = AudioTrack {
            language: None,
            title: None,
            channels: None,
            ..track
        };
        assert_eq!(audio_track_label(&bare), "Track 2 · AC3");

        let external = SubtitleChoice::External(ExternalSubtitle {
            path: PathBuf::from("/videos/movie.chs.ass"),
            language: Some("chs".to_owned()),
        });
        assert_eq!(
            subtitle_label(&external),
            "Simplified Chinese · movie.chs.ass"
        );
        let embedded = SubtitleChoice::Embedded(SubtitleTrack {
            ordinal: 0,
            codec: "subrip".to_owned(),
            is_image: false,
            language: Some("pt".to_owned()),
            title: None,
            is_default: false,
        });
        assert_eq!(subtitle_label(&embedded), "PT");
    }
}
