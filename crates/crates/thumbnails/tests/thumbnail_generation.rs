use image::ImageEncoder;
use std::path::Path;

use thumbnails::{
    generate_image_thumbnail, is_supported_thumbnail_path, load_cached_thumbnail,
    load_image_dimensions, load_or_generate_image_thumbnail, load_or_generate_thumbnail,
    path_is_in_thumbnail_cache, ThumbnailError, ThumbnailOptions, ThumbnailRequest,
    ThumbnailSourceMetadata,
};

const AVIF_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/primary-colors.avif"
);

#[tokio::test]
async fn generate_image_thumbnail_writes_small_image() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let output = dir.path().join("cache/thumb.png");
    let image = image::RgbImage::new(64, 32);
    image.save(&source).unwrap();

    let thumbnail = generate_image_thumbnail(&source, &output, ThumbnailOptions { max_edge: 16 })
        .await
        .unwrap();

    assert_eq!(thumbnail.source, source);
    assert_eq!(thumbnail.output, output);
    assert!(thumbnail.width <= 16);
    assert!(thumbnail.height <= 16);
    assert!(thumbnail.output.exists());
}

#[tokio::test]
async fn load_or_generate_image_thumbnail_reuses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let cache_dir = dir.path().join("cache");
    let image = image::RgbImage::new(64, 32);
    image.save(&source).unwrap();
    let request = thumbnail_request(&source, 16);

    let first = load_or_generate_image_thumbnail(&cache_dir, request.clone())
        .await
        .unwrap();
    let second = load_or_generate_image_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(first.key, second.key);
    assert_eq!(first.output, second.output);
}

#[tokio::test]
async fn load_cached_thumbnail_reads_existing_cache_only() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let cache_dir = dir.path().join("cache");
    let image = image::RgbImage::new(64, 32);
    image.save(&source).unwrap();
    let request = thumbnail_request(&source, 16);
    let generated = load_or_generate_image_thumbnail(&cache_dir, request.clone())
        .await
        .unwrap();

    let cached = load_cached_thumbnail(&cache_dir, request)
        .await
        .unwrap()
        .expect("cached thumbnail");

    assert!(cached.cache_hit);
    assert_eq!(cached.key, generated.key);
    assert_eq!(cached.output, generated.output);
    assert_eq!(
        (cached.width, cached.height),
        (generated.width, generated.height)
    );
}

#[tokio::test]
async fn load_cached_thumbnail_miss_does_not_create_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let cache_dir = dir.path().join("cache");
    let image = image::RgbImage::new(64, 32);
    image.save(&source).unwrap();
    let request = thumbnail_request(&source, 16);

    let cached = load_cached_thumbnail(&cache_dir, request).await.unwrap();

    assert!(cached.is_none());
    assert!(!cache_dir.exists());
}

#[tokio::test]
async fn load_or_generate_thumbnail_rejects_cache_directory_source() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let source = cache_dir.join("generated.png");
    let image = image::RgbImage::new(64, 32);
    image.save(&source).unwrap();
    let request = thumbnail_request(&source, 16);

    let error = load_or_generate_thumbnail(&cache_dir, request)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ThumbnailError::SourceInsideCacheDirectory { path, cache_dir: rejected_cache_dir }
            if path == source && rejected_cache_dir == cache_dir
    ));
    assert_eq!(std::fs::read_dir(&cache_dir).unwrap().count(), 1);
}

#[tokio::test]
async fn load_or_generate_image_thumbnail_reads_bmp_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bmp");
    let cache_dir = dir.path().join("cache");
    let image = image::RgbImage::new(48, 24);
    image
        .save_with_format(&source, image::ImageFormat::Bmp)
        .unwrap();
    let request = thumbnail_request(&source, 12);

    let thumbnail = load_or_generate_image_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert_eq!(thumbnail.source, source);
    assert!(thumbnail.width <= 12);
    assert!(thumbnail.height <= 12);
    assert!(thumbnail.output.exists());
}

#[tokio::test]
async fn avif_dimensions_thumbnail_and_cache_use_bitmap_decoder() {
    let source = Path::new(AVIF_FIXTURE);
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let request = thumbnail_request(source, 8);

    assert_eq!(load_image_dimensions(source).await.unwrap(), (16, 8));

    let first = load_or_generate_image_thumbnail(&cache_dir, request.clone())
        .await
        .unwrap();
    let second = load_or_generate_image_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert_eq!((first.width, first.height), (8, 4));
    assert_eq!(image::image_dimensions(&first.output).unwrap(), (8, 4));
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(first.key, second.key);
    assert_eq!(first.output, second.output);
}

