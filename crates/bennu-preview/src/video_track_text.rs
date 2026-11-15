//! 视频的音轨、字幕在菜单和信息层里怎么写：语言名、标题、编码、声道；信息层的各行。

use bennu_localization::{format_duration, format_file_size, tr};

use crate::info_overlay::InfoRow;
use crate::preview::VideoPreviewDetails;
use crate::video_preview::{AudioTrack, SubtitleChoice, VideoStreamFacts, VideoTransfer};

/// 信息层：分辨率、帧率、时长、视频编码、码率、动态范围、各音轨、各字幕、容器、大小。
/// 都来自打开时那一次 ffprobe，读不到的行不写。
pub(crate) fn video_info_rows(details: &VideoPreviewDetails) -> Vec<InfoRow> {
    let probe = &details.probe;
    let mut rows = Vec::new();
    let mut push = |label: String, value: String| rows.push(InfoRow { label, value });
    if let Some(video) = &probe.video {
        push(tr!("Resolution"), resolution_text(video));
        if let Some(rate) = video.frame_rate {
            push(tr!("Frame rate"), frame_rate_text(rate));
        }
    }
    if let Some(duration) = probe.duration {
        push(tr!("Duration"), format_duration(duration));
    }
    if let Some(video) = &probe.video {
        let mut codec = video_codec_name(&video.codec);
        if let Some(bits) = video.pixel_format.as_deref().and_then(bit_depth) {
            codec = format!("{codec} · {}", tr!("%1-bit", bits));
        }
        push(tr!("Video"), codec);
    }
    if let Some(bit_rate) = probe.bit_rate {
        push(tr!("Bit rate"), bit_rate_text(bit_rate));
    }
    if let Some(video) = &probe.video {
        push(
            tr!("Dynamic range"),
            match video.color.transfer {
                VideoTransfer::Sdr => "SDR",
                VideoTransfer::Pq => "HDR10",
                VideoTransfer::Hlg => "HLG",
            }
            .to_owned(),
        );
    }
    for track in &probe.audio_tracks {
        push(tr!("Audio"), audio_track_label(track));
    }
    for choice in &details.subtitles {
        push(tr!("Subtitles"), subtitle_info(choice));
    }
    if let Some(container) = &probe.container {
        push(tr!("Container"), container.clone());
    }
    push(tr!("Size"), format_file_size(details.len));
    rows
}

/// 存的宽高；转了方向或像素不是方的，再写显示出来的宽高。
fn resolution_text(video: &VideoStreamFacts) -> String {
    let stored = format!("{} × {}", video.width, video.height);
    let (width, height) = video.display_size();
    let shown = (width.round() as u32, height.round() as u32);
    if shown == (video.width, video.height) {
        stored
    } else {
        format!("{stored} → {} × {}", shown.0, shown.1)
    }
}

/// 「23.976 fps」「30 fps」：最多三位小数，去掉末尾的 0。
fn frame_rate_text((numerator, denominator): (u32, u32)) -> String {
    let rate = format!("{:.3}", f64::from(numerator) / f64::from(denominator));
    let rate = rate.trim_end_matches('0').trim_end_matches('.');
    tr!("%1 fps", rate)
}

fn bit_rate_text(bits_per_second: u64) -> String {
    if bits_per_second >= 1_000_000 {
        format!("{:.1} Mbps", bits_per_second as f64 / 1_000_000.0)
    } else {
        format!("{} kbps", (bits_per_second + 500) / 1000)
    }
}

/// 像素格式里的位深（yuv420p10le → 10，p010le → 10）；8 位不写。nv12 这类名字里的数字不是位深。
fn bit_depth(pixel_format: &str) -> Option<u32> {
    let name = pixel_format
        .strip_suffix("le")
        .or_else(|| pixel_format.strip_suffix("be"))
        .unwrap_or(pixel_format);
    let digits_start = name.rfind(|character: char| !character.is_ascii_digit())? + 1;
    if !name[..digits_start].ends_with('p') {
        return None;
    }
    name[digits_start..]
        .parse::<u32>()
        .ok()
        .filter(|bits| *bits > 8)
}

