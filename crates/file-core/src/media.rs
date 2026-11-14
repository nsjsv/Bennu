use std::path::Path;

use crate::mime::{MimeDatabase, NameType, TEXT_PLAIN};

/// 类型库把这几种播放列表挂在 audio/ 下，但它们是指向别处的清单，不是音频数据，
/// 也没声明继承 text/plain（m3u 声明了，按文本处理）。
const AUDIO_PLAYLIST_TYPES: [&str; 3] = ["audio/x-scpls", "audio/x-ms-asx", "audio/x-iriver-pla"];
const SUPPORTED_IMAGE_EXTENSIONS: [&str; 11] = [
    "avif", "bmp", "gif", "ico", "jpg", "jpeg", "png", "svg", "tif", "tiff", "webp",
];
const SUPPORTED_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "m4v", "mkv", "mov", "webm", "avi"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedMediaKind {
    Audio,
    Image,
    Video,
}

pub fn supported_media_kind_for_path(path: impl AsRef<Path>) -> Option<SupportedMediaKind> {
    let path = path.as_ref();
    let extension = path.extension().and_then(std::ffi::OsStr::to_str);
    if extension.is_some_and(is_supported_image_extension) {
        Some(SupportedMediaKind::Image)
    } else if extension.is_some_and(is_supported_video_extension) {
        Some(SupportedMediaKind::Video)
    } else if is_audio_by_name(path) {
        Some(SupportedMediaKind::Audio)
    } else {
        None
    }
}

/// 音频交给 ffmpeg 解码，它能解的远不止几个常见后缀，所以按类型库认而不是维护
/// 后缀表。只看名字、不读内容：名字认不出类型的文件不算音频。
pub fn is_audio_by_name(path: impl AsRef<Path>) -> bool {
    let Some(file_name) = path.as_ref().file_name() else {
        return false;
    };
    let types = MimeDatabase::system();
    let is_audio = |mime: &String| {
        let mime = types.canonical(mime);
        mime.starts_with("audio/")
            && !types.is_subclass(&mime, TEXT_PLAIN)
            && !AUDIO_PLAYLIST_TYPES.contains(&mime.as_str())
    };
    match types.type_from_name(file_name) {
        NameType::Known(mime) => is_audio(&mime),
        NameType::Candidates(candidates) => candidates.iter().any(is_audio),
        NameType::Unknown => false,
    }
}

pub fn is_supported_image_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_supported_image_extension)
}

pub fn is_supported_image_extension(extension: &str) -> bool {
    extension_matches(extension, &SUPPORTED_IMAGE_EXTENSIONS)
}

pub fn is_supported_video_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_supported_video_extension)
}

pub fn is_supported_video_extension(extension: &str) -> bool {
    extension_matches(extension, &SUPPORTED_VIDEO_EXTENSIONS)
}

fn extension_matches(extension: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::{
        is_audio_by_name, is_supported_image_path, is_supported_video_path,
        supported_media_kind_for_path, SupportedMediaKind,
    };

    #[test]
    fn detects_supported_media_extensions_case_insensitively() {
        assert_eq!(
            supported_media_kind_for_path("song.MP3"),
            Some(SupportedMediaKind::Audio)
        );
        assert_eq!(
            supported_media_kind_for_path("photo.AVIF"),
            Some(SupportedMediaKind::Image)
        );
        assert_eq!(
            supported_media_kind_for_path("clip.MP4"),
            Some(SupportedMediaKind::Video)
        );
        assert_eq!(supported_media_kind_for_path("notes.txt"), None);
    }

    #[test]
    fn exposes_specific_media_predicates() {
        assert!(is_audio_by_name("voice.m4a"));
        for path in [
            "image.avif",
            "icon.bmp",
            "animation.gif",
            "favicon.ico",
            "photo.jpg",
            "photo.jpeg",
            "icon.png",
            "vector.svg",
            "scan.tif",
            "scan.tiff",
            "image.webp",
        ] {
            assert!(is_supported_image_path(path), "{path}");
        }
        assert!(is_supported_video_path("movie.webm"));
        assert!(!is_audio_by_name("archive.zip"));
        assert!(!is_supported_image_path("photo.heic"));
        assert!(!is_supported_image_path("photo.heif"));
    }

    #[test]
    fn audio_follows_the_type_library_by_name() {
        for path in [
            "a.mp3", "a.FLAC", "a.opus", "a.wma", "a.ape", "a.aiff", "a.mka", "a.wv",
        ] {
            assert!(is_audio_by_name(path), "{path}");
            assert_eq!(
                supported_media_kind_for_path(path),
                Some(SupportedMediaKind::Audio),
                "{path}"
            );
        }
        // 播放列表不是音频数据；没后缀的不读内容。
        for path in ["list.m3u", "list.pls", "list.asx", "track01", "notes.txt"] {
            assert!(!is_audio_by_name(path), "{path}");
        }
    }
}
