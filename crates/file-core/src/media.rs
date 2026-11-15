use std::io::Read;
use std::path::Path;

use crate::mime::{MimeDatabase, NameType, TEXT_PLAIN};

/// 类型库把这几种播放列表挂在 audio/ 下，但它们是指向别处的清单，不是音频数据，
/// 也没声明继承 text/plain（m3u 声明了，按文本处理）。
const AUDIO_PLAYLIST_TYPES: [&str; 3] = ["audio/x-scpls", "audio/x-ms-asx", "audio/x-iriver-pla"];
/// 类型库把这几种影音容器挂在 application/ 下（rm/rmvb、asf、mxf），里面装的是视频。
const VIDEO_CONTAINER_TYPES: [&str; 3] = [
    "application/vnd.rn-realmedia",
    "application/vnd.ms-asf",
    "application/mxf",
];
/// 像素图：内置解码器认得的自己解，其余交给 ffmpeg（见 `image_decode`）。只列画得出来的；
/// 类型库里还有 XCF、EPS、DjVu、WMF、CAD、3D 之类挂在 image/ 下但没法当图片画的，不算。
const PICTURE_IMAGE_TYPES: [&str; 35] = [
    "image/png",
    "image/apng",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/vnd.microsoft.icon",
    "image/tiff",
    "image/x-tiff-multipage",
    "image/avif",
    "image/heif",
    "image/jxl",
    "image/vnd.adobe.photoshop",
    "image/x-tga",
    "image/x-exr",
    "image/vnd.radiance",
    "image/vnd.ms-dds",
    "image/qoi",
    "image/x-portable-anymap",
    "image/x-portable-bitmap",
    "image/x-portable-graymap",
    "image/x-portable-pixmap",
    "image/x-pfm",
    "image/x-phm",
    "image/jp2",
    "image/x-jp2-codestream",
    "image/x-sgi",
    "image/x-rgb",
    "image/x-sun-raster",
    "image/x-xwindowdump",
    "image/vnd.zbrush.pcx",
    "image/x-photo-cd",
    "image/x-ilbm",
    "image/dpx",
    "image/vnd.wap.wbmp",
];
/// 相机 RAW：显示里面自带的 JPEG 预览图。只列预览图放在我们认得的位置（TIFF 目录、
/// RAF 文件头、CR3 第一条轨道）的。
const CAMERA_RAW_IMAGE_TYPES: [&str; 23] = [
    "image/x-adobe-dng",
    "image/x-canon-cr2",
    "image/x-canon-cr3",
    "image/x-nikon-nef",
    "image/x-nikon-nrw",
    "image/x-sony-arw",
    "image/x-sony-sr2",
    "image/x-sony-srf",
    "image/x-olympus-orf",
    "image/x-panasonic-rw2",
    "image/x-panasonic-rw",
    "image/x-pentax-pef",
    "image/x-samsung-srw",
    "image/x-fuji-raf",
    "image/x-epson-erf",
    "image/x-hasselblad-3fr",
    "image/x-hasselblad-fff",
    "image/x-kodak-dcr",
    "image/x-kodak-kdc",
    "image/x-kodak-k25",
    "image/x-mamiya-mef",
    "image/x-leaf-mos",
    "image/x-phaseone-iiq",
];
const VECTOR_IMAGE_TYPES: [&str; 2] = ["image/svg+xml", "image/svg+xml-compressed"];

/// 图片按怎么画分：SVG 按显示大小现画，其余解成像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Vector,
    Raster(RasterSource),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterSource {
    /// 普通像素图。
    Picture,
    /// 相机 RAW，画的是里面自带的预览图。
    CameraRaw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedMediaKind {
    Audio,
    Image,
    Video,
}

/// 只看名字对「是不是视频」的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoByName {
    Yes,
    No,
    /// 名字对应的几种类型里只有一部分是视频（`.ts`：电视录像 / Qt 翻译），要看开头内容。
    AskContent,
}

/// 会读文件：名字定不下来的视频后缀要看开头内容。
pub fn media_kind_for_file(path: impl AsRef<Path>) -> Option<SupportedMediaKind> {
    let path = path.as_ref();
    if is_image_by_name(path) {
        Some(SupportedMediaKind::Image)
    } else if is_video_file(path) {
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
    match types.type_from_name(file_name) {
        NameType::Known(mime) => is_audio_type(&types, &mime),
        NameType::Candidates(candidates) => {
            candidates.iter().any(|mime| is_audio_type(&types, mime))
        }
        NameType::Unknown => false,
    }
}

/// 视频同样交给 ffmpeg，按类型库认。名字认不出类型的不读内容，不算视频。
pub fn video_by_name(path: impl AsRef<Path>) -> VideoByName {
    let Some(file_name) = path.as_ref().file_name() else {
        return VideoByName::No;
    };
    let types = MimeDatabase::system();
    video_verdict(&types, &types.type_from_name(file_name))
}

/// 不读文件的场合（预览分类）：名字定不下来的不算，交给打开时的内容检查。
pub fn is_video_by_name(path: impl AsRef<Path>) -> bool {
    video_by_name(path) == VideoByName::Yes
}

/// 名字加上开头内容（`head` 至少给 `MimeDatabase::magic_extent()` 字节，文件更短就给全部）。
pub fn is_video_content(name: &NameType, head: &[u8]) -> bool {
    video_from_content(&MimeDatabase::system(), name, head)
}

/// 本来就要读文件的场合（生成缩略图、打开「转换」）：名字定不下来时读开头；读不了不算视频。
pub fn is_video_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let types = MimeDatabase::system();
    let name = types.type_from_name(file_name);
    match video_verdict(&types, &name) {
        VideoByName::Yes => true,
        VideoByName::No => false,
        VideoByName::AskContent => read_head(path, types.magic_extent())
            .is_some_and(|head| video_from_content(&types, &name, &head)),
    }
}

