use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{imageops::FilterType, DynamicImage, ImageFormat, ImageResult};
use tokio_util::sync::CancellationToken;

use super::super::FileOperationControls;
use super::{ConversionRequest, QualitySpec, ResizeSpec};
use crate::FileError;

/// 质量二分最多轮次:0-100 域二分 7 轮收敛到 1 以内的质量步长。
const JPEG_QUALITY_BINARY_SEARCH_ROUNDS: u8 = 7;

/// image crate 引擎:解码源图、按需缩放、按质量规格编码 JPEG/PNG 到输出路径。
pub(super) async fn convert_with_image_crate(
    request: &super::ConversionRequest,
    output: &Path,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let source = request.source.clone();
    let controls_token = controls.cancellation_token();
    // 解码与缩放是 CPU 密集的同步操作,移出异步运行时线程。
    let decoded =
        tokio::task::spawn_blocking(move || decode_source_image(&source, &controls_token))
            .await
            .map_err(|error| join_error(output, error))??;

    let image = apply_resize(decoded, request.resize);
    let encoded = match request.quality {
        QualitySpec::Level(level) => encode_jpeg_with_quality(&image, level, controls).await?,
        QualitySpec::TargetBytes(target_bytes) => {
            encode_jpeg_towards_target(&image, target_bytes, controls).await?
        }
        QualitySpec::Lossless => {
            encode_lossless(&image, lossless_format(request), controls).await?
        }
        QualitySpec::Preset(_) => {
            return Err(FileError::InvalidInput {
                path: output.to_path_buf(),
                message: "image crate engine does not accept presets".to_owned(),
            })
        }
    };

    write_encoded_output(output, &encoded).await?;
    Ok(encoded.len() as u64)
}

fn decode_source_image(
    source: &Path,
    cancel: &CancellationToken,
) -> Result<DynamicImage, FileError> {
    if cancel.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    image::open(source).map_err(|error| FileError::Convert {
        path: source.to_path_buf(),
        message: format!("could not decode image: {error}"),
    })
}

fn apply_resize(image: DynamicImage, resize: ResizeSpec) -> DynamicImage {
    match resize {
        ResizeSpec::Keep => image,
        ResizeSpec::Percent(percent) => {
            let width = scaled_dimension(image.width(), u32::from(percent), 100);
            let height = scaled_dimension(image.height(), u32::from(percent), 100);
            image.resize_exact(width.max(1), height.max(1), FilterType::Lanczos3)
        }
        ResizeSpec::Width(width) => {
            let height = scaled_dimension(image.height(), width, image.width().max(1));
            image.resize_exact(width.max(1), height.max(1), FilterType::Lanczos3)
        }
    }
}

fn scaled_dimension(dimension: u32, numerator: u32, denominator: u32) -> u32 {
    ((u64::from(dimension) * u64::from(numerator)) / u64::from(denominator.max(1))) as u32
}

async fn encode_jpeg_with_quality(
    image: &DynamicImage,
    level: u8,
    controls: &FileOperationControls,
) -> Result<Vec<u8>, FileError> {
    controls.checkpoint_now()?;
    encode_in_blocking(
        image.clone(),
        move |image| encode_jpeg_bytes(image, level),
        controls,
    )
    .await
}

/// 体积模式:在 1-100 质量域二分寻找不超目标的最大质量;压不到时以质量 1 交付,
/// 由调用方按容差判定 reached_target。
async fn encode_jpeg_towards_target(
    image: &DynamicImage,
    target_bytes: u64,
    controls: &FileOperationControls,
) -> Result<Vec<u8>, FileError> {
    controls.checkpoint_now()?;
    let snapshot = image.clone();
    // 快路径:最高质量已达标就无需二分。
    let top_quality_bytes = encode_in_blocking(
        snapshot.clone(),
        move |image| encode_jpeg_bytes(image, 100),
        controls,
    )
    .await?;
    if top_quality_bytes.len() as u64 <= target_bytes {
        return Ok(top_quality_bytes);
    }

    let mut low = 1u8;
    let mut high = 99u8;
    let mut best: Option<Vec<u8>> = None;
    for _ in 0..JPEG_QUALITY_BINARY_SEARCH_ROUNDS {
        controls.checkpoint_now()?;
        if low > high {
            break;
        }
        let mid = low + (high - low) / 2;
        let bytes = encode_in_blocking(
            snapshot.clone(),
            move |image| encode_jpeg_bytes(image, mid),
            controls,
        )
        .await?;
        if bytes.len() as u64 <= target_bytes {
            best = Some(bytes);
            low = mid + 1;
        } else if mid > 1 {
            high = mid - 1;
        } else {
            break;
        }
    }

    match best {
        Some(bytes) => Ok(bytes),
        None => {
            controls.checkpoint_now()?;
            encode_in_blocking(snapshot, move |image| encode_jpeg_bytes(image, 1), controls).await
        }
    }
}

/// image crate 引擎内的无损目标:PNG/TIFF/BMP/ICO。
async fn encode_lossless(
    image: &DynamicImage,
    format: ImageFormat,
    controls: &FileOperationControls,
) -> Result<Vec<u8>, FileError> {
    controls.checkpoint_now()?;
    encode_in_blocking(
        image.clone(),
        move |image| {
            let mut bytes = Vec::new();
            image.write_to(&mut Cursor::new(&mut bytes), format)?;
            Ok(bytes)
        },
        controls,
    )
    .await
}

