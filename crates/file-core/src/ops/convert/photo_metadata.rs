//! 转换时带过去的照片信息：EXIF（拍摄时间、相机、GPS……）和 ICC 颜色配置。
//! 读：kamadak-exif 认各种照片容器（含手机的 HEIC）；ICC 用 image 解码器只读文件头。
//! 写：img-parts 往 JPEG/PNG/WebP 输出里补，不重编码画面。

use std::io::{BufReader, Cursor};
use std::path::Path;

use exif::experimental::Writer;
use exif::{Field, In, Tag, Value};
use image::ImageDecoder;
use img_parts::{DynImage, ImageEXIF, ImageICC};

use crate::FileError;

/// 转换后会错的字段：方向另写成“正常”（像素已摆正）；尺寸变了；厂商私有数据里
/// 存的是相对原文件的偏移，重写后会坏，还常占几十 KB。内嵌缩略图在第二个 IFD，整个不带。
const DROPPED_TAGS: [Tag; 6] = [
    Tag::Orientation,
    Tag::PixelXDimension,
    Tag::PixelYDimension,
    Tag::ImageWidth,
    Tag::ImageLength,
    Tag::MakerNote,
];
const UPRIGHT: u16 = 1;
/// ICC 头里色彩空间签名的位置；只带 RGB 的：CMYK/灰度源解出来已经是 RGB 像素。
const ICC_COLOR_SPACE: std::ops::Range<usize> = 16..20;
/// 容器为每段信息加的头（JPEG APP 段、PNG/RIFF 块）；按大小模式扣预算时算上。
const CONTAINER_OVERHEAD_BYTES: u64 = 64;

/// 要写进输出的信息；两样都没有时不用补。
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CarriedMetadata {
    exif: Option<Vec<u8>>,
    icc: Option<Vec<u8>>,
}

impl CarriedMetadata {
    pub(super) fn is_empty(&self) -> bool {
        self.exif.is_none() && self.icc.is_none()
    }

    pub(super) fn byte_len(&self) -> u64 {
        [&self.exif, &self.icc]
            .into_iter()
            .flatten()
            .map(|bytes| bytes.len() as u64 + CONTAINER_OVERHEAD_BYTES)
            .sum()
    }
}

/// 阻塞：只读文件头。源文件打不开时什么都不带，转换本身会报那个错。
pub(super) fn read_carried_metadata(source: &Path) -> CarriedMetadata {
    CarriedMetadata {
        exif: read_upright_exif(source),
        icc: read_rgb_icc(source),
    }
}

fn read_upright_exif(source: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(source).ok()?;
    let read = exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut BufReader::new(file))
        .or_else(|error| {
            error.distill_partial_result(|errors| {
                tracing::warn!(
                    target: "file_core::convert",
                    path = %source.display(),
                    skipped = errors.len(),
                    "some exif fields could not be read; carrying the rest"
                );
            })
        });
    match read {
        Ok(exif) => upright_exif(&exif, source),
        Err(exif::Error::NotFound(_)) => None,
        // 信息坏了不该让整张图转换失败：像素没问题，只是没信息可带。
        Err(error) => {
            tracing::warn!(
                target: "file_core::convert",
                path = %source.display(),
                error = %error,
                "exif could not be read; converting without it"
            );
            None
        }
    }
}

fn upright_exif(exif: &exif::Exif, source: &Path) -> Option<Vec<u8>> {
    let upright = Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![UPRIGHT]),
    };
    let mut writer = Writer::new();
    for field in exif
        .fields()
        .filter(|field| field.ifd_num == In::PRIMARY && !DROPPED_TAGS.contains(&field.tag))
    {
        writer.push_field(field);
    }
    writer.push_field(&upright);
    let mut bytes = Cursor::new(Vec::new());
    match writer.write(&mut bytes, exif.little_endian()) {
        Ok(()) => Some(bytes.into_inner()),
        Err(error) => {
            tracing::warn!(
                target: "file_core::convert",
                path = %source.display(),
                error = %error,
                "exif could not be rewritten; converting without it"
            );
            None
        }
    }
}