/// 候选全是视频才算视频；候选里有音频的（`.ogg`、`.mp2`）归音频——分类时视频先于音频，
/// 否则会把它们抢走；也不让音乐文件夹里每个 `.ogg` 都为缩略图读一次内容。
fn video_verdict(types: &MimeDatabase, name: &NameType) -> VideoByName {
    match name {
        NameType::Known(mime) if is_video_type(types, mime) => VideoByName::Yes,
        NameType::Candidates(candidates) => {
            let videos = candidates
                .iter()
                .filter(|mime| is_video_type(types, mime))
                .count();
            if videos == candidates.len() {
                VideoByName::Yes
            } else if videos == 0 || candidates.iter().any(|mime| is_audio_type(types, mime)) {
                VideoByName::No
            } else {
                VideoByName::AskContent
            }
        }
        NameType::Known(_) | NameType::Unknown => VideoByName::No,
    }
}

fn video_from_content(types: &MimeDatabase, name: &NameType, head: &[u8]) -> bool {
    match video_verdict(types, name) {
        VideoByName::Yes => true,
        VideoByName::No => false,
        VideoByName::AskContent => types
            .type_from_content(name, head)
            .is_some_and(|mime| is_video_type(types, &mime)),
    }
}

fn is_audio_type(types: &MimeDatabase, mime: &str) -> bool {
    let mime = types.canonical(mime);
    mime.starts_with("audio/")
        && !types.is_subclass(&mime, TEXT_PLAIN)
        && !AUDIO_PLAYLIST_TYPES.contains(&mime.as_str())
}

/// 播放列表 video/vnd.mpegurl 挂在 video/ 下但继承 text/plain，不算。
fn is_video_type(types: &MimeDatabase, mime: &str) -> bool {
    let mime = types.canonical(mime);
    (mime.starts_with("video/") && !types.is_subclass(&mime, TEXT_PLAIN))
        || VIDEO_CONTAINER_TYPES.contains(&mime.as_str())
}

