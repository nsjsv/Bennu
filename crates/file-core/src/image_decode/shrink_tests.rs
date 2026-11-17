//! 缩小解码（JPEG 的 DCT 缩放、大 PNG 边读边缩）出来的图，要和整图解码再缩的一样；整图解码
//! 共用的内存预算满了要等。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbImage, RgbaImage};

use super::*;
use crate::decode_budget;

/// 平滑的渐变：红随 x、绿随 y 变，缩小的方法不同也只差取整。
fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, 128])
    })
}

fn jpeg_bytes(image: &RgbImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 95)
        .encode_image(image)
        .expect("encode jpeg");
    bytes
}

/// 在文件头后插一段只有方向的 EXIF（APP1，小端 TIFF）。
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend(orientation.to_le_bytes());
    exif.extend([0, 0, 0, 0, 0, 0]);
    let mut bytes = jpeg[..2].to_vec();
    bytes.extend([0xFF, 0xE1]);
    bytes.extend(
        u16::try_from(exif.len() + 2)
            .expect("short exif")
            .to_be_bytes(),
    );
    bytes.extend(exif);
    bytes.extend(&jpeg[2..]);
    bytes
}

/// 整图解码再缩的结果，当作标准答案。
fn whole_jpeg(path: &Path, bound: DecodeBound) -> DecodedImage {
    let mut decoder = JpegDecoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("open jpeg"),
    ))
    .expect("jpeg header");
    let orientation = decoder.orientation().expect("orientation");
    decode_whole(decoder, orientation, bound).expect("decode whole jpeg")
}

/// 两张同样大小的图逐通道平均差多少。
fn mean_difference(left: &RgbaImage, right: &RgbaImage) -> f64 {
    assert_eq!(left.dimensions(), right.dimensions());
    let total: u64 = left
        .as_raw()
        .iter()
        .zip(right.as_raw())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    total as f64 / left.as_raw().len() as f64
}

/// 大 JPEG 在解码时缩小，和整图解出来再缩的一样；报的还是原图大小。
#[test]
fn a_jpeg_shrunk_while_decoding_matches_the_whole_picture_shrunk() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("photo.jpg");
    std::fs::write(&path, jpeg_bytes(&gradient(1600, 1200))).expect("write jpeg");
    let bound = DecodeBound::within(200, 200);

    // 1/8 正好是 200×150：真的在解码时缩了，不是整图解了再缩。
    let scaled = scaled_jpeg::read_header(&path)
        .expect("jpeg header")
        .decode_scaled((200, 150))
        .expect("8-bit RGB");
    assert_eq!((scaled.width(), scaled.height()), (200, 150));

    let shrunk = decode_image(&path, RasterSource::Picture, bound).expect("decode");
    let whole = whole_jpeg(&path, bound);
    assert_eq!((shrunk.width, shrunk.height), (1600, 1200));
    assert_eq!(shrunk.pixels.dimensions(), (200, 150));
    assert!(!shrunk.has_alpha);
    let difference = mean_difference(&shrunk.pixels, &whole.pixels);
    assert!(difference < 3.0, "{difference}");
}

/// 要转正的 JPEG（EXIF 方向 6，顺时针转 90 度）缩小解码后照样转正。
#[test]
fn a_turned_jpeg_is_shrunk_and_turned_upright() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("turned.jpg");
    std::fs::write(
        &path,
        with_exif_orientation(&jpeg_bytes(&gradient(640, 320)), 6),
    )
    .expect("write jpeg");
    let bound = DecodeBound::within(80, 80);

    let shrunk = decode_image(&path, RasterSource::Picture, bound).expect("decode");
    let whole = whole_jpeg(&path, bound);
    assert_eq!((shrunk.width, shrunk.height), (320, 640));
    assert_eq!(shrunk.pixels.dimensions(), (40, 80));
    let difference = mean_difference(&shrunk.pixels, &whole.pixels);
    assert!(difference < 3.0, "{difference}");
}

/// 边读边缩的 PNG 和整图解出来再缩的一样，半透明照样保留。
#[test]
fn a_png_shrunk_row_by_row_matches_the_whole_picture_shrunk() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("alpha.png");
    let picture = RgbaImage::from_fn(600, 400, |x, y| {
        image::Rgba([(x * 255 / 600) as u8, (y * 255 / 400) as u8, 90, 160])
    });
    picture.save(&path).expect("save png");

    let rows = png_rows::decode(&path, (150, 100))
        .expect("decode")
        .expect("not interlaced");
    let whole = DynamicImage::ImageRgba8(picture)
        .thumbnail(150, 100)
        .into_rgba8();
    let difference = mean_difference(&rows, &whole);
    assert!(difference < 3.0, "{difference}");
    assert_eq!(rows.get_pixel(75, 50).0[3], 160);
}

