//! 字幕：同目录的同名外挂文件和视频里的字幕轨，选中的一条由 ffmpeg 解码时烧进画面
//! （ASS 的位置、颜色、特效都在，图片字幕也能显示）。
//!
//! 文字字幕不直接把视频路径交给 `subtitles` 滤镜：它在启动时要把整个文件读一遍取全部字幕，
//! 每次跳转重开解码都会再读一遍。所以文字字幕先在后台抽成一个小的 UTF-8 文件（每次预览一次），
//! 解码时只读它；外挂字幕也先转成 UTF-8 拷过去，顺便免掉滤镜参数里的路径转义问题。
//! 图片字幕（PGS/DVD）跟着视频流读，用 `overlay` 叠上去，不用先抽。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use bennu_localization::UiLanguage;
use tokio::process::Command;

use crate::video_preview::{SubtitleTrack, VideoProbe};

/// 认作外挂字幕的后缀（不分大小写）。
const EXTERNAL_SUBTITLE_EXTENSIONS: [&str; 4] = ["srt", "ass", "ssa", "vtt"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSubtitle {
    pub path: PathBuf,
    /// `电影.zh.srt` 里的 `zh`；`电影.srt` 没有。
    pub language: Option<String>,
}

/// 能选的一条字幕。
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleChoice {
    External(ExternalSubtitle),
    Embedded(SubtitleTrack),
}

impl SubtitleChoice {
    pub fn language(&self) -> Option<&str> {
        match self {
            SubtitleChoice::External(subtitle) => subtitle.language.as_deref(),
            SubtitleChoice::Embedded(track) => track.language.as_deref(),
        }
    }
}

/// 解码时怎么把字幕烧进去。
#[derive(Debug)]
pub enum SubtitleBurn {
    /// 准备好的 UTF-8 字幕文件（预览结束随最后一个引用删掉）。
    Text(tempfile::TempPath),
    /// 视频里第 `ordinal` 条字幕轨（图片字幕），叠在画面上。
    Image { ordinal: usize },
}

/// 视频同目录里同名的字幕文件：`电影.srt`、`电影.zh.srt`、`电影.chs.ass`……
/// 完全同名的排前面。读不了目录就当没有。
pub(crate) async fn find_external_subtitles(video: PathBuf) -> Vec<ExternalSubtitle> {
    tokio::task::spawn_blocking(move || external_subtitles_blocking(&video))
        .await
        .unwrap_or_default()
}

fn external_subtitles_blocking(video: &Path) -> Vec<ExternalSubtitle> {
    let (Some(directory), Some(stem)) = (video.parent(), video.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (base, extension) = name.rsplit_once('.')?;
            if !EXTERNAL_SUBTITLE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
            {
                return None;
            }
            let language = if base == stem {
                None
            } else {
                let tag = base.strip_prefix(stem.as_ref())?.strip_prefix('.')?;
                Some(tag.split('.').next().unwrap_or(tag).to_owned())
            };
            Some(ExternalSubtitle {
                path: entry.path(),
                language,
            })
        })
        .collect::<Vec<_>>();
    found.sort_by(|left, right| {
        (left.language.is_some(), &left.path).cmp(&(right.language.is_some(), &right.path))
    });
    found
}

/// 外挂在前，然后是视频里的字幕轨。
pub(crate) fn subtitle_choices(
    external: Vec<ExternalSubtitle>,
    probe: &VideoProbe,
) -> Vec<SubtitleChoice> {
    external
        .into_iter()
        .map(SubtitleChoice::External)
        .chain(
            probe
                .subtitle_tracks
                .iter()
                .cloned()
                .map(SubtitleChoice::Embedded),
        )
        .collect()
}