#[tokio::test]
async fn load_or_generate_image_thumbnail_renders_svg_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.svg");
    let cache_dir = dir.path().join("cache");
    write_svg_image(&source);
    let request = thumbnail_request(&source, 20);

    let thumbnail = load_or_generate_image_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert_eq!(thumbnail.source, source);
    assert_eq!((thumbnail.width, thumbnail.height), (20, 10));
    assert!(thumbnail.output.exists());
    assert_eq!(
        image::image_dimensions(&thumbnail.output).unwrap(),
        (20, 10)
    );
}

#[tokio::test]
async fn load_or_generate_image_thumbnail_upscales_small_svg_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("small.svg");
    let cache_dir = dir.path().join("cache");
    write_sized_svg_image(&source, 24, 24);
    let request = thumbnail_request(&source, 128);

    let thumbnail = load_or_generate_image_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert_eq!(thumbnail.source, source);
    assert_eq!((thumbnail.width, thumbnail.height), (128, 128));
    assert_eq!(
        image::image_dimensions(&thumbnail.output).unwrap(),
        (128, 128)
    );
}

#[tokio::test]
async fn load_image_dimensions_reads_source_size() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let image = image::RgbImage::new(37, 19);
    image.save(&source).unwrap();

    let dimensions = load_image_dimensions(&source).await.unwrap();

    assert_eq!(dimensions, (37, 19));
}

#[tokio::test]
async fn dimensions_and_thumbnail_honor_exif_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("rotated.jpg");
    let output = dir.path().join("rotated-thumbnail.png");
    write_exif_rotated_jpeg(&source, 4, 2);

    let dimensions = load_image_dimensions(&source).await.unwrap();
    let thumbnail = generate_image_thumbnail(&source, &output, ThumbnailOptions { max_edge: 64 })
        .await
        .unwrap();

    assert_eq!(dimensions, (2, 4));
    assert!(thumbnail.height > thumbnail.width);
    assert_eq!(
        image::image_dimensions(&thumbnail.output).unwrap(),
        (thumbnail.width, thumbnail.height)
    );
}

#[tokio::test]
async fn load_image_dimensions_reads_svg_size() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.svg");
    write_svg_image(&source);

    let dimensions = load_image_dimensions(&source).await.unwrap();

    assert_eq!(dimensions, (80, 40));
}

#[test]
fn supported_thumbnail_path_includes_images_and_videos() {
    for path in [
        "source.avif",
        "source.bmp",
        "source.gif",
        "source.ico",
        "source.jpg",
        "source.jpeg",
        "source.png",
        "source.svg",
        "source.tif",
        "source.tiff",
        "source.webp",
    ] {
        assert!(is_supported_thumbnail_path(path), "{path}");
    }
    assert!(is_supported_thumbnail_path("clip.MP4"));
    assert!(is_supported_thumbnail_path("movie.webm"));
    assert!(!is_supported_thumbnail_path("notes.txt"));
    assert!(!is_supported_thumbnail_path("photo.heic"));
    assert!(!is_supported_thumbnail_path("photo.heif"));
}

#[test]
fn thumbnail_cache_path_matches_cache_dir_descendants_only() {
    let cache_dir = Path::new("/tmp/bennu/thumbnails");

    assert!(path_is_in_thumbnail_cache(
        cache_dir,
        "/tmp/bennu/thumbnails"
    ));
    assert!(path_is_in_thumbnail_cache(
        cache_dir,
        "/tmp/bennu/thumbnails/generated.png"
    ));
    assert!(!path_is_in_thumbnail_cache(
        cache_dir,
        "/tmp/bennu/thumbnails-extra/generated.png"
    ));
    assert!(!path_is_in_thumbnail_cache("", "generated.png"));
}

fn thumbnail_request(source: &Path, max_edge: u32) -> ThumbnailRequest {
    let metadata = std::fs::metadata(source).unwrap();
    ThumbnailRequest::new(
        source,
        ThumbnailSourceMetadata {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        },
        max_edge,
    )
}

fn write_svg_image(path: &Path) {
    write_sized_svg_image(path, 80, 40);
}

fn write_sized_svg_image(path: &Path, width: u32, height: u32) {
    std::fs::write(
        path,
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="{width}" height="{height}" fill="#3366cc"/></svg>"##
        ),
    )
    .unwrap();
}

fn write_exif_rotated_jpeg(path: &Path, width: u32, height: u32) {
    let pixels = vec![127; width as usize * height as usize * 3];
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(std::fs::File::create(path).unwrap());
    encoder
        .set_exif_metadata(vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0,
            0,
        ])
        .unwrap();
    encoder
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
        .unwrap();
}
