use image::ImageEncoder;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use thumbnails::{
    file_uri, is_supported_thumbnail_path, load_cached_thumbnail, load_image_dimensions,
    load_or_generate_thumbnail, path_is_in_thumbnail_cache, thumbnail_cache_dir, ThumbnailError,
    ThumbnailRequest, ThumbnailSourceMetadata,
};

const AVIF_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/primary-colors.avif"
);

#[tokio::test]
async fn thumbnail_is_written_where_and_how_the_freedesktop_spec_says() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("My Photos (1).png");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(300, 150).save(&source).unwrap();
    let request = thumbnail_request(&source, 96);

    let first = load_or_generate_thumbnail(&cache_dir, request.clone())
        .await
        .unwrap();
    let second = load_or_generate_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    let uri = file_uri(&source);
    assert_eq!(first.output, cache_dir.join("normal").join(md5_name(&uri)));
    assert_eq!((first.width, first.height), (128, 64));
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(first.output, second.output);
    let texts = png_texts(&first.output);
    let source_metadata = std::fs::metadata(&source).unwrap();
    assert_eq!(text(&texts, "Thumb::URI"), Some(uri.as_str()));
    assert_eq!(
        text(&texts, "Thumb::MTime"),
        Some(unix_seconds(&source_metadata).to_string().as_str())
    );
    assert_eq!(
        text(&texts, "Thumb::Size"),
        Some(source_metadata.len().to_string().as_str())
    );
    assert_eq!(mode(&first.output), 0o600);
    assert_eq!(mode(&cache_dir.join("normal")), 0o700);
}

#[tokio::test]
async fn requests_land_in_the_four_spec_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("wide.png");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(2000, 1000).save(&source).unwrap();

    for (max_edge, bucket, width) in [
        (48, "normal", 128),
        (200, "large", 256),
        (300, "x-large", 512),
        (2048, "xx-large", 1024),
    ] {
        let thumbnail =
            load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, max_edge))
                .await
                .unwrap();
        assert!(
            thumbnail.output.starts_with(cache_dir.join(bucket)),
            "{max_edge}"
        );
        assert_eq!(thumbnail.width, width, "{max_edge}");
    }
}

#[tokio::test]
async fn changed_source_is_regenerated_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photo.png");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(64, 32).save(&source).unwrap();
    let first = load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, 64))
        .await
        .unwrap();

    image::RgbImage::new(40, 40).save(&source).unwrap();
    let file = std::fs::File::options().write(true).open(&source).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(2_000_000_000))
        .unwrap();
    let request = thumbnail_request(&source, 64);
    assert_eq!(
        load_cached_thumbnail(&cache_dir, request.clone())
            .await
            .unwrap(),
        None
    );
    let second = load_or_generate_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert!(!second.cache_hit);
    assert_eq!(second.output, first.output);
    assert_ne!(second.key, first.key);
    assert_eq!((second.width, second.height), (40, 40));
    assert_eq!(
        text(&png_texts(&second.output), "Thumb::MTime"),
        Some("2000000000")
    );
}

#[tokio::test]
async fn thumbnail_written_by_another_program_is_used_as_is() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photo.png");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(640, 480).save(&source).unwrap();
    let uri = file_uri(&source);
    let foreign = cache_dir.join("large").join(md5_name(&uri));
    std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    let mtime = unix_seconds(&std::fs::metadata(&source).unwrap());
    write_png_with_texts(
        &foreign,
        (7, 5),
        &[
            ("Thumb::URI", uri.as_str()),
            ("Thumb::MTime", &format!("{mtime}.123456")),
            ("Software", "GNOME::ThumbnailFactory"),
        ],
    );

    let thumbnail = load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, 200))
        .await
        .unwrap();

    assert!(thumbnail.cache_hit);
    assert_eq!(thumbnail.output, foreign);
    assert_eq!((thumbnail.width, thumbnail.height), (7, 5));
}

#[test]
fn file_uri_escapes_like_glib() {
    assert_eq!(
        file_uri(Path::new("/home/u/My Photos (1)/#1 100%.jpg")),
        "file:///home/u/My%20Photos%20(1)/%231%20100%25.jpg"
    );
    assert_eq!(
        file_uri(Path::new("/srv/a!$&'*+,;=:@~_-.b")),
        "file:///srv/a!$&'*+,%3B=:@~_-.b"
    );
    assert_eq!(
        file_uri(Path::new("/图片/a?b[c].png")),
        "file:///%E5%9B%BE%E7%89%87/a%3Fb%5Bc%5D.png"
    );
    let non_utf8 = PathBuf::from(OsString::from_vec(b"/tmp/\xff.png".to_vec()));
    assert_eq!(file_uri(&non_utf8), "file:///tmp/%FF.png");
}

#[tokio::test]
async fn load_cached_thumbnail_miss_does_not_create_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(64, 32).save(&source).unwrap();

    let cached = load_cached_thumbnail(&cache_dir, thumbnail_request(&source, 16))
        .await
        .unwrap();

    assert!(cached.is_none());
    assert!(!cache_dir.exists());
}

