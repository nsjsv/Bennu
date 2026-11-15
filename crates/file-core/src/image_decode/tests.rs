use std::path::Path;

use image::metadata::Orientation;

use super::camera_raw::test_files;
use super::*;

fn write(directory: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, bytes).expect("write test file");
    path
}

#[test]
fn bound_fits_inside_without_upscaling() {
    let bound = DecodeBound::within(800, 600);
    assert_eq!(bound.fit(4000, 3000), (800, 600));
    assert_eq!(bound.fit(3000, 4000), (450, 600));
    assert_eq!(bound.fit(400, 300), (400, 300));
    assert_eq!(DecodeBound::FULL.fit(9000, 7000), (9000, 7000));
}

#[test]
fn builtin_picture_is_bounded_and_keeps_the_source_size() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("wide.png");
    image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 128]))
        .save(&path)
        .expect("save png");

    assert_eq!(
        image_dimensions(&path, RasterSource::Picture).expect("dimensions"),
        (400, 200)
    );
    let decoded =
        decode_image(&path, RasterSource::Picture, DecodeBound::within(100, 100)).expect("decode");
    assert_eq!((decoded.width, decoded.height), (400, 200));
    assert_eq!(decoded.pixels.dimensions(), (100, 50));
    assert_eq!(decoded.pixels.get_pixel(0, 0).0[3], 128);
    assert!(decoded.has_alpha);

    let opaque = directory.path().join("opaque.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
        .save(&opaque)
        .expect("save png");
    let decoded = decode_image(&opaque, RasterSource::Picture, DecodeBound::FULL).expect("decode");
    assert!(!decoded.has_alpha);
}

#[test]
fn content_decides_the_builtin_format_not_the_name() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = write(
        directory.path(),
        "actually-jpeg.png",
        &test_files::jpeg(8, 4),
    );
    assert_eq!(
        image_dimensions(&path, RasterSource::Picture).expect("dimensions"),
        (8, 4)
    );
}

#[test]
fn tiff_raw_shows_the_largest_preview_turned_upright() {
    let directory = tempfile::tempdir().expect("temp dir");
    let thumbnail = test_files::jpeg(40, 30);
    let preview = test_files::jpeg(160, 120);
    // 6 = 顺时针转 90 度才正。
    let path = write(
        directory.path(),
        "shot.nef",
        &test_files::tiff_raw(6, &thumbnail, &preview),
    );

    assert_eq!(
        image_dimensions(&path, RasterSource::CameraRaw).expect("dimensions"),
        (120, 160)
    );
    let decoded = decode_image(&path, RasterSource::CameraRaw, DecodeBound::FULL).expect("decode");
    assert_eq!(decoded.pixels.dimensions(), (120, 160));
}

#[test]
fn raf_cr3_and_olympus_previews_are_found() {
    let directory = tempfile::tempdir().expect("temp dir");
    let preview = test_files::jpeg(64, 48);
    for (name, bytes) in [
        ("shot.raf", test_files::raf(&preview)),
        ("shot.cr3", test_files::cr3(&preview)),
        ("shot.orf", test_files::olympus_orf(&preview)),
    ] {
        let path = write(directory.path(), name, &bytes);
        assert_eq!(
            image_dimensions(&path, RasterSource::CameraRaw).expect(name),
            (64, 48),
            "{name}"
        );
    }
}

#[test]
fn raw_without_a_displayable_preview_says_so() {
    let directory = tempfile::tempdir().expect("temp dir");
    // 只有 TIFF 头和一个空目录。
    let path = write(
        directory.path(),
        "empty.dng",
        b"II*\0\x08\0\0\0\0\0\0\0\0\0",
    );
    assert!(matches!(
        image_dimensions(&path, RasterSource::CameraRaw),
        Err(ImageDecodeError::NoEmbeddedPreview)
    ));
}

#[test]
fn oriented_dimensions_swap_only_for_quarter_turns() {
    assert_eq!(oriented_dimensions((4, 2), Orientation::Rotate90), (2, 4));
    assert_eq!(oriented_dimensions((4, 2), Orientation::Rotate180), (4, 2));
}

/// 需要本机装了 ffmpeg：内置解码器不认的格式交给它，按要求缩小，尺寸取原图的。
#[test]
#[ignore = "needs ffmpeg on PATH"]
fn ffmpeg_decodes_formats_the_builtin_decoders_lack() {
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("source.png");
    image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 100, 50, 255]))
        .save(&source)
        .expect("save png");
    let target = directory.path().join("picture.tga");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&source)
        .arg(&target)
        .status()
        .expect("run ffmpeg");
    assert!(status.success());

    assert_eq!(
        image_dimensions(&target, RasterSource::Picture).expect("dimensions"),
        (300, 200)
    );
    let decoded = decode_image(
        &target,
        RasterSource::Picture,
        DecodeBound::within(150, 150),
    )
    .expect("decode");
    assert_eq!((decoded.width, decoded.height), (300, 200));
    assert_eq!(decoded.pixels.dimensions(), (150, 100));
}
