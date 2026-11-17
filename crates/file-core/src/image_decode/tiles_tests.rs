use image::metadata::Orientation;
use image::{DynamicImage, RgbaImage};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::image_decode::row_reader::RowReader;

/// 测试用：一行行交出内存里的一张图。
struct ImageRows {
    image: RgbaImage,
    next_row: u32,
}

impl RowReader for ImageRows {
    fn dimensions(&self) -> (u32, u32) {
        self.image.dimensions()
    }

    fn read_rows(&mut self, rgba: &mut [u8]) -> Result<usize, ImageDecodeError> {
        let row_bytes = self.image.width() as usize * 4;
        let mut rows = 0;
        for slot in rgba.chunks_exact_mut(row_bytes) {
            if self.next_row == self.image.height() {
                break;
            }
            slot.copy_from_slice(
                &self.image.as_raw()[self.next_row as usize * row_bytes..][..row_bytes],
            );
            self.next_row += 1;
            rows += 1;
        }
        Ok(rows)
    }
}

/// 每个像素的颜色由坐标决定、互不相同，拼错一个都看得出来。
fn coordinates_picture(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            (x & 0xff) as u8,
            (y & 0xff) as u8,
            ((x >> 8) | (y >> 8 << 4)) as u8,
            255,
        ])
    })
}

fn build(directory: &Path, image: RgbaImage, orientation: Orientation) -> TiledImage {
    TiledImage::build_in(
        directory,
        Box::new(ImageRows { image, next_row: 0 }),
        orientation,
        &CancellationToken::new(),
    )
    .expect("build")
    .expect("enough space, not cancelled")
}

/// 一级的所有块按位置拼回整张。
fn assemble(tiled: &TiledImage, level: usize) -> RgbaImage {
    let (width, height) = tiled.level_dimensions(level);
    let (columns, rows) = tiled.tile_grid(level);
    let mut whole = RgbaImage::new(width, height);
    for row in 0..rows {
        for column in 0..columns {
            let tile = tiled.tile(level, column, row).expect("tile");
            image::imageops::replace(
                &mut whole,
                &tile,
                i64::from(column * TILE_SIZE),
                i64::from(row * TILE_SIZE),
            );
        }
    }
    whole
}

/// 块按存储方向写、按显示方向读：八种 EXIF 方向拼回来都和整张转正一样（宽高不是块的倍数）。
#[test]
fn tiles_read_back_upright_for_every_orientation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let picture = coordinates_picture(300, 170);
    for orientation in [
        Orientation::NoTransforms,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipHorizontal,
        Orientation::FlipVertical,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ] {
        let tiled = build(directory.path(), picture.clone(), orientation);
        let mut upright = DynamicImage::ImageRgba8(picture.clone());
        upright.apply_orientation(orientation);

        assert_eq!(
            tiled.dimensions(),
            (upright.width(), upright.height()),
            "{orientation:?}"
        );
        assert!(
            assemble(&tiled, 0) == upright.into_rgba8(),
            "{orientation:?}"
        );
    }
}

/// 一半那级：每边一半（奇数向上取整），每个像素是 2×2 的平均，颜色按不透明度加权。
#[test]
fn the_half_level_averages_two_by_two_weighted_by_opacity() {
    let directory = tempfile::tempdir().expect("tempdir");
    let picture = RgbaImage::from_fn(5, 3, |x, y| match (x, y) {
        (0, 0) => image::Rgba([200, 0, 0, 255]),
        (1, 0) => image::Rgba([0, 0, 0, 0]),
        (0, 1) => image::Rgba([100, 0, 0, 255]),
        (1, 1) => image::Rgba([0, 0, 0, 0]),
        _ => image::Rgba([10, 20, 30, 255]),
    });
    let tiled = build(directory.path(), picture, Orientation::NoTransforms);

    let half = assemble(&tiled, 1);

    assert_eq!(half.dimensions(), (3, 2));
    // 透明像素不参与颜色：红是 (200 + 100) / 2，不透明度是四个的平均。
    assert_eq!(half.get_pixel(0, 0).0, [150, 0, 0, 128]);
    // 右下角落单的一个像素原样留下。
    assert_eq!(half.get_pixel(2, 1).0, [10, 20, 30, 255]);
}

#[test]
fn the_tile_file_goes_away_with_the_tiled_image() {
    let directory = tempfile::tempdir().expect("tempdir");
    let tiled = build(
        directory.path(),
        coordinates_picture(40, 30),
        Orientation::NoTransforms,
    );
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list").count(),
        1
    );

    drop(tiled);

    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list").count(),
        0
    );
}

#[test]
fn cancelling_leaves_no_half_written_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let tiled = TiledImage::build_in(
        directory.path(),
        Box::new(ImageRows {
            image: coordinates_picture(40, 30),
            next_row: 0,
        }),
        Orientation::NoTransforms,
        &cancellation,
    )
    .expect("build");

    assert!(tiled.is_none());
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list").count(),
        0
    );
}

/// 崩溃、被杀的进程留下的块文件（文件名开头的进程号已经不在）下次切块时删掉，自己的不动。
#[test]
fn files_left_by_exited_processes_are_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let stale = directory.path().join("4294967294-0.tiles");
    std::fs::write(&stale, b"left over").expect("write");

    let _tiled = build(
        directory.path(),
        coordinates_picture(40, 30),
        Orientation::NoTransforms,
    );

    assert!(!stale.exists());
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list").count(),
        1
    );
}

/// 用户定的门槛：整张解码超过 64 MB 才切块。
#[test]
fn only_pictures_over_sixty_four_megabytes_are_tiled() {
    assert!(!needs_tiling(4096, 4096));
    assert!(needs_tiling(4097, 4096));
    assert!(needs_tiling(12000, 9000));
}

/// 真实 JPEG 经系统 libjpeg-turbo 逐行读、切块，拼回来和整张解码的一样（两边舍入差几个色阶）。
#[test]
fn a_jpeg_is_tiled_from_its_rows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("photo.jpg");
    let picture = image::RgbImage::from_fn(520, 300, |x, y| {
        image::Rgb([x as u8, y as u8, (x / 3) as u8])
    });
    picture.save(&path).expect("save jpeg");
    let reader = super::row_reader::open(&path, ImageFormat::Jpeg)
        .expect("open")
        .expect("baseline jpeg reads row by row");
    let tiles = directory.path().join("tiles");

    let tiled = TiledImage::build_in(
        &tiles,
        reader,
        Orientation::NoTransforms,
        &CancellationToken::new(),
    )
    .expect("build")
    .expect("tiled");

    let whole = image::open(&path).expect("decode").into_rgba8();
    let assembled = assemble(&tiled, 0);
    let largest = assembled
        .as_raw()
        .iter()
        .zip(whole.as_raw())
        .map(|(left, right)| left.abs_diff(*right))
        .max();
    assert!(largest <= Some(8), "{largest:?}");
}