#[tokio::test]
async fn load_or_generate_thumbnail_rejects_cache_directory_source() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let source = cache_dir.join("generated.png");
    image::RgbImage::new(64, 32).save(&source).unwrap();
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
async fn small_bitmaps_are_not_upscaled() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bmp");
    let cache_dir = dir.path().join("cache");
    image::RgbImage::new(48, 24)
        .save_with_format(&source, image::ImageFormat::Bmp)
        .unwrap();

    let thumbnail = load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, 12))
        .await
        .unwrap();

    assert_eq!((thumbnail.width, thumbnail.height), (48, 24));
    assert_eq!(
        image::image_dimensions(&thumbnail.output).unwrap(),
        (48, 24)
    );
}

#[tokio::test]
async fn avif_dimensions_thumbnail_and_cache_use_bitmap_decoder() {
    let source = Path::new(AVIF_FIXTURE);
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let request = thumbnail_request(source, 8);

    assert_eq!(load_image_dimensions(source).await.unwrap(), (16, 8));

    let first = load_or_generate_thumbnail(&cache_dir, request.clone())
        .await
        .unwrap();
    let second = load_or_generate_thumbnail(&cache_dir, request)
        .await
        .unwrap();

    assert_eq!((first.width, first.height), (16, 8));
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
}

#[tokio::test]
async fn svg_is_rendered_at_the_bucket_size_with_spec_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.svg");
    let cache_dir = dir.path().join("cache");
    write_sized_svg_image(&source, 80, 40);

    let thumbnail = load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, 20))
        .await
        .unwrap();

    assert_eq!((thumbnail.width, thumbnail.height), (128, 64));
    assert_eq!(
        image::image_dimensions(&thumbnail.output).unwrap(),
        (128, 64)
    );
    assert_eq!(
        text(&png_texts(&thumbnail.output), "Thumb::URI"),
        Some(file_uri(&source).as_str())
    );
    let pixel = image::open(&thumbnail.output).unwrap().to_rgba8()[(0, 0)];
    assert_eq!(pixel.0, [0x33, 0x66, 0xcc, 0xff]);
}

#[tokio::test]
async fn load_image_dimensions_reads_source_size() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    image::RgbImage::new(37, 19).save(&source).unwrap();

    assert_eq!(load_image_dimensions(&source).await.unwrap(), (37, 19));
}

#[tokio::test]
async fn dimensions_and_thumbnail_honor_exif_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("rotated.jpg");
    let cache_dir = dir.path().join("cache");
    write_exif_rotated_jpeg(&source, 4, 2);

    let dimensions = load_image_dimensions(&source).await.unwrap();
    let thumbnail = load_or_generate_thumbnail(&cache_dir, thumbnail_request(&source, 64))
        .await
        .unwrap();

    assert_eq!(dimensions, (2, 4));
    assert_eq!((thumbnail.width, thumbnail.height), (2, 4));
    assert_eq!(image::image_dimensions(&thumbnail.output).unwrap(), (2, 4));
}

#[tokio::test]
async fn load_image_dimensions_reads_svg_size() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.svg");
    write_sized_svg_image(&source, 80, 40);

    assert_eq!(load_image_dimensions(&source).await.unwrap(), (80, 40));
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
}

#[test]
fn cache_location_follows_the_freedesktop_rules() {
    std::env::set_var("XDG_CACHE_HOME", "/xdg/cache");
    std::env::set_var("HOME", "/home/user");
    assert_eq!(
        thumbnail_cache_dir(),
        Some(PathBuf::from("/xdg/cache/thumbnails"))
    );

    std::env::set_var("XDG_CACHE_HOME", "relative/cache");
    assert_eq!(
        thumbnail_cache_dir(),
        Some(PathBuf::from("/home/user/.cache/thumbnails"))
    );

    std::env::remove_var("XDG_CACHE_HOME");
    std::env::remove_var("HOME");
    assert_eq!(thumbnail_cache_dir(), None);
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

fn md5_name(uri: &str) -> String {
    use md5::{Digest, Md5};
    let digest = Md5::digest(uri.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        + ".png"
}

fn unix_seconds(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn png_texts(path: &Path) -> Vec<(String, String)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let reader = png::Decoder::new(file).read_info().unwrap();
    reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .map(|chunk| (chunk.keyword.clone(), chunk.text.clone()))
        .collect()
}

fn text<'a>(texts: &'a [(String, String)], key: &str) -> Option<&'a str> {
    texts
        .iter()
        .find(|(keyword, _)| keyword == key)
        .map(|(_, text)| text.as_str())
}

fn write_png_with_texts(path: &Path, (width, height): (u32, u32), texts: &[(&str, &str)]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    for (keyword, text) in texts {
        encoder
            .add_ztxt_chunk((*keyword).to_owned(), (*text).to_owned())
            .unwrap();
    }
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![0; (width * height * 4) as usize])
        .unwrap();
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
