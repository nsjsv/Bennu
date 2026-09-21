#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use image::{metadata::Orientation, ImageDecoder};
use thiserror::Error;
use tokio::fs;

mod svg;

const CACHE_FORMAT_EXTENSION: &str = "png";
const THUMBNAILER_VERSION: u8 = 4;
const VIDEO_THUMBNAIL_SEEK_TIME: &str = "00:00:01";
const VIDEO_THUMBNAILER_DETAIL_CHAR_LIMIT: usize = 1_000;
const THUMBNAIL_NORMAL_DIR: &str = "normal";
const THUMBNAIL_LARGE_DIR: &str = "large";
const THUMBNAIL_X_LARGE_DIR: &str = "x-large";
const THUMBNAIL_XX_LARGE_DIR: &str = "xx-large";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbnailMediaKind {
    Image,
    Video,
}

impl ThumbnailMediaKind {
    fn cache_tag(self) -> &'static [u8] {
        match self {
            Self::Image => b"image",
            Self::Video => b"video",
        }
    }
}

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
        let media_kind =
            thumbnail_media_kind_for_path(&self.source).unwrap_or(ThumbnailMediaKind::Image);
        thumbnail_key(self, media_kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailOptions {
    pub max_edge: u32,
}

impl Default for ThumbnailOptions {
    fn default() -> Self {
        Self { max_edge: 256 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub source: PathBuf,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
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
    #[error("could not create thumbnail cache directory {path:?}: {source}")]
    CreateCacheDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("thumbnail task failed: {0}")]
    Join(String),
    #[error("could not read cached thumbnail {path:?}: {source}")]
    ReadCachedThumbnail {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
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
    #[error("could not write SVG thumbnail {path:?}: {source}")]
    WriteSvgThumbnail {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write thumbnail {path:?}: {source}")]
    WriteImage {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("could not move thumbnail {from:?} to {to:?}: {source}")]
    RenameThumbnail {
        from: PathBuf,
        to: PathBuf,
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

pub fn path_is_in_thumbnail_cache(cache_dir: impl AsRef<Path>, path: impl AsRef<Path>) -> bool {
    let cache_dir = cache_dir.as_ref();
    !cache_dir.as_os_str().is_empty() && path.as_ref().starts_with(cache_dir)
}

pub async fn load_or_generate_thumbnail(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
) -> Result<CachedThumbnail, ThumbnailError> {
    let media_kind = thumbnail_media_kind_for_path(&request.source).ok_or_else(|| {
        ThumbnailError::UnsupportedFormat {
            path: request.source.clone(),
        }
    })?;
    load_or_generate_thumbnail_with_kind(cache_dir, request, media_kind).await
}

pub async fn load_cached_thumbnail(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
) -> Result<Option<CachedThumbnail>, ThumbnailError> {
    let media_kind = thumbnail_media_kind_for_path(&request.source).ok_or_else(|| {
        ThumbnailError::UnsupportedFormat {
            path: request.source.clone(),
        }
    })?;
    let cache_dir = cache_dir.as_ref();
    if path_is_in_thumbnail_cache(cache_dir, &request.source) {
        return Err(ThumbnailError::SourceInsideCacheDirectory {
            path: request.source,
            cache_dir: cache_dir.to_path_buf(),
        });
    }
    let key = thumbnail_key(&request, media_kind);
    let output = cached_thumbnail_output_path(cache_dir, &key, request.max_edge);
    match cached_thumbnail_dimensions(output.clone()).await {
        Ok((width, height)) => Ok(Some(CachedThumbnail {
            key,
            source: request.source,
            output,
            width,
            height,
            cache_hit: true,
        })),
        Err(_) => Ok(None),
    }
}

pub async fn load_or_generate_image_thumbnail(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
) -> Result<CachedThumbnail, ThumbnailError> {
    if !file_core::is_supported_image_path(&request.source) {
        return Err(ThumbnailError::UnsupportedFormat {
            path: request.source,
        });
    }
    load_or_generate_thumbnail_with_kind(cache_dir, request, ThumbnailMediaKind::Image).await
}

async fn load_or_generate_thumbnail_with_kind(
    cache_dir: impl AsRef<Path>,
    request: ThumbnailRequest,
    media_kind: ThumbnailMediaKind,
) -> Result<CachedThumbnail, ThumbnailError> {
    let started_at = Instant::now();
    let cache_dir = cache_dir.as_ref();
    if path_is_in_thumbnail_cache(cache_dir, &request.source) {
        return Err(ThumbnailError::SourceInsideCacheDirectory {
            path: request.source,
            cache_dir: cache_dir.to_path_buf(),
        });
    }
    let key = thumbnail_key(&request, media_kind);
    let output = cached_thumbnail_output_path(cache_dir, &key, request.max_edge);
    tracing::debug!(
        target: "thumbnails",
        source = ?request.source,
        output = ?output,
        key = key.as_str(),
        media_kind = ?media_kind,
        max_edge = request.max_edge,
        "thumbnail requested"
    );

    match cached_thumbnail_dimensions(output.clone()).await {
        Ok((width, height)) => {
            tracing::debug!(
                target: "thumbnails",
                source = ?request.source,
                output = ?output,
                key = key.as_str(),
                max_edge = request.max_edge,
                width,
                height,
                elapsed_ms = started_at.elapsed().as_millis(),
                "thumbnail cache hit"
            );
            return Ok(CachedThumbnail {
                key,
                source: request.source,
                output,
                width,
                height,
                cache_hit: true,
            });
        }
        Err(_) => {
            let _ = fs::remove_file(&output).await;
        }
    }

    generate_cached_thumbnail(request, output, key, media_kind).await
}

fn cached_thumbnail_output_path(cache_dir: &Path, key: &ThumbnailKey, max_edge: u32) -> PathBuf {
    cache_dir.join(thumbnail_bucket_dir(max_edge)).join(format!(
        "{}.{}",
        key.as_str(),
        CACHE_FORMAT_EXTENSION
    ))
}

fn thumbnail_bucket_dir(max_edge: u32) -> &'static str {
    match max_edge {
        0..=128 => THUMBNAIL_NORMAL_DIR,
        129..=256 => THUMBNAIL_LARGE_DIR,
        257..=512 => THUMBNAIL_X_LARGE_DIR,
        _ => THUMBNAIL_XX_LARGE_DIR,
    }
}

pub async fn generate_image_thumbnail(
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: ThumbnailOptions,
) -> Result<Thumbnail, ThumbnailError> {
    let source = source.as_ref().to_path_buf();
    let output = output.as_ref().to_path_buf();

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).await.map_err(|source| {
            ThumbnailError::CreateCacheDirectory {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }

    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        output = ?output,
        max_edge = options.max_edge,
        "generate image thumbnail requested"
    );

    tokio::task::spawn_blocking(move || generate_image_thumbnail_blocking(source, output, options))
        .await
        .map_err(|source| ThumbnailError::Join(source.to_string()))?
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

async fn cached_thumbnail_dimensions(path: PathBuf) -> Result<(u32, u32), ThumbnailError> {
    tokio::task::spawn_blocking(move || {
        image::image_dimensions(&path)
            .map_err(|source| ThumbnailError::ReadCachedThumbnail { path, source })
    })
    .await
    .map_err(|source| ThumbnailError::Join(source.to_string()))?
}

async fn generate_cached_thumbnail(
    request: ThumbnailRequest,
    output: PathBuf,
    key: ThumbnailKey,
    media_kind: ThumbnailMediaKind,
) -> Result<CachedThumbnail, ThumbnailError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).await.map_err(|source| {
            ThumbnailError::CreateCacheDirectory {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }

    let temporary_output = temporary_output_path(&output);
    // 图片解码和外部视频缩略图命令都会阻塞，必须留在 blocking 线程池。
    tokio::task::spawn_blocking(move || match media_kind {
        ThumbnailMediaKind::Image => {
            generate_cached_image_thumbnail_blocking(request, temporary_output, output, key)
        }
        ThumbnailMediaKind::Video => {
            generate_cached_video_thumbnail_blocking(request, temporary_output, output, key)
        }
    })
    .await
    .map_err(|source| ThumbnailError::Join(source.to_string()))?
}

fn generate_cached_image_thumbnail_blocking(
    request: ThumbnailRequest,
    temporary_output: PathBuf,
    output: PathBuf,
    key: ThumbnailKey,
) -> Result<CachedThumbnail, ThumbnailError> {
    let source = request.source;
    if is_svg_path(&source) {
        let started_at = Instant::now();
        tracing::debug!(
            target: "thumbnails",
            source = ?source,
            output = ?temporary_output,
            max_edge = request.max_edge,
            "SVG thumbnail render started"
        );
        let (width, height) =
            svg::render_svg_thumbnail(&source, &temporary_output, request.max_edge)?;
        tracing::debug!(
            target: "thumbnails",
            source = ?source,
            output = ?temporary_output,
            width,
            height,
            elapsed_ms = started_at.elapsed().as_millis(),
            "SVG thumbnail render finished"
        );
        return finish_cached_thumbnail(key, source, temporary_output, output, width, height);
    }

    let (image, orientation) = decode_raster_with_orientation(&source)?;
    let mut thumbnail = image.thumbnail(request.max_edge, request.max_edge);
    thumbnail.apply_orientation(orientation);
    let width = thumbnail.width();
    let height = thumbnail.height();

    thumbnail
        .save_with_format(&temporary_output, image::ImageFormat::Png)
        .map_err(|source_error| ThumbnailError::WriteImage {
            path: temporary_output.clone(),
            source: source_error,
        })?;

    finish_cached_thumbnail(key, source, temporary_output, output, width, height)
}

fn generate_cached_video_thumbnail_blocking(
    request: ThumbnailRequest,
    temporary_output: PathBuf,
    output: PathBuf,
    key: ThumbnailKey,
) -> Result<CachedThumbnail, ThumbnailError> {
    let source = request.source;
    run_video_thumbnailer(&source, &temporary_output, request.max_edge)?;
    let (width, height) = image::image_dimensions(&temporary_output).map_err(|source_error| {
        ThumbnailError::ReadCachedThumbnail {
            path: temporary_output.clone(),
            source: source_error,
        }
    })?;

    finish_cached_thumbnail(key, source, temporary_output, output, width, height)
}

fn finish_cached_thumbnail(
    key: ThumbnailKey,
    source: PathBuf,
    temporary_output: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
) -> Result<CachedThumbnail, ThumbnailError> {
    std::fs::rename(&temporary_output, &output).map_err(|source_error| {
        ThumbnailError::RenameThumbnail {
            from: temporary_output.clone(),
            to: output.clone(),
            source: source_error,
        }
    })?;

    Ok(CachedThumbnail {
        key,
        source,
        output,
        width,
        height,
        cache_hit: false,
    })
}

fn generate_image_thumbnail_blocking(
    source: PathBuf,
    output: PathBuf,
    options: ThumbnailOptions,
) -> Result<Thumbnail, ThumbnailError> {
    if is_svg_path(&source) {
        let started_at = Instant::now();
        tracing::debug!(
            target: "thumbnails",
            source = ?source,
            output = ?output,
            max_edge = options.max_edge,
            "SVG direct thumbnail render started"
        );
        let (width, height) = svg::render_svg_thumbnail(&source, &output, options.max_edge)?;
        tracing::debug!(
            target: "thumbnails",
            source = ?source,
            output = ?output,
            width,
            height,
            elapsed_ms = started_at.elapsed().as_millis(),
            "SVG direct thumbnail render finished"
        );
        return Ok(Thumbnail {
            source,
            output,
            width,
            height,
        });
    }

    let (image, orientation) = decode_raster_with_orientation(&source)?;
    let mut thumbnail = image.thumbnail(options.max_edge, options.max_edge);
    thumbnail.apply_orientation(orientation);
    let width = thumbnail.width();
    let height = thumbnail.height();

    thumbnail
        .save(&output)
        .map_err(|source_error| ThumbnailError::WriteImage {
            path: output.clone(),
            source: source_error,
        })?;

    Ok(Thumbnail {
        source,
        output,
        width,
        height,
    })
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
    parent.join(format!(
        ".{stem}.{}.{}.tmp.{CACHE_FORMAT_EXTENSION}",
        std::process::id(),
        unique
    ))
}

fn thumbnail_key(request: &ThumbnailRequest, media_kind: ThumbnailMediaKind) -> ThumbnailKey {
    let mut hasher = blake3::Hasher::new();
    update_hash(&mut hasher, b"file-manager-thumbnail");
    update_hash(&mut hasher, &[THUMBNAILER_VERSION]);
    update_hash(&mut hasher, media_kind.cache_tag());
    update_hash(&mut hasher, request_path_bytes(&request.source));
    update_hash(&mut hasher, &request.metadata.len.to_le_bytes());
    update_system_time_hash(&mut hasher, request.metadata.modified);
    update_hash(&mut hasher, &request.max_edge.to_le_bytes());
    update_hash(&mut hasher, CACHE_FORMAT_EXTENSION.as_bytes());
    ThumbnailKey(hasher.finalize().to_hex().to_string())
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