/// 整图解码超出预算的大 PNG 走边读边缩，左半不透明的红、右半全透明照样分开。
#[test]
fn a_png_too_big_to_decode_whole_is_still_shown() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("huge.png");
    // 4200×4000 RGBA 整解要 67 MB，比预算大；一行行写，测试里也不占整图的内存。
    let (width, height) = (4200, 4000);
    let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("create png"));
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_compression(png::Compression::Fastest);
    let mut writer = encoder
        .write_header()
        .expect("png header")
        .into_stream_writer()
        .expect("png stream");
    let row: Vec<u8> = (0..width)
        .flat_map(|x| {
            if x < width / 2 {
                [200, 10, 10, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    for _ in 0..height {
        std::io::Write::write_all(&mut writer, &row).expect("png row");
    }
    writer.finish().expect("finish png");

    let decoded =
        decode_image(&path, RasterSource::Picture, DecodeBound::within(100, 100)).expect("decode");
    assert_eq!((decoded.width, decoded.height), (width, height));
    assert_eq!(decoded.pixels.dimensions(), (100, 95));
    assert_eq!(decoded.pixels.get_pixel(10, 50).0, [200, 10, 10, 255]);
    assert_eq!(decoded.pixels.get_pixel(90, 50).0[3], 0);
    assert!(decoded.has_alpha);
}

/// 整解再缩当标准答案（image 的 TIFF 解码器）。
fn whole_picture_shrunk(path: &Path, target: (u32, u32)) -> RgbaImage {
    image::open(path)
        .expect("decode whole tiff")
        .thumbnail(target.0, target.1)
        .into_rgba8()
}

/// 多条带的 16 位 RGB TIFF 按条带边读边缩，和整解再缩的一样。
#[test]
fn a_striped_tiff_is_shrunk_strip_by_strip() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("scan.tif");
    let (width, height): (u32, u32) = (320, 240);
    let samples: Vec<u16> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [(x * 200) as u16, (y * 270) as u16, 30_000]))
        .collect();
    let mut encoder =
        ::tiff::encoder::TiffEncoder::new(std::fs::File::create(&path).expect("create tiff"))
            .expect("tiff encoder");
    let mut image = encoder
        .new_image::<::tiff::encoder::colortype::RGB16>(width, height)
        .expect("tiff image");
    image.rows_per_strip(16).expect("rows per strip");
    image.write_data(&samples).expect("write tiff");
    let mut decoder = ::tiff::decoder::Decoder::new(std::fs::File::open(&path).expect("open tiff"))
        .expect("tiff header");
    assert!(decoder.strip_count().expect("strips") > 1);

    let rows = tiff_rows::decode(&path, (80, 60))
        .expect("decode")
        .expect("striped RGB16");
    let difference = mean_difference(&rows, &whole_picture_shrunk(&path, (80, 60)));
    assert!(difference < 3.0, "{difference}");
}

