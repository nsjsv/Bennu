use std::path::Path;

const SUPPORTED_AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "wav", "flac", "ogg", "oga", "m4a", "aac"];
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
    let extension = path
        .as_ref()
        .extension()
        .and_then(std::ffi::OsStr::to_str)?;
    if is_supported_audio_extension(extension) {
        Some(SupportedMediaKind::Audio)
    } else if is_supported_image_extension(extension) {
        Some(SupportedMediaKind::Image)
    } else if is_supported_video_extension(extension) {
        Some(SupportedMediaKind::Video)
    } else {
        None
    }
}

pub fn is_supported_audio_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_supported_audio_extension)
}

pub fn is_supported_audio_extension(extension: &str) -> bool {
    extension_matches(extension, &SUPPORTED_AUDIO_EXTENSIONS)
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
        is_supported_audio_path, is_supported_image_path, is_supported_video_path,
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
        assert!(is_supported_audio_path("voice.m4a"));
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
        assert!(!is_supported_audio_path("archive.zip"));
        assert!(!is_supported_image_path("photo.heic"));
        assert!(!is_supported_image_path("photo.heif"));
    }
}