fn read_head(path: &Path, extent: usize) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(extent as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// 图片也按类型库认，只看名字：列表里每个条目都要问，不能读文件。
pub fn image_kind_by_name(path: impl AsRef<Path>) -> Option<ImageKind> {
    let file_name = path.as_ref().file_name()?;
    let types = MimeDatabase::system();
    match types.type_from_name(file_name) {
        NameType::Known(mime) => image_kind_of_type(&types, &mime),
        // 候选全是图片才算：名字同时可能是文本之类的，不为它去起解码器。
        NameType::Candidates(candidates) => candidates
            .iter()
            .map(|mime| image_kind_of_type(&types, mime))
            .collect::<Option<Vec<_>>>()?
            .first()
            .copied(),
        NameType::Unknown => None,
    }
}

pub fn is_image_by_name(path: impl AsRef<Path>) -> bool {
    image_kind_by_name(path).is_some()
}

fn image_kind_of_type(types: &MimeDatabase, mime: &str) -> Option<ImageKind> {
    let mime = types.canonical(mime);
    let mime = mime.as_str();
    if PICTURE_IMAGE_TYPES.contains(&mime) {
        Some(ImageKind::Raster(RasterSource::Picture))
    } else if CAMERA_RAW_IMAGE_TYPES.contains(&mime) {
        Some(ImageKind::Raster(RasterSource::CameraRaw))
    } else if VECTOR_IMAGE_TYPES.contains(&mime) {
        Some(ImageKind::Vector)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        image_kind_by_name, is_audio_by_name, is_image_by_name, is_video_by_name, is_video_file,
        media_kind_for_file, video_by_name, ImageKind, RasterSource, SupportedMediaKind,
        VideoByName,
    };

    /// 五个 188 字节的 MPEG-TS 包，同步字节 0x47 打头。
    fn transport_stream_bytes() -> Vec<u8> {
        let mut bytes = vec![0; 188 * 5];
        for packet in bytes.chunks_mut(188) {
            packet[0] = 0x47;
        }
        bytes
    }

    #[test]
    fn detects_supported_media_kinds_case_insensitively() {
        assert_eq!(
            media_kind_for_file("song.MP3"),
            Some(SupportedMediaKind::Audio)
        );
        assert_eq!(
            media_kind_for_file("photo.AVIF"),
            Some(SupportedMediaKind::Image)
        );
        assert_eq!(
            media_kind_for_file("clip.MP4"),
            Some(SupportedMediaKind::Video)
        );
        assert_eq!(media_kind_for_file("notes.txt"), None);
    }

    #[test]
    fn exposes_specific_media_predicates() {
        assert!(is_audio_by_name("voice.m4a"));
        assert!(!is_audio_by_name("archive.zip"));
        assert!(!is_image_by_name("archive.zip"));
    }

    #[test]
    fn images_follow_the_type_library_by_name() {
        for path in [
            "a.avif", "a.bmp", "a.gif", "a.ico", "a.jpg", "a.JPEG", "a.png", "a.tif", "a.tiff",
            "a.webp", "a.heic", "a.HEIF", "a.jxl", "a.psd", "a.tga", "a.exr", "a.hdr", "a.dds",
            "a.qoi", "a.ppm", "a.pgm", "a.jp2", "a.pcx",
        ] {
            assert_eq!(
                image_kind_by_name(path),
                Some(ImageKind::Raster(RasterSource::Picture)),
                "{path}"
            );
            assert_eq!(
                media_kind_for_file(path),
                Some(SupportedMediaKind::Image),
                "{path}"
            );
        }
        for path in [
            "a.cr2", "a.CR3", "a.nef", "a.arw", "a.dng", "a.raf", "a.orf", "a.rw2", "a.pef",
        ] {
            assert_eq!(
                image_kind_by_name(path),
                Some(ImageKind::Raster(RasterSource::CameraRaw)),
                "{path}"
            );
        }
        assert_eq!(image_kind_by_name("a.svg"), Some(ImageKind::Vector));
        assert_eq!(image_kind_by_name("a.svgz"), Some(ImageKind::Vector));
        // 挂在 image/ 下但画不出来的、文本形式的图片、没后缀的都不算。
        for path in [
            "a.xcf", "a.eps", "a.djvu", "a.wmf", "a.xpm", "a.dwg", "photo", "a.txt",
        ] {
            assert!(!is_image_by_name(path), "{path}");
        }
    }

    #[test]
    fn audio_follows_the_type_library_by_name() {
        for path in [
            "a.mp3", "a.FLAC", "a.opus", "a.wma", "a.ape", "a.aiff", "a.mka", "a.wv",
        ] {
            assert!(is_audio_by_name(path), "{path}");
            assert_eq!(
                media_kind_for_file(path),
                Some(SupportedMediaKind::Audio),
                "{path}"
            );
        }
        // 播放列表不是音频数据；没后缀的不读内容。
        for path in ["list.m3u", "list.pls", "list.asx", "track01", "notes.txt"] {
            assert!(!is_audio_by_name(path), "{path}");
        }
    }

    #[test]
    fn video_follows_the_type_library_by_name() {
        for path in [
            "a.mp4", "a.M4V", "a.mkv", "a.mov", "a.webm", "a.avi", "a.flv", "a.wmv", "a.m2ts",
            "a.mpg", "a.vob", "a.3gp", "a.ogv", "a.rmvb", "a.rm", "a.asf", "a.mxf",
        ] {
            assert!(is_video_by_name(path), "{path}");
            assert_eq!(
                media_kind_for_file(path),
                Some(SupportedMediaKind::Video),
                "{path}"
            );
        }
        // 播放列表、没后缀的、音频不算；候选里有音频的归音频。
        for path in ["list.m4u", "movie", "a.mp3", "notes.txt", "a.ogg", "a.mp2"] {
            assert_eq!(video_by_name(path), VideoByName::No, "{path}");
        }
        assert_eq!(
            media_kind_for_file("a.ogg"),
            Some(SupportedMediaKind::Audio)
        );
        assert_eq!(video_by_name("clip.ts"), VideoByName::AskContent);
        assert!(!is_video_by_name("clip.ts"));
    }

    #[test]
    fn ambiguous_names_are_settled_by_content() {
        let directory = tempfile::tempdir().expect("temp dir");
        let recording = directory.path().join("recording.ts");
        std::fs::write(&recording, transport_stream_bytes()).expect("write recording");
        let code = directory.path().join("code.ts");
        std::fs::write(&code, "export const answer: number = 42;\n").expect("write code");

        assert!(is_video_file(&recording));
        assert_eq!(
            media_kind_for_file(&recording),
            Some(SupportedMediaKind::Video)
        );
        assert!(!is_video_file(&code));
        assert_eq!(media_kind_for_file(&code), None);
        // 读不了的不算视频。
        assert!(!is_video_file(directory.path().join("missing.ts")));
    }
}
