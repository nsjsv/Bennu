use iced::{Point, Size};

use super::{TileLevels, TileRange, TileView};

/// 1.08 亿像素的照片：12000×9000，一半那级 6000×4500。
const PHOTO: TileLevels = TileLevels {
    dimensions: [(12000, 9000), (6000, 4500)],
};

/// 1000×750 的媒体区，图适应窗口时正好占满。
fn view(scale: f32, offset: Point) -> TileView {
    TileView {
        panel: Size::new(1000.0, 750.0),
        fit: Size::new(1000.0, 750.0),
        scale,
        offset,
        display_scale: 1.0,
    }
}

fn range(level: usize, columns: std::ops::Range<u32>, rows: std::ops::Range<u32>) -> TileRange {
    TileRange {
        level,
        columns,
        rows,
    }
}

/// 适应窗口时一个原图像素远不到半个屏幕像素：用一半那级，整张都看得见。
#[test]
fn a_fitted_picture_uses_the_half_level_and_every_tile() {
    assert_eq!(
        view(1.0, Point::ORIGIN).range(&PHOTO, 0),
        Some(range(1, 0..24, 0..18))
    );
}

/// 1:1 时用原尺寸那级，只要窗口那么大的一片（加预取的一圈），不是整张。
#[test]
fn actual_size_needs_only_the_tiles_under_the_window() {
    // 1:1：放大 12 倍，图中心对着窗口中心 → 看到原图 (5500..6500, 4125..4875)。
    let actual = view(12.0, Point::ORIGIN);

    assert_eq!(actual.range(&PHOTO, 0), Some(range(0, 21..26, 16..20)));
    assert_eq!(actual.range(&PHOTO, 1), Some(range(0, 20..27, 15..21)));
}

/// 平移到左上角：块号从 0 开始，预取不会算出负数。
#[test]
fn panning_to_the_corner_clamps_the_tile_range() {
    // 图左上角对齐窗口左上角时，图中心在窗口中心右下方 (12000/2 - 500, 9000/2 - 375)。
    let corner = view(12.0, Point::new(5500.0, 4125.0));

    assert_eq!(corner.range(&PHOTO, 2), Some(range(0, 0..6, 0..5)));
}

#[test]
fn a_picture_dragged_out_of_the_window_needs_no_tiles() {
    assert_eq!(view(12.0, Point::new(9000.0, 0.0)).range(&PHOTO, 1), None);
}

/// 缩到一个原图像素不到半个屏幕像素时换一半那级：同样的窗口要的块少四倍。
#[test]
fn the_half_level_takes_over_below_half_size() {
    let at = |scale: f32| {
        view(scale, Point::ORIGIN)
            .range(&PHOTO, 0)
            .map(|range| range.level)
    };

    assert_eq!(at(5.9), Some(1));
    assert_eq!(at(6.1), Some(0));
}