/// 默认显示哪条：同名外挂（有和界面语言一致的先选它）> 标了「默认」的 > 和界面语言一致的 > 第一条。
pub(crate) fn default_subtitle(choices: &[SubtitleChoice], ui: UiLanguage) -> Option<usize> {
    let externals = || {
        choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| matches!(choice, SubtitleChoice::External(_)))
    };
    let matches_ui = |choice: &SubtitleChoice| {
        choice
            .language()
            .is_some_and(|language| language_matches(language, ui))
    };
    externals()
        .find(|(_, choice)| matches_ui(choice))
        .or_else(|| externals().next())
        .map(|(index, _)| index)
        .or_else(|| {
            choices.iter().position(
                |choice| matches!(choice, SubtitleChoice::Embedded(track) if track.is_default),
            )
        })
        .or_else(|| choices.iter().position(matches_ui))
        .or_else(|| (!choices.is_empty()).then_some(0))
}

/// 字幕语言标记和界面语言是不是一种：ISO 639-1/2 代码和常见的中文写法（chs、cht、sc、tc）。
fn language_matches(language: &str, ui: UiLanguage) -> bool {
    let language = language.to_ascii_lowercase();
    let primary = language.split(['-', '_']).next().unwrap_or(&language);
    match ui {
        UiLanguage::Chinese => matches!(
            primary,
            "zh" | "chi"
                | "zho"
                | "chs"
                | "cht"
                | "sc"
                | "tc"
                | "cn"
                | "cmn"
                | "yue"
                | "gb"
                | "big5"
        ),
        UiLanguage::English => matches!(primary, "en" | "eng"),
    }
}

/// 准备好要烧进去的字幕。文字字幕会读文件（视频里的要把整个视频读一遍），在后台跑。
pub(crate) async fn prepare_subtitle(
    video: PathBuf,
    choice: SubtitleChoice,
) -> Result<SubtitleBurn, String> {
    match choice {
        SubtitleChoice::Embedded(track) if track.is_image => Ok(SubtitleBurn::Image {
            ordinal: track.ordinal,
        }),
        SubtitleChoice::Embedded(track) => extract_embedded(&video, track.ordinal).await,
        SubtitleChoice::External(subtitle) => {
            tokio::task::spawn_blocking(move || copy_external_as_utf8(&subtitle.path))
                .await
                .map_err(|error| format!("could not prepare the subtitle: {error}"))?
        }
    }
}

async fn extract_embedded(video: &Path, ordinal: usize) -> Result<SubtitleBurn, String> {
    let file = subtitle_file("ass")?;
    let output = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .arg("-map")
        .arg(format!("0:s:{ordinal}"))
        .args(["-c:s", "ass", "-f", "ass"])
        .arg(file.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("ffmpeg failed");
        return Err(format!(
            "could not read the subtitle track: {}",
            reason.trim()
        ));
    }
    Ok(SubtitleBurn::Text(file.into_temp_path()))
}

/// 外挂字幕常是 GBK/Big5：按 file-core 的编码识别转成 UTF-8，后缀不变（滤镜按后缀认格式）。
fn copy_external_as_utf8(path: &Path) -> Result<SubtitleBurn, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("could not read the subtitle: {error}"))?;
    let text = match file_core::text_encoding::sniff_text_encoding(&bytes, true) {
        Some(sniffed) => sniffed
            .encoding
            .decode_without_bom_handling(&bytes[sniffed.bom_length..])
            .0
            .into_owned(),
        None => String::from_utf8_lossy(&bytes).into_owned(),
    };
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "srt".to_owned());
    let mut file = subtitle_file(&extension)?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("could not prepare the subtitle: {error}"))?;
    Ok(SubtitleBurn::Text(file.into_temp_path()))
}

fn subtitle_file(extension: &str) -> Result<tempfile::NamedTempFile, String> {
    tempfile::Builder::new()
        .prefix("bennu-subtitle-")
        .suffix(&format!(".{extension}"))
        .tempfile()
        .map_err(|error| format!("could not prepare the subtitle: {error}"))
}

/// `subtitles` 滤镜：参数值和滤镜图两层都要转义（路径里的 `:`、`'`、`,`、`[` 等）。
pub(crate) fn text_subtitle_filter(path: &Path) -> String {
    let value = escape(&path.to_string_lossy(), &['\\', '\'', ':']);
    format!(
        "subtitles=filename={}",
        escape(&value, &['\\', '\'', '[', ']', ',', ';'])
    )
}

fn escape(text: &str, special: &[char]) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if special.contains(&character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
#[path = "subtitles_tests.rs"]
mod tests;
