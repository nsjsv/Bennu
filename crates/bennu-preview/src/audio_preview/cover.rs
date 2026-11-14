//! 封面：优先文件里内嵌的（ffmpeg 原样拷出那条图片流，不重新编码），没有就找同目录里
//! 播放器约定俗成的封面图。解出来缩到预览用得上的尺寸，内嵌的 3000 像素大图不常驻内存。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use iced::widget::image;

const COVER_MAX_EDGE: u32 = 512;
/// 按优先级排：cover 最常见，其次 folder（Windows 媒体播放器写的）、front、album。
const FOLDER_COVER_STEMS: [&str; 5] = ["cover", "folder", "front", "album", "albumart"];
const FOLDER_COVER_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

pub async fn load_audio_cover(
    path: PathBuf,
    embedded_stream: Option<usize>,
) -> Option<image::Handle> {
    tokio::task::spawn_blocking(move || {
        let bytes = embedded_stream
            .and_then(|stream| embedded_cover_bytes(&path, stream))
            .or_else(|| std::fs::read(folder_cover_path(&path)?).ok())?;
        cover_handle(&bytes)
    })
    .await
    .ok()
    .flatten()
}

fn embedded_cover_bytes(path: &Path, stream: usize) -> Option<Vec<u8>> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .arg("-map")
        .arg(format!("0:{stream}"))
        .args(["-c", "copy", "-frames:v", "1", "-f", "image2pipe", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

fn folder_cover_path(audio: &Path) -> Option<PathBuf> {
    let directory = audio.parent()?;
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let rank = folder_cover_rank(&path)?;
            Some((rank, path))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, path)| path)
}

/// 名字（不分大小写，允许隐藏文件的前导点）在约定表里的位置；不是封面图返回 None。
fn folder_cover_rank(path: &Path) -> Option<usize> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !FOLDER_COVER_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let stem = stem.trim_start_matches('.');
    FOLDER_COVER_STEMS
        .iter()
        .position(|candidate| *candidate == stem)
}

fn cover_handle(bytes: &[u8]) -> Option<image::Handle> {
    let decoded = ::image::load_from_memory(bytes).ok()?;
    let fitted = if decoded.width() > COVER_MAX_EDGE || decoded.height() > COVER_MAX_EDGE {
        decoded.thumbnail(COVER_MAX_EDGE, COVER_MAX_EDGE)
    } else {
        decoded
    };
    let rgba = fitted.to_rgba8();
    Some(image::Handle::from_rgba(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_covers_are_ranked_by_convention() {
        assert_eq!(folder_cover_rank(Path::new("/m/Cover.JPG")), Some(0));
        assert_eq!(folder_cover_rank(Path::new("/m/.folder.png")), Some(1));
        assert_eq!(folder_cover_rank(Path::new("/m/AlbumArt.webp")), Some(4));
        assert_eq!(folder_cover_rank(Path::new("/m/cover.txt")), None);
        assert_eq!(folder_cover_rank(Path::new("/m/scan.jpg")), None);
    }

    #[test]
    fn folder_cover_prefers_cover_over_folder() {
        let directory = tempfile::tempdir().expect("temp dir");
        for name in ["song.flac", "Folder.jpg", "cover.png", "notes.txt"] {
            std::fs::write(directory.path().join(name), b"x").expect("write fixture");
        }
        assert_eq!(
            folder_cover_path(&directory.path().join("song.flac")),
            Some(directory.path().join("cover.png"))
        );
    }

    #[test]
    fn large_covers_are_shrunk_for_display() {
        let mut encoded = Vec::new();
        ::image::DynamicImage::new_rgb8(1200, 600)
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                ::image::ImageFormat::Png,
            )
            .expect("encode png");
        let handle = cover_handle(&encoded).expect("decode cover");
        let image::Handle::Rgba { width, height, .. } = handle else {
            panic!("rgba handle");
        };
        assert_eq!((width, height), (512, 256));
    }

    #[test]
    fn embedded_cover_is_copied_out_of_the_audio_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        if crate::audio_preview::test_fixtures::sine_fixture(directory.path(), "probe.wav", 1)
            .is_none()
        {
            return;
        }
        let song = directory.path().join("with-cover.mp3");
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=1",
            ])
            .args(["-f", "lavfi", "-i", "color=c=red:s=64x48:d=1"])
            .args([
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "libmp3lame",
                "-c:v",
                "mjpeg",
            ])
            .args(["-frames:v", "1", "-disposition:v", "attached_pic"])
            .arg(&song)
            .status()
            .expect("run ffmpeg");
        assert!(status.success());

        let bytes = embedded_cover_bytes(&song, 1).expect("embedded cover");
        let image::Handle::Rgba { width, height, .. } = cover_handle(&bytes).expect("decode")
        else {
            panic!("rgba handle");
        };
        assert_eq!((width, height), (64, 48));
    }
}
