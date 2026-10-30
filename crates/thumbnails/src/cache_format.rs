//! freedesktop 缩略图规范的磁盘格式：档目录、文件名、PNG 里的 `Thumb::*` 元数据。
//! 照 gnome-desktop 的缩略图工厂写，Nautilus、Dolphin 写的合规文件也能直接用。

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, BufReader, BufWriter};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use md5::{Digest, Md5};

use crate::ThumbnailSourceMetadata;

/// 规范里最大的一档（`xx-large`）。
pub const LARGEST_THUMBNAIL_EDGE: u32 = 1024;

const URI_KEY: &str = "Thumb::URI";
const MTIME_KEY: &str = "Thumb::MTime";
const SIZE_KEY: &str = "Thumb::Size";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThumbnailBucket {
    Normal,
    Large,
    XLarge,
    XXLarge,
}

impl ThumbnailBucket {
    pub(crate) const ALL: [Self; 4] = [Self::Normal, Self::Large, Self::XLarge, Self::XXLarge];

    /// 能装下请求边长的最小一档；更大的请求都落到最大一档。
    pub(crate) fn for_edge(max_edge: u32) -> Self {
        Self::ALL
            .into_iter()
            .find(|bucket| max_edge <= bucket.edge())
            .unwrap_or(Self::XXLarge)
    }

    pub(crate) fn edge(self) -> u32 {
        match self {
            Self::Normal => 128,
            Self::Large => 256,
            Self::XLarge => 512,
            Self::XXLarge => LARGEST_THUMBNAIL_EDGE,
        }
    }

    pub(crate) fn dir_name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Large => "large",
            Self::XLarge => "x-large",
            Self::XXLarge => "xx-large",
        }
    }
}

/// 规范的文件名：文件 URI 的 MD5（小写十六进制）加 `.png`。
pub(crate) fn thumbnail_file_path(cache_dir: &Path, uri: &str, bucket: ThumbnailBucket) -> PathBuf {
    let digest = Md5::digest(uri.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    cache_dir.join(bucket.dir_name()).join(name + ".png")
}

/// 同 GLib `g_filename_to_uri`：字母、数字和 `!$&'()*+,-./:=@_~` 原样，其它字节写成
/// 大写 `%XX`。差一个字符，算出的 MD5 就和 Nautilus 的对不上。
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"!$&'()*+,-./:=@_~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// 磁盘上的缩略图对得上源文件时返回它的宽高（同 gnome-desktop
/// `thumbnail_is_valid`）：URI 相同、修改时间整秒相同，写了大小的话大小也相同。
/// 不存在、读不了、对不上都返回 `None`，由调用方重新生成。
pub(crate) fn valid_thumbnail_dimensions(
    path: &Path,
    uri: &str,
    metadata: ThumbnailSourceMetadata,
) -> Option<(u32, u32)> {
    let source_mtime = unix_seconds(metadata)?;
    let reader = png::Decoder::new(BufReader::new(File::open(path).ok()?))
        .read_info()
        .ok()?;
    let info = reader.info();
    let texts = info
        .uncompressed_latin1_text
        .iter()
        .map(|chunk| (chunk.keyword.clone(), chunk.text.clone()))
        .chain(
            info.compressed_latin1_text
                .iter()
                .filter_map(|chunk| Some((chunk.keyword.clone(), chunk.get_text().ok()?))),
        )
        .chain(
            info.utf8_text
                .iter()
                .filter_map(|chunk| Some((chunk.keyword.clone(), chunk.get_text().ok()?))),
        )
        .collect::<Vec<_>>();
    let text = |key: &str| {
        texts
            .iter()
            .find(|(keyword, _)| keyword == key)
            .map(|(_, text)| text.as_str())
    };
    // 有的程序写成 "秒.微秒"，只比整秒。
    let mtime_matches = text(MTIME_KEY)
        .and_then(|mtime| mtime.split('.').next()?.parse::<u64>().ok())
        == Some(source_mtime);
    let size_matches = text(SIZE_KEY).is_none_or(|size| size.parse() == Ok(metadata.len));
    (text(URI_KEY) == Some(uri) && mtime_matches && size_matches)
        .then_some((info.width, info.height))
}

/// 先写同目录的临时文件（0600）再改名，别的程序不会读到半个文件。档目录按规范 0700。
pub(crate) fn write_thumbnail(
    image: &image::RgbaImage,
    output: &Path,
    temporary: &Path,
    uri: &str,
    metadata: ThumbnailSourceMetadata,
) -> io::Result<()> {
    if let Some(parent) = output.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temporary)?;
    let written = encode_thumbnail(image, file, uri, metadata)
        .and_then(|()| std::fs::rename(temporary, output));
    if written.is_err() {
        remove_temporary_file(temporary);
    }
    written
}

/// 删不掉只记一笔：临时文件以 `.png` 结尾，定期清理会按时间删掉。
pub(crate) fn remove_temporary_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        tracing::debug!(
            target: "thumbnails",
            path = ?path,
            %error,
            "temporary thumbnail was not removed"
        );
    }
}

fn encode_thumbnail(
    image: &image::RgbaImage,
    file: File,
    uri: &str,
    metadata: ThumbnailSourceMetadata,
) -> io::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_text_chunk(URI_KEY.to_owned(), uri.to_owned())
        .map_err(io::Error::other)?;
    if let Some(mtime) = unix_seconds(metadata) {
        encoder
            .add_text_chunk(MTIME_KEY.to_owned(), mtime.to_string())
            .map_err(io::Error::other)?;
    }
    encoder
        .add_text_chunk(SIZE_KEY.to_owned(), metadata.len.to_string())
        .map_err(io::Error::other)?;
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer
        .write_image_data(image.as_raw())
        .map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)
}

fn unix_seconds(metadata: ThumbnailSourceMetadata) -> Option<u64> {
    metadata
        .modified?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}
