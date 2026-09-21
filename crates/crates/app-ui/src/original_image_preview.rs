use std::io::Cursor;
use std::path::{Path, PathBuf};

use iced::widget::{image as iced_image, svg};
use image::ImageDecoder;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

static ORIGINAL_IMAGE_DECODE_SEMAPHORE: Semaphore = Semaphore::const_new(1);
const ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE: u32 = 512;

#[derive(Debug, Clone)]
pub(crate) enum OriginalImagePreview {
    Raster {
        raster_handle: iced_image::Handle,
        placeholder_handle: iced_image::Handle,
        width: u32,
        height: u32,
    },
    Svg {
        handle: svg::Handle,
        width: u32,
        height: u32,
        has_intrinsic_size: bool,
    },
}

pub(crate) async fn load_original_image_preview(
    path: PathBuf,
    max_file_bytes: u64,
    cancellation: CancellationToken,
    placeholder_handle: Option<iced_image::Handle>,
) -> Result<OriginalImagePreview, String> {
    let permit = tokio::select! {
        _ = cancellation.cancelled() => return Err(original_image_preview_cancelled(&path)),
        permit = ORIGINAL_IMAGE_DECODE_SEMAPHORE.acquire() => permit
            .map_err(|_| "original image decoder is unavailable".to_owned())?,
    };
    let error_path = path.clone();
    let decode_cancellation = cancellation.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        load_original_image_preview_blocking(
            &path,
            max_file_bytes,
            &decode_cancellation,
            placeholder_handle,
        )
    })
    .await
    .map_err(|error| {
        format!("could not join original image preview task for {error_path:?}: {error}")
    })?;
    drop(permit);
    outcome
}

fn load_original_image_preview_blocking(
    path: &Path,
    max_file_bytes: u64,
    cancellation: &CancellationToken,
    placeholder_handle: Option<iced_image::Handle>,
) -> Result<OriginalImagePreview, String> {
    if cancellation.is_cancelled() {
        return Err(original_image_preview_cancelled(path));
    }
    ensure_preview_file_size(path, max_file_bytes)?;
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read original image {path:?}: {error}"))?;
    let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if max_file_bytes != 0 && byte_len > max_file_bytes {
        return Err(preview_file_size_error(path, byte_len, max_file_bytes));
    }
    if cancellation.is_cancelled() {
        return Err(original_image_preview_cancelled(path));
    }

    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        let has_intrinsic_size = svg_has_intrinsic_pixel_size(&bytes);
        let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
            .map_err(|error| format!("could not parse original SVG {path:?}: {error}"))?;
        if cancellation.is_cancelled() {
            return Err(original_image_preview_cancelled(path));
        }
        let size = tree.size();
        return Ok(OriginalImagePreview::Svg {
            handle: svg::Handle::from_memory(bytes),
            width: size.width().round().max(1.0) as u32,
            height: size.height().round().max(1.0) as u32,
            has_intrinsic_size,
        });
    }

    let mut reader = ::image::ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format()
        .map_err(|error| format!("could not identify original image {path:?}: {error}"))?;
    reader.limits(::image::Limits::default());
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("could not inspect original image {path:?}: {error}"))?;
    let source_dimensions = decoder.dimensions();
    let orientation = decoder
        .orientation()
        .unwrap_or(::image::metadata::Orientation::NoTransforms);
    let (expected_width, expected_height) =
        thumbnails::oriented_image_dimensions(source_dimensions, orientation);
    validate_raster_decode_budget(path, expected_width, expected_height)?;
    let limits = decoder_limits_for_total_bytes(path, decoder.total_bytes())?;
    decoder
        .set_limits(limits)
        .map_err(|error| format!("could not apply original image limits {path:?}: {error}"))?;
    let mut decoded = ::image::DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("could not decode original image {path:?}: {error}"))?;
    decoded.apply_orientation(orientation);
    if cancellation.is_cancelled() {
        return Err(original_image_preview_cancelled(path));
    }

    let width = decoded.width();
    let height = decoded.height();
    validate_raster_decode_budget(path, width, height)?;
    let placeholder_handle = if let Some(placeholder_handle) = placeholder_handle {
        placeholder_handle
    } else {
        let placeholder = decoded.thumbnail(
            ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE,
            ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE,
        );
        let placeholder = placeholder.into_rgba8();
        iced_image::Handle::from_rgba(
            placeholder.width(),
            placeholder.height(),
            placeholder.into_raw(),
        )
    };
    let raster = decoded.into_rgba8();

    Ok(OriginalImagePreview::Raster {
        raster_handle: iced_image::Handle::from_rgba(
            raster.width(),
            raster.height(),
            raster.into_raw(),
        ),
        placeholder_handle,
        width,
        height,
    })
}

