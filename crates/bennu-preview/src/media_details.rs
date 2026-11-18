//! 属性窗口的「详细信息」：图片读文件头和 EXIF，音视频各跑一次 ffprobe。行文案和预览
//! 信息层是同一套（`image_info_text`、`video_track_text`、`audio_track_text`）。

use std::path::PathBuf;

use file_core::image_decode::{image_dimensions, image_facts, ImageDecodeError, ImageFacts};
use file_core::{ExternalTool, ImageKind, MediaByName};

use crate::audio_preview::{probe_audio_preview, AudioProbeError};
use crate::audio_track_text::audio_detail_rows;
use crate::image_info_text::image_detail_rows;
use crate::info_overlay::InfoRow;
use crate::preview_panel::tool_required_explanation;
use crate::video_preview::{probe_video_preview, SubtitleChoice, VideoProbeError};
use crate::video_track_text::video_detail_rows;

/// 不是图片、音视频的（含名字像视频、内容不是的 `.ts`）给空表。
pub async fn media_detail_rows(path: PathBuf, media: MediaByName) -> Result<Vec<InfoRow>, String> {
    match media {
        MediaByName::Image(ImageKind::Vector) => Ok(image_detail_rows(
            None,
            &ImageFacts {
                format: Some("SVG".to_owned()),
                ..ImageFacts::default()
            },
        )),
        MediaByName::Image(ImageKind::Raster(source)) => tokio::task::spawn_blocking(move || {
            let dimensions = image_dimensions(&path, source).map_err(|error| match error {
                ImageDecodeError::FfmpegMissing => tool_required_explanation(ExternalTool::Ffmpeg),
                error => error.to_string(),
            })?;
            Ok(image_detail_rows(
                Some(dimensions),
                &image_facts(&path, source),
            ))
        })
        .await
        .map_err(|error| error.to_string())?,
        MediaByName::Video => video_rows(path).await,
        MediaByName::MaybeVideo => {
            let content_path = path.clone();
            let is_video =
                tokio::task::spawn_blocking(move || file_core::is_video_file(content_path))
                    .await
                    .map_err(|error| error.to_string())?;
            if is_video {
                video_rows(path).await
            } else {
                Ok(Vec::new())
            }
        }
        MediaByName::Audio => match probe_audio_preview(path).await {
            Ok(probe) => Ok(audio_detail_rows(&probe)),
            Err(AudioProbeError::FfmpegMissing) => {
                Err(tool_required_explanation(ExternalTool::Ffmpeg))
            }
            Err(AudioProbeError::Failed(error)) => Err(error),
        },
        MediaByName::Other => Ok(Vec::new()),
    }
}

/// 只列视频里内嵌的字幕轨：同目录的外挂字幕不是这个文件的属性。
async fn video_rows(path: PathBuf) -> Result<Vec<InfoRow>, String> {
    let probe = probe_video_preview(path)
        .await
        .map_err(|error| match error {
            VideoProbeError::FfmpegMissing => tool_required_explanation(ExternalTool::Ffmpeg),
            VideoProbeError::Failed(error) => error,
        })?;
    let subtitles = probe
        .subtitle_tracks
        .iter()
        .cloned()
        .map(SubtitleChoice::Embedded)
        .collect::<Vec<_>>();
    Ok(video_detail_rows(&probe, &subtitles))
}