/// 分图块的 TIFF（右边、下边的图块只有一部分是图）按一排排图块边读边缩。
#[test]
fn a_tiled_tiff_is_shrunk_one_row_of_tiles_at_a_time() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("tiled.tif");
    let (width, height, tile) = (40u32, 24u32, 16u32);
    let (across, down) = (width.div_ceil(tile), height.div_ceil(tile));
    let tile_bytes = tile * tile * 3;
    // 小端 TIFF：头 8 字节，图块数据，然后 IFD 与它引用的数组。
    let mut file = b"II*\0\0\0\0\0".to_vec();
    let mut offsets = Vec::new();
    for tile_row in 0..down {
        for tile_column in 0..across {
            offsets.push(file.len() as u32);
            for y in 0..tile {
                for x in 0..tile {
                    let (gx, gy) = (tile_column * tile + x, tile_row * tile + y);
                    file.extend([(gx * 6) as u8, (gy * 10) as u8, 77]);
                }
            }
        }
    }
    let bits_offset = file.len() as u32;
    file.extend([8, 0, 8, 0, 8, 0]);
    let offsets_offset = file.len() as u32;
    file.extend(offsets.iter().flat_map(|offset| offset.to_le_bytes()));
    let counts_offset = file.len() as u32;
    file.extend(offsets.iter().flat_map(|_| tile_bytes.to_le_bytes()));
    let ifd_offset = file.len() as u32;
    file[4..8].copy_from_slice(&ifd_offset.to_le_bytes());
    let count = across * down;
    // (标签, 类型 3=SHORT 4=LONG, 个数, 值或偏移)
    let entries: [(u16, u16, u32, u32); 11] = [
        (256, 4, 1, width),
        (257, 4, 1, height),
        (258, 3, 3, bits_offset),
        (259, 3, 1, 1),
        (262, 3, 1, 2),
        (277, 3, 1, 3),
        (284, 3, 1, 1),
        (322, 4, 1, tile),
        (323, 4, 1, tile),
        (324, 4, count, offsets_offset),
        (325, 4, count, counts_offset),
    ];
    file.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        file.extend(tag.to_le_bytes());
        file.extend(kind.to_le_bytes());
        file.extend(count.to_le_bytes());
        file.extend(value.to_le_bytes());
    }
    file.extend(0u32.to_le_bytes());
    std::fs::write(&path, file).expect("write tiff");

    let rows = tiff_rows::decode(&path, (20, 12))
        .expect("decode")
        .expect("tiled RGB8");
    let difference = mean_difference(&rows, &whole_picture_shrunk(&path, (20, 12)));
    assert!(difference < 3.0, "{difference}");
}

/// 每种图走哪条路：缩一半以上的 JPEG 解码时缩，超出预算的 PNG 逐行读，要缩的 TIFF 按段读；
/// 不用缩的整解。
#[test]
fn each_picture_takes_the_cheapest_decode_route() {
    let thumbnail = |size| {
        ShrinkPlan::new(
            size,
            Orientation::NoTransforms,
            DecodeBound::within(256, 256),
        )
    };
    let over_budget = decode_budget::BUDGET_BYTES + 1;
    assert_eq!(
        decode_route(
            Some(ImageFormat::Jpeg),
            &thumbnail((12000, 9000)),
            over_budget
        ),
        DecodeRoute::ShrinkJpeg
    );
    assert_eq!(
        decode_route(
            Some(ImageFormat::Png),
            &thumbnail((8000, 8000)),
            over_budget
        ),
        DecodeRoute::PngRows
    );
    assert_eq!(
        decode_route(
            Some(ImageFormat::Tiff),
            &thumbnail((4000, 3000)),
            72_000_000
        ),
        DecodeRoute::TiffRows
    );
    assert_eq!(
        decode_route(Some(ImageFormat::Tiff), &thumbnail((200, 100)), 60_000),
        DecodeRoute::Whole
    );
    assert_eq!(
        decode_route(Some(ImageFormat::Png), &thumbnail((1920, 1080)), 8_294_400),
        DecodeRoute::Whole
    );
    // 放大预览要原图：缩不了，整解。
    let full = ShrinkPlan::new((12000, 9000), Orientation::NoTransforms, DecodeBound::FULL);
    assert_eq!(
        decode_route(Some(ImageFormat::Jpeg), &full, over_budget),
        DecodeRoute::Whole
    );
    assert_eq!(
        decode_route(Some(ImageFormat::Png), &full, over_budget),
        DecodeRoute::Whole
    );
}

/// 透明像素不参与颜色平均：红与全透明各一半，平均出来是半透明的红，不是暗红。
#[test]
fn shrinking_weights_colour_by_opacity() {
    let mut shrink = area_shrink::AreaShrink::new((2, 1), (1, 1));
    shrink.push_row(&[255, 0, 0, 255, 0, 0, 0, 0]);
    assert_eq!(shrink.finish().get_pixel(0, 0).0, [255, 0, 0, 128]);
}

/// 整图解码共用的预算占满时，别的解码等前面的还回来。
#[test]
fn whole_decodes_wait_for_room_in_the_shared_budget() {
    let full = decode_budget::reserve(decode_budget::BUDGET_BYTES);
    let reserved = Arc::new(AtomicBool::new(false));
    let waiter = std::thread::spawn({
        let reserved = Arc::clone(&reserved);
        move || {
            let _small = decode_budget::reserve(1);
            reserved.store(true, Ordering::SeqCst);
        }
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(!reserved.load(Ordering::SeqCst));
    drop(full);
    waiter.join().expect("waiter");
    assert!(reserved.load(Ordering::SeqCst));
}