fn svg_has_intrinsic_pixel_size(bytes: &[u8]) -> bool {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(document) = roxmltree::Document::parse(source) else {
        return false;
    };
    let root = document.root_element();
    let Some(width) = root.attribute("width") else {
        return false;
    };
    let Some(height) = root.attribute("height") else {
        return false;
    };
    svg_length_is_intrinsic(width) && svg_length_is_intrinsic(height)
}

fn svg_length_is_intrinsic(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.ends_with('%')
        && !value.eq_ignore_ascii_case("auto")
        && !value.eq_ignore_ascii_case("inherit")
}

fn validate_raster_decode_budget(path: &Path, width: u32, height: u32) -> Result<(), String> {
    let rgba_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("original image {path:?} has invalid dimensions"))?;
    let max_alloc = ::image::Limits::default().max_alloc.unwrap_or(u64::MAX);
    if rgba_bytes > max_alloc {
        return Err(format!(
            "Original image {path:?} needs {rgba_bytes} decoded RGBA bytes, exceeding the decoder's {} MiB allocation limit",
            max_alloc / 1024 / 1024
        ));
    }
    Ok(())
}

fn decoder_limits_for_total_bytes(
    path: &Path,
    total_bytes: u64,
) -> Result<::image::Limits, String> {
    let mut limits = ::image::Limits::default();
    limits.reserve(total_bytes).map_err(|error| {
        format!("original image {path:?} exceeds the decoder allocation limit: {error}")
    })?;
    Ok(limits)
}

fn original_image_preview_cancelled(path: &Path) -> String {
    format!("original image preview cancelled for {path:?}")
}

fn ensure_preview_file_size(path: &Path, max_file_bytes: u64) -> Result<(), String> {
    if max_file_bytes == 0 {
        return Ok(());
    }
    let byte_len = std::fs::metadata(path)
        .map_err(|error| format!("could not inspect original image {path:?}: {error}"))?
        .len();
    if byte_len > max_file_bytes {
        return Err(preview_file_size_error(path, byte_len, max_file_bytes));
    }
    Ok(())
}

