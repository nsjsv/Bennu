#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use image::{metadata::Orientation, ImageDecoder};
use thiserror::Error;

mod cache_format;
mod purge;
mod svg;

pub use cache_format::{file_uri, LARGEST_THUMBNAIL_EDGE};
use cache_format::{
    remove_temporary_file, thumbnail_file_path, valid_thumbnail_dimensions, write_thumbnail,
    ThumbnailBucket,
};
pub use purge::keep_thumbnail_cache_trimmed;

const VIDEO_THUMBNAIL_SEEK_TIME: &str = "00:00:01";
const VIDEO_THUMBNAILER_DETAIL_CHAR_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbnailMediaKind {
    Image,
    Video,
}

/// 本进程内认缩略图条目用：源路径、大小、修改时间和尺寸档相同就是同一项。
/// 磁盘上的文件名另按规范算（见 `cache_format`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey(String);

impl ThumbnailKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailSourceMetadata {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl From<&file_core::EntryMetadata> for ThumbnailSourceMetadata {
    fn from(metadata: &file_core::EntryMetadata) -> Self {
        Self {
            len: metadata.len,
            modified: metadata.modified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailRequest {
    pub source: PathBuf,
    pub metadata: ThumbnailSourceMetadata,
    pub max_edge: u32,
}

impl ThumbnailRequest {
    pub fn new(source: impl AsRef<Path>, metadata: ThumbnailSourceMetadata, max_edge: u32) -> Self {
        Self {
            source: source.as_ref().to_path_buf(),
            metadata,
            max_edge,
        }
    }

    pub fn key(&self) -> ThumbnailKey {
        let mut hasher = blake3::Hasher::new();
        update_hash(&mut hasher, request_path_bytes(&self.source));
        update_hash(&mut hasher, &self.metadata.len.to_le_bytes());
        update_system_time_hash(&mut hasher, self.metadata.modified);
        update_hash(&mut hasher, &self.bucket().edge().to_le_bytes());
        ThumbnailKey(hasher.finalize().to_hex().to_string())
    }

    fn bucket(&self) -> ThumbnailBucket {
        ThumbnailBucket::for_edge(self.max_edge)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedThumbnail {
    pub key: ThumbnailKey,
    pub source: PathBuf,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    pub cache_hit: bool,
}

#[derive(Debug, Error)]
pub enum ThumbnailError {
    #[error("unsupported thumbnail format for {path:?}")]
    UnsupportedFormat { path: PathBuf },
    #[error("thumbnail source {path:?} is inside thumbnail cache directory {cache_dir:?}")]
    SourceInsideCacheDirectory { path: PathBuf, cache_dir: PathBuf },
    #[error("thumbnail task failed: {0}")]
    Join(String),
    #[error("could not read image {path:?}: {source}")]
    ReadImage {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("could not read SVG image {path:?}: {source}")]
    ReadSvg {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse SVG image {path:?}: {source}")]
    ParseSvg {
        path: PathBuf,
        #[source]
        source: resvg::usvg::Error,
    },
    #[error("could not render SVG image {path:?}")]
    RenderSvg { path: PathBuf },
    #[error("could not read the frame the video thumbnailer wrote to {path:?}: {source}")]
    ReadVideoFrame {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("could not write thumbnail {path:?}: {source}")]
    WriteThumbnail {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not find ffmpegthumbnailer or ffmpeg to generate video thumbnail for {path:?}")]
    VideoThumbnailerUnavailable { path: PathBuf },
    #[error("could not run video thumbnailer {command} for {path:?}: {source}")]
    RunVideoThumbnailer {
        path: PathBuf,
        command: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("video thumbnailer {command} failed for {path:?}: {message}")]
    GenerateVideoThumbnail {
        path: PathBuf,
        command: &'static str,
        message: String,
    },
}

pub fn is_supported_thumbnail_path(path: impl AsRef<Path>) -> bool {
    thumbnail_media_kind_for_path(path.as_ref()).is_some()
}

/// freedesktop 缩略图规范的共享缓存目录，主程序和文件选择器都只从这里取。
/// `XDG_CACHE_HOME` 只认绝对路径（规范要求），否则用 `$HOME/.cache`；两者都没有时
/// 返回 `None`，调用方不生成缩略图，而不是写到相对路径里。
pub fn thumbnail_cache_dir() -> Option<PathBuf> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })?;
    Some(cache_home.join("thumbnails"))
}

pub fn path_is_in_thumbnail_cache(cache_dir: impl AsRef<Path>, path: impl AsRef<Path>) -> bool {
    path.as_ref().starts_with(cache_dir)
}

/// 请求在缓存里对应的位置。
struct CacheSlot {
    media_kind: ThumbnailMediaKind,
    key: ThumbnailKey,
    uri: String,
    output: PathBuf,
}

impl CacheSlot {
    fn for_request(cache_dir: &Path, request: &ThumbnailRequest) -> Result<Self, ThumbnailError> {
        let media_kind = thumbnail_media_kind_for_path(&request.source).ok_or_else(|| {
            ThumbnailError::UnsupportedFormat {
                path: request.source.clone(),
            }
        })?;
        if path_is_in_thumbnail_cache(cache_dir, &request.source) {
            return Err(ThumbnailError::SourceInsideCacheDirectory {
                path: request.source.clone(),
                cache_dir: cache_dir.to_path_buf(),
            });
        }
        let uri = file_uri(&request.source);
        Ok(Self {
            media_kind,
            key: request.key(),
            output: thumbnail_file_path(cache_dir, &uri, request.bucket()),
            uri,
        })
    }

    /// 磁盘上有对得上的缩略图（不论是谁写的）时返回它。
    async fn cached(
        self,
        request: ThumbnailRequest,
    ) -> Result<Result<CachedThumbnail, Self>, ThumbnailError> {
        let output = self.output.clone();
        let uri = self.uri.clone();
        let metadata = request.metadata;
        let dimensions = tokio::task::spawn_blocking(move || {
            valid_thumbnail_dimensions(&output, &uri, metadata)
        })
        .await
        .map_err(|source| ThumbnailError::Join(source.to_string()))?;
        Ok(match dimensions {
            Some((width, height)) => Ok(CachedThumbnail {
                key: self.key,
                source: request.source,
                output: self.output,
                width,
                height,
                cache_hit: true,
            }),
            None => Err(self),
        })
    }
}

pub async fn load_or_generate_thumbnail(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
) -> Result<CachedThumbnail, ThumbnailError> {
    let started_at = Instant::now();
    let slot = CacheSlot::for_request(cache_dir.as_ref(), &request)?;
    tracing::debug!(
        target: "thumbnails",
        source = ?request.source,
        output = ?slot.output,
        media_kind = ?slot.media_kind,
        max_edge = request.max_edge,
        "thumbnail requested"
    );
    let slot = match slot.cached(request.clone()).await? {
        Ok(cached) => {
            tracing::debug!(
                target: "thumbnails",
                source = ?cached.source,
                output = ?cached.output,
                width = cached.width,
                height = cached.height,
                elapsed_ms = started_at.elapsed().as_millis(),
                "thumbnail cache hit"
            );
            return Ok(cached);
        }
        Err(slot) => slot,
    };
    // 图片解码和外部视频缩略图命令都会阻塞，必须留在 blocking 线程池。
    tokio::task::spawn_blocking(move || generate_thumbnail_blocking(request, slot))
        .await
        .map_err(|source| ThumbnailError::Join(source.to_string()))?
}

pub async fn load_cached_thumbnail(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
) -> Result<Option<CachedThumbnail>, ThumbnailError> {
    let slot = CacheSlot::for_request(cache_dir.as_ref(), &request)?;
    Ok(slot.cached(request).await?.ok())
}

/// 按档的边长生成（不按请求的边长），写成规范的 PNG 后原子替换旧文件。
fn generate_thumbnail_blocking(
    request: ThumbnailRequest,
    slot: CacheSlot,
) -> Result<CachedThumbnail, ThumbnailError> {
    let edge = request.bucket().edge();
    let temporary = temporary_output_path(&slot.output);
    let image = match slot.media_kind {
        ThumbnailMediaKind::Image if is_svg_path(&request.source) => {
            svg::render_svg_thumbnail(&request.source, edge)?
        }
        ThumbnailMediaKind::Image => raster_thumbnail(&request.source, edge)?,
        ThumbnailMediaKind::Video => video_thumbnail(&request.source, &temporary, edge)?,
    };
    write_thumbnail(
        &image,
        &slot.output,
        &temporary,
        &slot.uri,
        request.metadata,
    )
    .map_err(|source| ThumbnailError::WriteThumbnail {
        path: slot.output.clone(),
        source,
    })?;
    Ok(CachedThumbnail {
        key: slot.key,
        source: request.source,
        output: slot.output,
        width: image.width(),
        height: image.height(),
        cache_hit: false,
    })
}

/// 比这一档小的原图不放大（同 gnome-desktop），显示时再缩放。
fn raster_thumbnail(source: &Path, edge: u32) -> Result<image::RgbaImage, ThumbnailError> {
    let (image, orientation) = decode_raster_with_orientation(source)?;
    let mut thumbnail = if image.width() > edge || image.height() > edge {
        image.thumbnail(edge, edge)
    } else {
        image
    };
    thumbnail.apply_orientation(orientation);
    Ok(thumbnail.to_rgba8())
}

/// 视频缩略图命令只会写普通 PNG：先写到临时位置，读回后再按规范写一遍。
fn video_thumbnail(
    source: &Path,
    scratch: &Path,
    edge: u32,
) -> Result<image::RgbaImage, ThumbnailError> {
    run_video_thumbnailer(source, scratch, edge)?;
    let frame = image::open(scratch);
    remove_temporary_file(scratch);
    frame
        .map(|frame| frame.to_rgba8())
        .map_err(|source| ThumbnailError::ReadVideoFrame {
            path: scratch.to_path_buf(),
            source,
        })
}

pub async fn load_image_dimensions(source: impl AsRef<Path>) -> Result<(u32, u32), ThumbnailError> {
    let source = source.as_ref().to_path_buf();
    let log_source = source.clone();
    let started_at = Instant::now();
    tracing::debug!(
        target: "thumbnails",
        source = ?log_source,
        "image dimensions requested"
    );
    let dimensions = tokio::task::spawn_blocking(move || load_image_dimensions_blocking(source))
        .await
        .map_err(|source| ThumbnailError::Join(source.to_string()))?;
    match &dimensions {
        Ok((width, height)) => tracing::debug!(
            target: "thumbnails",
            source = ?log_source,
            width,
            height,
            elapsed_ms = started_at.elapsed().as_millis(),
            "image dimensions loaded"
        ),
        Err(error) => tracing::debug!(
            target: "thumbnails",
            source = ?log_source,
            error = %error,
            elapsed_ms = started_at.elapsed().as_millis(),
            "image dimensions failed"
        ),
    }
    dimensions
}

fn load_image_dimensions_blocking(source: PathBuf) -> Result<(u32, u32), ThumbnailError> {
    if is_svg_path(&source) {
        return svg::load_svg_dimensions(&source);
    }

    let mut decoder = open_raster_decoder(&source)?;
    let dimensions = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    Ok(oriented_image_dimensions(dimensions, orientation))
}

fn open_raster_decoder(source: &Path) -> Result<impl ImageDecoder, ThumbnailError> {
    image::ImageReader::open(source)
        .map_err(|source_error| ThumbnailError::ReadImage {
            path: source.to_path_buf(),
            source: image::ImageError::IoError(source_error),
        })?
        .into_decoder()
        .map_err(|source_error| ThumbnailError::ReadImage {
            path: source.to_path_buf(),
            source: source_error,
        })
}

fn decode_raster_with_orientation(
    source: &Path,
) -> Result<(image::DynamicImage, Orientation), ThumbnailError> {
    let mut decoder = open_raster_decoder(source)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut limits = image::Limits::default();
    limits
        .reserve(decoder.total_bytes())
        .map_err(|source_error| ThumbnailError::ReadImage {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    decoder
        .set_limits(limits)
        .map_err(|source_error| ThumbnailError::ReadImage {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    let image = image::DynamicImage::from_decoder(decoder).map_err(|source_error| {
        ThumbnailError::ReadImage {
            path: source.to_path_buf(),
            source: source_error,
        }
    })?;
    Ok((image, orientation))
}

pub fn oriented_image_dimensions(
    (width, height): (u32, u32),
    orientation: Orientation,
) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (height, width)
    } else {
        (width, height)
    }
}

fn run_video_thumbnailer(
    source: &Path,
    output: &Path,
    max_edge: u32,
) -> Result<(), ThumbnailError> {
    match run_ffmpegthumbnailer(source, output, max_edge) {
        Ok(()) => Ok(()),
        Err(first_error) => match run_ffmpeg(source, output, max_edge) {
            Ok(()) => Ok(()),
            Err(ThumbnailError::VideoThumbnailerUnavailable { .. }) => Err(first_error),
            Err(second_error) => Err(second_error),
        },
    }
}

fn run_ffmpegthumbnailer(
    source: &Path,
    output: &Path,
    max_edge: u32,
) -> Result<(), ThumbnailError> {
    let command_output = Command::new("ffmpegthumbnailer")
        .arg("-i")
        .arg(source)
        .arg("-o")
        .arg(output)
        .arg("-s")
        .arg(max_edge.to_string())
        .arg("-t")
        .arg("10%")
        .output();
    handle_video_thumbnailer_output("ffmpegthumbnailer", source, command_output)
}

fn run_ffmpeg(source: &Path, output: &Path, max_edge: u32) -> Result<(), ThumbnailError> {
    let scale_filter = format!("scale={max_edge}:{max_edge}:force_original_aspect_ratio=decrease");
    let command_output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(VIDEO_THUMBNAIL_SEEK_TIME)
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(scale_filter)
        .arg("-f")
        .arg("image2")
        .arg("-vcodec")
        .arg("png")
        .arg(output)
        .output();
    handle_video_thumbnailer_output("ffmpeg", source, command_output)
}

fn handle_video_thumbnailer_output(
    command: &'static str,
    path: &Path,
    output: std::io::Result<Output>,
) -> Result<(), ThumbnailError> {
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(ThumbnailError::GenerateVideoThumbnail {
            path: path.to_path_buf(),
            command,
            message: video_thumbnailer_failure_message(&output),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(ThumbnailError::VideoThumbnailerUnavailable {
                path: path.to_path_buf(),
            })
        }
        Err(source) => Err(ThumbnailError::RunVideoThumbnailer {
            path: path.to_path_buf(),
            command,
            source,
        }),
    }
}

fn video_thumbnailer_failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return bounded_video_thumbnailer_detail(&stderr);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return bounded_video_thumbnailer_detail(&stdout);
    }
    format!("exit status {}", output.status)
}

fn bounded_video_thumbnailer_detail(detail: &str) -> String {
    let mut detail = detail.chars();
    let mut bounded = detail
        .by_ref()
        .take(VIDEO_THUMBNAILER_DETAIL_CHAR_LIMIT)
        .collect::<String>();
    if detail.next().is_some() {
        bounded.pop();
        bounded.push('…');
    }
    bounded
}

fn thumbnail_media_kind_for_path(path: &Path) -> Option<ThumbnailMediaKind> {
    let extension = path.extension().and_then(std::ffi::OsStr::to_str)?;
    if file_core::is_supported_image_extension(extension) {
        Some(ThumbnailMediaKind::Image)
    } else if file_core::is_supported_video_extension(extension) {
        Some(ThumbnailMediaKind::Video)
    } else {
        None
    }
}

fn is_svg_path(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// 与成品同目录，名字以 `.png` 结尾：崩溃留下的也会被定期清理按时间删掉。
fn temporary_output_path(output: &Path) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output
        .file_stem()
        .map(|stem| stem.to_string_lossy())
        .unwrap_or_else(|| "thumbnail".into());
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    parent.join(format!(".{stem}.{}.{}.tmp.png", std::process::id(), unique))
}

#[cfg(unix)]
fn request_path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn request_path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn update_hash(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    update_hash_inner(hasher, bytes);
}

#[cfg(not(unix))]
fn update_hash(hasher: &mut blake3::Hasher, bytes: impl AsRef<[u8]>) {
    update_hash_inner(hasher, bytes.as_ref());
}

fn update_hash_inner(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn update_system_time_hash(hasher: &mut blake3::Hasher, time: Option<SystemTime>) {
    match time {
        Some(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                update_hash(hasher, b"mtime+");
                update_hash(hasher, &duration.as_secs().to_le_bytes());
                update_hash(hasher, &duration.subsec_nanos().to_le_bytes());
            }
            Err(error) => {
                let duration = error.duration();
                update_hash(hasher, b"mtime-");
                update_hash(hasher, &duration.as_secs().to_le_bytes());
                update_hash(hasher, &duration.subsec_nanos().to_le_bytes());
            }
        },
        None => update_hash(hasher, b"mtime-none"),
    }
}