/// 编码名给人看的写法；没收录的原样大写。
fn video_codec_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264",
        "hevc" => "HEVC (H.265)",
        "av1" => "AV1",
        "vp9" => "VP9",
        "vp8" => "VP8",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "mpeg1video" => "MPEG-1",
        "vc1" => "VC-1",
        "wmv3" => "WMV 9",
        "prores" => "ProRes",
        "rv40" => "RealVideo 4",
        "rv30" => "RealVideo 3",
        "theora" => "Theora",
        "mjpeg" => "Motion JPEG",
        other => return other.to_ascii_uppercase(),
    }
    .to_owned()
}

/// 字幕在信息层里再写清是外挂还是内嵌、文字还是图片。
fn subtitle_info(choice: &SubtitleChoice) -> String {
    let kind = match choice {
        SubtitleChoice::External(_) => tr!("External file"),
        SubtitleChoice::Embedded(track) if track.is_image => {
            format!("{} · {}", tr!("Embedded track"), tr!("Image"))
        }
        SubtitleChoice::Embedded(track) => {
            format!(
                "{} · {}",
                tr!("Embedded track"),
                track.codec.to_ascii_uppercase()
            )
        }
    };
    format!("{} · {kind}", subtitle_label(choice))
}

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
    fn info_details_are_written_for_people() {
        assert_eq!(frame_rate_text((24000, 1001)), "23.976 fps");
        assert_eq!(frame_rate_text((30, 1)), "30 fps");
        assert_eq!(bit_rate_text(8_460_000), "8.5 Mbps");
        assert_eq!(bit_rate_text(640_000), "640 kbps");
        assert_eq!(bit_depth("yuv420p10le"), Some(10));
        assert_eq!(bit_depth("p010le"), Some(10));
        assert_eq!(bit_depth("yuv420p"), None);
        assert_eq!(bit_depth("nv12"), None);
        assert_eq!(video_codec_name("hevc"), "HEVC (H.265)");
        assert_eq!(video_codec_name("ffv1"), "FFV1");
    }

    #[test]
    fn info_rows_cover_picture_tracks_container_and_size() {
        use crate::video_preview::{VideoColor, VideoPrimaries, VideoProbe, YuvMatrix};
        let details = VideoPreviewDetails {
            len: 1024 * 1024 * 1024,
            probe: VideoProbe {
                duration: Some(std::time::Duration::from_secs(3725)),
                container: Some("Matroska / WebM".to_owned()),
                bit_rate: Some(12_000_000),
                video: Some(VideoStreamFacts {
                    ordinal: 0,
                    codec: "hevc".to_owned(),
                    width: 1080,
                    height: 1920,
                    sample_aspect: (1, 1),
                    frame_rate: Some((60, 1)),
                    rotation: 90,
                    pixel_format: Some("yuv420p10le".to_owned()),
                    color: VideoColor {
                        matrix: YuvMatrix::Bt2020,
                        transfer: VideoTransfer::Hlg,
                        primaries: VideoPrimaries::Bt2020,
                    },
                    interlaced: false,
                    bit_rate: None,
                }),
                audio_tracks: Vec::new(),
                subtitle_tracks: Vec::new(),
            },
            subtitles: vec![SubtitleChoice::External(ExternalSubtitle {
                path: PathBuf::from("/videos/clip.srt"),
                language: None,
            })],
        };
        let rows = video_info_rows(&details)
            .into_iter()
            .map(|row| (row.label, row.value))
            .collect::<Vec<_>>();
        assert!(rows.contains(&(
            "Resolution".to_owned(),
            "1080 × 1920 → 1920 × 1080".to_owned()
        )));
        assert!(rows.contains(&("Video".to_owned(), "HEVC (H.265) · 10-bit".to_owned())));
        assert!(rows.contains(&("Dynamic range".to_owned(), "HLG".to_owned())));
        assert!(rows.contains(&(
            "Subtitles".to_owned(),
            "clip.srt · External file".to_owned()
        )));
        assert!(rows.contains(&("Container".to_owned(), "Matroska / WebM".to_owned())));
        assert_eq!(rows.last().map(|row| row.0.as_str()), Some("Size"));
    }

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
