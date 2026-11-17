use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{GrayImage, ImageFormat, RgbImage, RgbaImage};

use super::row_reader::{self, BAND_ROWS};

fn read_all(path: &Path, format: ImageFormat) -> RgbaImage {
    let mut reader = row_reader::open(path, format)
        .expect("open")
        .expect("readable row by row");
    let (width, height) = reader.dimensions();
    let row_bytes = width as usize * 4;
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    let mut band = vec![0; row_bytes * BAND_ROWS];
    loop {
        let rows = reader.read_rows(&mut band).expect("rows");
        if rows == 0 {
            break;
        }
        pixels.extend_from_slice(&band[..rows * row_bytes]);
    }
    RgbaImage::from_raw(width, height, pixels).expect("every row was read")
}

fn largest_channel_difference(left: &RgbaImage, right: &RgbaImage) -> u8 {
    left.as_raw()
        .iter()
        .zip(right.as_raw())
        .map(|(left, right)| left.abs_diff(*right))
        .max()
        .unwrap_or(0)
}

fn write_jpeg(path: &Path, picture: &image::DynamicImage) {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 92)
        .encode_image(picture)
        .expect("encode jpeg");
    std::fs::write(path, bytes).expect("write jpeg");
}

/// 系统 libjpeg-turbo 逐行解出来的和整张解的一样（两边 IDCT、上采样的舍入不同，差几个色阶）。
#[test]
fn a_jpeg_read_row_by_row_matches_the_whole_picture() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("photo.jpg");
    let picture = RgbImage::from_fn(613, 409, |x, y| {
        image::Rgb([
            (x * 255 / 613) as u8,
            (y * 255 / 409) as u8,
            ((x ^ y) & 0xff) as u8,
        ])
    });
    write_jpeg(&path, &image::DynamicImage::ImageRgb8(picture));

    let rows = read_all(&path, ImageFormat::Jpeg);
    let whole = image::open(&path).expect("decode whole").into_rgba8();

    assert_eq!(rows.dimensions(), (613, 409));
    assert!(rows.pixels().all(|pixel| pixel.0[3] == u8::MAX));
    assert!(largest_channel_difference(&rows, &whole) <= 8);
}

#[test]
fn a_grayscale_jpeg_comes_out_as_gray_rgba() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("gray.jpg");
    let picture = GrayImage::from_fn(100, 60, |x, _| image::Luma([(x * 2) as u8]));
    write_jpeg(&path, &image::DynamicImage::ImageLuma8(picture));

    let rows = read_all(&path, ImageFormat::Jpeg);

    assert!(rows
        .pixels()
        .all(|pixel| pixel.0[0] == pixel.0[1] && pixel.0[1] == pixel.0[2] && pixel.0[3] == 255));
}

/// 坏文件只返回错误：libjpeg 的出错跳转在 C 包装里接住，不会把进程带崩。
#[test]
fn a_file_that_is_not_a_jpeg_is_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("broken.jpg");
    std::fs::write(&path, b"this is not a jpeg at all").expect("write");

    assert!(row_reader::open(&path, ImageFormat::Jpeg).is_err());
}

/// 截断的 JPEG 能解出多少给多少（和常见看图软件一样），读完整个高度、不出错也不崩。
#[test]
fn a_truncated_jpeg_still_reads_to_the_end() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("truncated.jpg");
    let picture = RgbImage::from_fn(320, 240, |x, y| image::Rgb([x as u8, y as u8, 7]));
    write_jpeg(&path, &image::DynamicImage::ImageRgb8(picture));
    let bytes = std::fs::read(&path).expect("read");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate");

    let rows = read_all(&path, ImageFormat::Jpeg);

    assert_eq!(rows.dimensions(), (320, 240));
}