fn preview_file_size_error(path: &Path, byte_len: u64, max_file_bytes: u64) -> String {
    format!(
        "Original image {path:?} is too large to preview ({byte_len} bytes; limit {max_file_bytes} bytes)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::ImageEncoder;
    use iced::advanced::svg::Data;
    use tempfile::tempdir;

    const MAX_FILE_BYTES: u64 = 1024 * 1024;

    #[tokio::test]
    async fn loads_png_as_rgba_source_with_full_dimensions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("photo.png");
        let source =
            ::image::RgbaImage::from_fn(3, 2, |x, y| ::image::Rgba([x as u8, y as u8, 7, 255]));
        source
            .save_with_format(&path, ::image::ImageFormat::Png)
            .unwrap();

        let preview =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .unwrap();
        let OriginalImagePreview::Raster {
            raster_handle,
            placeholder_handle,
            width,
            height,
        } = preview
        else {
            panic!("PNG must load as a raster preview");
        };
        assert_eq!((width, height), (3, 2));
        assert!(matches!(
            raster_handle,
            iced_image::Handle::Rgba {
                width: 3,
                height: 2,
                ..
            }
        ));
        assert!(matches!(
            placeholder_handle,
            iced_image::Handle::Rgba { .. }
        ));
    }

    #[tokio::test]
    async fn reuses_provided_placeholder_handle() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("photo.png");
        ::image::RgbaImage::from_pixel(3, 2, ::image::Rgba([1, 2, 3, 255]))
            .save_with_format(&path, ::image::ImageFormat::Png)
            .unwrap();
        let placeholder_handle = iced_image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]);
        let placeholder_id = placeholder_handle.id();

        let preview = load_original_image_preview(
            path,
            MAX_FILE_BYTES,
            CancellationToken::new(),
            Some(placeholder_handle),
        )
        .await
        .unwrap();
        let OriginalImagePreview::Raster {
            placeholder_handle, ..
        } = preview
        else {
            panic!("PNG must load as a raster preview");
        };

        assert_eq!(placeholder_handle.id(), placeholder_id);
    }

    #[tokio::test]
    async fn loads_jpeg_as_full_raster_dimensions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("photo.jpg");
        let source = ::image::RgbImage::from_fn(4, 3, |x, y| ::image::Rgb([x as u8, y as u8, 11]));
        source
            .save_with_format(&path, ::image::ImageFormat::Jpeg)
            .unwrap();

        let preview =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .unwrap();
        let OriginalImagePreview::Raster { width, height, .. } = preview else {
            panic!("JPEG must load as a raster preview");
        };
        assert_eq!((width, height), (4, 3));
    }

    #[tokio::test]
    async fn loads_svg_from_source_bytes_without_rasterizing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("vector.svg");
        std::fs::write(
            &path,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="7" height="5"><rect width="7" height="5" fill="red"/></svg>"#,
        )
        .unwrap();

        let preview =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .unwrap();
        let OriginalImagePreview::Svg {
            handle,
            width,
            height,
            has_intrinsic_size,
        } = preview
        else {
            panic!("SVG must retain an SVG handle");
        };
        assert_eq!((width, height), (7, 5));
        assert!(has_intrinsic_size);
        assert!(matches!(handle.data(), Data::Bytes(_)));

        let no_size_path = directory.path().join("viewbox-only.svg");
        std::fs::write(
            &no_size_path,
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 5"><rect width="7" height="5"/></svg>"#,
        )
        .unwrap();
        let no_size = load_original_image_preview(
            no_size_path,
            MAX_FILE_BYTES,
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let OriginalImagePreview::Svg {
            has_intrinsic_size, ..
        } = no_size
        else {
            panic!("viewBox-only SVG must load as SVG preview");
        };
        assert!(!has_intrinsic_size);
    }

    #[tokio::test]
    async fn loads_exif_rotated_jpeg_with_renderer_dimensions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("rotated.jpg");
        write_exif_rotated_jpeg(&path, 4, 2);

        let preview =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .unwrap();
        let OriginalImagePreview::Raster {
            placeholder_handle,
            width,
            height,
            ..
        } = preview
        else {
            panic!("JPEG must load as a raster preview");
        };

        assert_eq!((width, height), (2, 4));
        let (placeholder_width, placeholder_height) = match placeholder_handle {
            iced_image::Handle::Rgba { width, height, .. } => (width, height),
            _ => panic!("decoded placeholder must be an RGBA handle"),
        };
        assert!(placeholder_height > placeholder_width);
        assert!(placeholder_width <= ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE);
        assert!(placeholder_height <= ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE);
    }

    #[tokio::test]
    async fn rejects_invalid_image_source() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("broken.png");
        std::fs::write(&path, b"not an image").unwrap();

        let error =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .expect_err("invalid image must fail");
        assert!(error.contains("original image"));
    }

    #[tokio::test]
    async fn rejects_png_with_valid_header_and_truncated_pixels() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("truncated.png");
        let source = ::image::RgbaImage::from_fn(32, 32, |x, y| {
            ::image::Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
        });
        source
            .save_with_format(&path, ::image::ImageFormat::Png)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let idat_type = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("PNG must contain image data");
        let idat_len =
            u32::from_be_bytes(bytes[idat_type - 4..idat_type].try_into().unwrap()) as usize;
        bytes.truncate(idat_type + 4 + idat_len / 2);
        std::fs::write(&path, &bytes).unwrap();
        let decoder = ::image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .expect("truncated payload must retain a valid PNG header");
        assert_eq!(decoder.dimensions(), (32, 32));

        let error =
            load_original_image_preview(path, MAX_FILE_BYTES, CancellationToken::new(), None)
                .await
                .expect_err("truncated pixel data must fail before reaching the renderer");

        assert!(error.contains("could not decode original image"));
    }

    #[tokio::test]
    async fn cancelled_load_stops_before_publishing_an_image() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = load_original_image_preview(
            PathBuf::from("/workspace/photo.png"),
            MAX_FILE_BYTES,
            cancellation,
            None,
        )
        .await
        .expect_err("cancelled original image load must stop");

        assert!(error.contains("cancelled"));
    }

    #[tokio::test]
    async fn rejects_source_over_preview_file_limit() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("large.png");
        let source = ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([1, 2, 3, 255]));
        source
            .save_with_format(&path, ::image::ImageFormat::Png)
            .unwrap();
        let source_len = std::fs::metadata(&path).unwrap().len();

        let error =
            load_original_image_preview(path, source_len - 1, CancellationToken::new(), None)
                .await
                .expect_err("source over the configured limit must fail");
        assert!(error.contains("too large to preview"));
    }

    #[test]
    fn accepts_raster_dimensions_above_the_old_preview_budget() {
        validate_raster_decode_budget(Path::new("/workspace/large.png"), 8_000, 6_000)
            .expect("decoder default must allow a 192 MB RGBA image");
    }

    #[tokio::test]
    #[ignore = "allocates and decodes a real 183 MiB RGBA image; run explicitly"]
    async fn decodes_real_image_above_the_old_128_mib_budget() {
        const WIDTH: u32 = 8_000;
        const HEIGHT: u32 = 6_000;

        let directory = tempdir().unwrap();
        let path = directory.path().join("real-183-mib.png");
        let source =
            ::image::RgbaImage::from_pixel(WIDTH, HEIGHT, ::image::Rgba([17, 34, 51, 255]));
        source
            .save_with_format(&path, ::image::ImageFormat::Png)
            .unwrap();
        drop(source);
        let source_len = std::fs::metadata(&path).unwrap().len();

        let preview = load_original_image_preview(path, source_len, CancellationToken::new(), None)
            .await
            .expect("a real 183 MiB decoded RGBA image must pass the decoder budget");
        let OriginalImagePreview::Raster {
            raster_handle,
            placeholder_handle,
            width,
            height,
        } = preview
        else {
            panic!("PNG must load as a raster preview");
        };

        assert_eq!((width, height), (WIDTH, HEIGHT));
        assert!(matches!(raster_handle, iced_image::Handle::Rgba { .. }));
        let (placeholder_width, placeholder_height) = match placeholder_handle {
            iced_image::Handle::Rgba { width, height, .. } => (width, height),
            _ => panic!("decoded placeholder must be an RGBA handle"),
        };
        assert!(placeholder_width <= ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE);
        assert!(placeholder_height <= ORIGINAL_IMAGE_PLACEHOLDER_MAX_EDGE);
    }

    #[test]
    fn rejects_raster_dimensions_over_decoder_budget() {
        let max_alloc = ::image::Limits::default().max_alloc.unwrap();
        let width = (max_alloc / 4 + 1) as u32;
        let error = validate_raster_decode_budget(Path::new("/workspace/huge.png"), width, 1)
            .expect_err("decoded RGBA buffer over the decoder limit must be rejected");
        assert!(error.contains("decoded RGBA bytes"));
    }

    #[test]
    fn rejects_high_bit_depth_decoder_allocation_above_default_limit() {
        let path = Path::new("/workspace/rgba16.png");
        let (width, height) = (9_000_u32, 9_000_u32);
        validate_raster_decode_budget(path, width, height)
            .expect("RGBA8 renderer budget alone would allow this image");
        let rgba16_bytes = u64::from(width) * u64::from(height) * 8;

        let error = decoder_limits_for_total_bytes(path, rgba16_bytes)
            .expect_err("RGBA16 decoder allocation over the default limit must fail");

        assert!(error.contains("decoder allocation limit"));
    }

    fn write_exif_rotated_jpeg(path: &Path, width: u32, height: u32) {
        let pixels = vec![127; width as usize * height as usize * 3];
        let mut encoder =
            ::image::codecs::jpeg::JpegEncoder::new(std::fs::File::create(path).unwrap());
        encoder.set_exif_metadata(exif_orientation(6)).unwrap();
        encoder
            .write_image(&pixels, width, height, ::image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    fn exif_orientation(value: u16) -> Vec<u8> {
        vec![
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0,
            0x12,
            0x01,
            3,
            0,
            1,
            0,
            0,
            0,
            value as u8,
            (value >> 8) as u8,
            0,
            0,
            0,
            0,
            0,
            0,
        ]
    }
}