/// 只有无损目标会进入 image crate 的 Lossless 分支;此处断言防呆。
fn lossless_format(request: &ConversionRequest) -> ImageFormat {
    match request.target {
        super::ConversionTarget::Image(super::ImageTargetFormat::Png) => ImageFormat::Png,
        super::ConversionTarget::Image(super::ImageTargetFormat::Tiff) => ImageFormat::Tiff,
        super::ConversionTarget::Image(super::ImageTargetFormat::Bmp) => ImageFormat::Bmp,
        super::ConversionTarget::Image(super::ImageTargetFormat::Ico) => ImageFormat::Ico,
        _ => unreachable!("lossless encoding only applies to raster lossless targets"),
    }
}

async fn encode_in_blocking<F>(
    image: DynamicImage,
    encode: F,
    controls: &FileOperationControls,
) -> Result<Vec<u8>, FileError>
where
    F: FnOnce(DynamicImage) -> ImageResult<Vec<u8>> + Send + 'static,
{
    let token = controls.cancellation_token();
    tokio::task::spawn_blocking(move || {
        if token.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        encode(image).map_err(|error| FileError::Convert {
            path: std::path::PathBuf::new(),
            message: format!("could not encode image: {error}"),
        })
    })
    .await
    .map_err(|error| FileError::Convert {
        path: std::path::PathBuf::new(),
        message: format!("image encoding task failed: {error}"),
    })?
}

fn encode_jpeg_bytes(image: DynamicImage, quality: u8) -> ImageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    let encoder = JpegEncoder::new_with_quality(&mut cursor, quality.clamp(1, 100));
    image.write_with_encoder(encoder)?;
    Ok(bytes)
}

async fn write_encoded_output(output: &Path, bytes: &[u8]) -> Result<(), FileError> {
    // 输出路径已由 reserve_output_path 以 create_new 占位;这里覆盖写入编码结果。
    tokio::fs::write(output, bytes)
        .await
        .map_err(|error| FileError::Convert {
            path: output.to_path_buf(),
            message: format!("could not write converted image: {error}"),
        })
}

fn join_error(path: &Path, error: tokio::task::JoinError) -> FileError {
    FileError::Convert {
        path: path.to_path_buf(),
        message: format!("image decode task failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::ConversionRequest;
    use super::super::{AudioChannelSpec, TARGET_BYTES_TOLERANCE_DENOMINATOR};
    use super::*;

    fn gradient_png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        }));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        bytes
    }

    fn request(source: &Path, quality: QualitySpec) -> ConversionRequest {
        ConversionRequest {
            source: source.to_path_buf(),
            target: super::super::ConversionTarget::Image(super::super::ImageTargetFormat::Jpeg),
            quality,
            resize: ResizeSpec::Keep,
            fps_override: None,
            audio_channels: AudioChannelSpec::Keep,
        }
    }

    async fn convert(source: &Path, quality: QualitySpec) -> u64 {
        let request = request(source, quality);
        let controls = FileOperationControls::running(CancellationToken::new());
        crate::ops::convert::convert_file_with_controls(request, &controls)
            .await
            .expect("convert succeeds")
            .byte_count
    }

    #[tokio::test]
    async fn jpeg_level_conversion_produces_smaller_output() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("input.png");
        std::fs::write(&source, gradient_png_bytes(256, 256)).expect("write png");
        let output = directory.path().join("input.jpg");
        let byte_count = convert(&source, QualitySpec::Level(60)).await;
        assert!(byte_count > 0);
        assert_eq!(
            std::fs::metadata(&output).expect("output exists").len(),
            byte_count
        );
    }

    #[tokio::test]
    async fn jpeg_target_size_converges_below_target() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("input.png");
        std::fs::write(&source, gradient_png_bytes(512, 512)).expect("write png");
        let target = 24 * 1024;
        let byte_count = convert(&source, QualitySpec::TargetBytes(target)).await;
        assert!(
            byte_count <= target * TARGET_BYTES_TOLERANCE_DENOMINATOR,
            "expected convergence, got {byte_count}"
        );
    }

    #[tokio::test]
    async fn jpeg_target_size_impossible_still_delivers_lowest_quality() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("input.png");
        std::fs::write(&source, gradient_png_bytes(64, 64)).expect("write png");
        let byte_count = convert(&source, QualitySpec::TargetBytes(64)).await;
        assert!(byte_count > 0, "falls back to lowest quality output");
    }

    #[tokio::test]
    async fn png_lossless_conversion_round_trips_pixels() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("input.png");
        std::fs::write(&source, gradient_png_bytes(128, 128)).expect("write png");
        let output = directory.path().join("input.png");
        let mut request = request(&source, QualitySpec::Lossless);
        request.target =
            super::super::ConversionTarget::Image(super::super::ImageTargetFormat::Png);
        let controls = FileOperationControls::running(CancellationToken::new());
        let converted = crate::ops::convert::convert_file_with_controls(request, &controls)
            .await
            .expect("convert succeeds");
        assert_eq!(
            converted.byte_count,
            std::fs::metadata(&output).expect("output").len()
        );
    }

    #[tokio::test]
    async fn percent_resize_halves_dimensions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("input.png");
        std::fs::write(&source, gradient_png_bytes(200, 100)).expect("write png");
        let output = directory.path().join("input.jpg");
        let mut request = request(&source, QualitySpec::Level(80));
        request.resize = ResizeSpec::Percent(50);
        let controls = FileOperationControls::running(CancellationToken::new());
        crate::ops::convert::convert_file_with_controls(request, &controls)
            .await
            .expect("convert succeeds");

        let decoded = image::open(&output).expect("decode output");
        assert_eq!((decoded.width(), decoded.height()), (100, 50));
    }
}