fn read_rgb_icc(source: &Path) -> Option<Vec<u8>> {
    let mut decoder = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    decoder
        .icc_profile()
        .ok()
        .flatten()
        .filter(|icc| icc.get(ICC_COLOR_SPACE) == Some(b"RGB ".as_slice()))
}

/// 阻塞：把信息补进已经写好的输出，返回补完后的大小。
pub(super) fn embed_carried_metadata(
    output: &Path,
    carried: CarriedMetadata,
) -> Result<u64, FileError> {
    let failed = |message: String| FileError::Convert {
        path: output.to_path_buf(),
        message,
    };
    let bytes =
        std::fs::read(output).map_err(|error| failed(format!("could not read output: {error}")))?;
    let mut image = DynImage::from_bytes(bytes.into())
        .map_err(|error| failed(format!("could not parse output container: {error}")))?
        .ok_or_else(|| failed("output format cannot carry photo metadata".to_owned()))?;
    image.set_exif(carried.exif.map(Into::into));
    image.set_icc_profile(carried.icc.map(Into::into));
    let file = std::fs::File::create(output)
        .map_err(|error| failed(format!("could not rewrite output: {error}")))?;
    image
        .encoder()
        .write_to(file)
        .map_err(|error| failed(format!("could not write photo metadata: {error}")))
}

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;
    use exif::Rational;
    use image::ImageEncoder;

    /// 假 ICC：只有头部色彩空间签名有意义。
    pub(in crate::ops::convert) fn icc(color_space: &[u8; 4]) -> Vec<u8> {
        let mut icc = vec![0u8; 132];
        icc[16..20].copy_from_slice(color_space);
        icc
    }

    /// 像手机拍的：竖拍（方向 6，像素横着存）、带型号、拍摄时间、GPS、
    /// 厂商私有数据和内嵌缩略图。
    pub(in crate::ops::convert) fn phone_photo_exif() -> Vec<u8> {
        let fields = [
            Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"Pixel 9".to_vec()]),
            },
            Field {
                tag: Tag::Orientation,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![6]),
            },
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"2026:11:18 09:30:00".to_vec()]),
            },
            Field {
                tag: Tag::PixelXDimension,
                ifd_num: In::PRIMARY,
                value: Value::Long(vec![400]),
            },
            Field {
                tag: Tag::MakerNote,
                ifd_num: In::PRIMARY,
                value: Value::Undefined(vec![7; 64], 0),
            },
            Field {
                tag: Tag::GPSLatitudeRef,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"N".to_vec()]),
            },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 31, denom: 1 },
                    Rational { num: 14, denom: 1 },
                    Rational { num: 0, denom: 1 },
                ]),
            },
            Field {
                tag: Tag::Compression,
                ifd_num: In::THUMBNAIL,
                value: Value::Short(vec![6]),
            },
        ];
        let thumbnail = [0xFF, 0xD8, 0xFF, 0xD9];
        let mut writer = Writer::new();
        for field in &fields {
            writer.push_field(field);
        }
        writer.set_jpeg(&thumbnail, In::THUMBNAIL);
        let mut bytes = Cursor::new(Vec::new());
        writer.write(&mut bytes, false).expect("write exif");
        bytes.into_inner()
    }

    /// 400×300 存储、方向 6 的 JPEG（看上去是 300×400 的竖图），带 EXIF 和 ICC。
    pub(in crate::ops::convert) fn write_phone_photo(path: &Path, icc_color_space: &[u8; 4]) {
        let pixels = image::RgbImage::from_fn(400, 300, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
        encoder
            .set_exif_metadata(phone_photo_exif())
            .expect("jpeg takes exif");
        encoder
            .set_icc_profile(icc(icc_color_space))
            .expect("jpeg takes icc");
        encoder
            .write_image(&pixels, 400, 300, image::ExtendedColorType::Rgb8)
            .expect("encode jpeg");
        std::fs::write(path, bytes).expect("write photo");
    }

    /// 输出里读回来的 EXIF；没有就是 None。
    pub(in crate::ops::convert) fn exif_of(path: &Path) -> Option<exif::Exif> {
        let file = std::fs::File::open(path).expect("open output");
        exif::Reader::new()
            .read_from_container(&mut BufReader::new(file))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn exif_is_rewritten_upright_without_stale_or_private_fields() {
        let directory = tempfile::tempdir().expect("tempdir");
        let photo = directory.path().join("photo.jpg");
        write_phone_photo(&photo, b"RGB ");

        let carried = read_carried_metadata(&photo);
        assert_eq!(carried.icc, Some(icc(b"RGB ")));
        let exif = exif::Reader::new()
            .read_raw(carried.exif.clone().expect("exif carried"))
            .expect("rewritten exif parses");
        let field = |tag| exif.get_field(tag, In::PRIMARY);
        assert_eq!(
            field(Tag::Orientation).and_then(|field| field.value.get_uint(0)),
            Some(1)
        );
        assert!(field(Tag::Model).is_some());
        assert!(field(Tag::DateTimeOriginal).is_some());
        assert!(field(Tag::GPSLatitude).is_some());
        assert!(field(Tag::MakerNote).is_none());
        assert!(field(Tag::PixelXDimension).is_none());
        assert!(exif.fields().all(|field| field.ifd_num == In::PRIMARY));
        assert!(carried.byte_len() > carried.exif.as_ref().map_or(0, Vec::len) as u64);
    }

    #[test]
    fn only_rgb_color_profiles_are_carried() {
        let directory = tempfile::tempdir().expect("tempdir");
        let photo = directory.path().join("print.jpg");
        write_phone_photo(&photo, b"CMYK");
        assert_eq!(read_carried_metadata(&photo).icc, None);
    }

    #[test]
    fn plain_images_carry_nothing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let plain = directory.path().join("plain.png");
        image::RgbImage::new(8, 8)
            .save(&plain)
            .expect("write plain png");
        assert!(read_carried_metadata(&plain).is_empty());
    }

    /// WebP 由 ffmpeg 编，信息由 img-parts 事后补；没装 ffmpeg 时跳过（同 convert_smoke）。
    #[tokio::test]
    async fn webp_from_ffmpeg_carries_or_drops_photo_metadata_with_real_ffmpeg() {
        use crate::ops::convert::{
            convert_file_with_controls, AudioChannelSpec, ConversionQualityPreset,
            ConversionRequest, ConversionTarget, ImageTargetFormat, MediaEdits, OutputLocation,
            PhotoMetadata, QualitySpec, ResizeSpec,
        };
        if crate::ops::convert::locate_media_tool(&["ffmpeg"])
            .await
            .is_none()
        {
            return;
        }
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("phone.jpg");
        write_phone_photo(&source, b"RGB ");
        for photo_metadata in [PhotoMetadata::Keep, PhotoMetadata::Strip] {
            let request = ConversionRequest {
                source: source.clone(),
                target: ConversionTarget::Image(ImageTargetFormat::Webp),
                quality: QualitySpec::Preset(ConversionQualityPreset::Medium),
                resize: ResizeSpec::Keep,
                fps_override: None,
                audio_channels: AudioChannelSpec::Keep,
                edits: MediaEdits::default(),
                photo_metadata,
                output: OutputLocation::BesideSource,
            };
            let controls = crate::ops::FileOperationControls::running(
                tokio_util::sync::CancellationToken::new(),
            );
            let converted = convert_file_with_controls(request, &controls)
                .await
                .expect("webp converts");
            let decoded = image::open(&converted.output).expect("decode webp");
            assert_eq!((decoded.width(), decoded.height()), (300, 400));
            let exif = exif_of(&converted.output);
            match photo_metadata {
                PhotoMetadata::Keep => {
                    let exif = exif.expect("exif kept");
                    let field = |tag| exif.get_field(tag, In::PRIMARY);
                    assert_eq!(
                        field(Tag::Orientation).and_then(|field| field.value.get_uint(0)),
                        Some(1)
                    );
                    assert!(field(Tag::GPSLatitude).is_some());
                }
                PhotoMetadata::Strip => assert!(exif.is_none()),
            }
            std::fs::remove_file(&converted.output).expect("clear output");
        }
    }
}
