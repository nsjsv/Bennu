//! 超大图放大后的切块显示（切块存储见 `file_core::image_decode::tiles`）：算视口里看得见哪些块、
//! 读进来、离开视口的丢掉，把读进来的块摆到放大后的图上。只在放大到超过按屏幕大小解的那张时用；
//! 内存只跟窗口大小有关（1:1 时 2000×1200 的窗口约 54 块 × 256 KB）。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use file_core::image_decode::tiles::{TiledImage, TILE_SIZE};
use iced::widget::image::{self, FilterMethod};
use iced::{Element, Length, Point, Size, Vector};

use bennu_theme::translated_surface::translated_surface;

/// 视口外再多读一圈：平移时边上的块已经在了。
const PREFETCH_TILES: u32 = 1;
/// 离开视口超过这么多圈的块丢掉（比预取多一圈，来回小幅平移不反复读）。
const KEEP_TILES: u32 = 2;

/// 切块存储，多处共享（后台读块的任务也拿着它）；最后一份放掉时删块文件。
#[derive(Clone)]
pub struct SharedTiledImage(pub Arc<TiledImage>);

impl fmt::Debug for SharedTiledImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedTiledImage")
            .field("dimensions", &self.0.dimensions())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub level: usize,
    pub column: u32,
    pub row: u32,
}

#[derive(Debug, Clone)]
pub enum ZoomTilesMessage {
    /// 切完了；`None` 是切不了（格式不能逐行读、磁盘不够），停在屏幕大小那张。
    Built {
        generation: u64,
        outcome: Result<Option<SharedTiledImage>, String>,
    },
    /// 读回来的块；读坏的是 `None`（记过日志，不再等它）。
    Loaded {
        generation: u64,
        tiles: Vec<(TileKey, Option<image::Handle>)>,
    },
}

/// 一张图的切块显示：存储、读进来的块、正在读的块。
#[derive(Debug)]
pub struct ZoomTiles {
    pub(crate) image: SharedTiledImage,
    levels: TileLevels,
    pub(crate) loaded: HashMap<TileKey, image::Handle>,
    pub(crate) in_flight: HashSet<TileKey>,
}

impl ZoomTiles {
    pub(crate) fn new(image: SharedTiledImage) -> Self {
        let levels = TileLevels {
            dimensions: [image.0.level_dimensions(0), image.0.level_dimensions(1)],
        };
        Self {
            image,
            levels,
            loaded: HashMap::new(),
            in_flight: HashSet::new(),
        }
    }

    /// 要读的块（预取范围里还没有、也没在读的），记成正在读。
    pub(crate) fn take_missing(&mut self, view: &TileView) -> Vec<TileKey> {
        let Some(range) = view.range(&self.levels, PREFETCH_TILES) else {
            return Vec::new();
        };
        let missing: Vec<TileKey> = range
            .keys()
            .filter(|key| !self.loaded.contains_key(key) && !self.in_flight.contains(key))
            .collect();
        self.in_flight.extend(missing.iter().copied());
        missing
    }

    /// 丢掉离开视口太远、或不是当前这一级的块。
    pub(crate) fn evict_outside(&mut self, view: &TileView) {
        match view.range(&self.levels, KEEP_TILES) {
            Some(keep) => self.loaded.retain(|key, _| keep.contains(*key)),
            None => self.loaded.clear(),
        }
    }
}

/// 两级（原尺寸、每边一半）转正后的大小。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileLevels {
    pub(crate) dimensions: [(u32, u32); 2],
}

impl TileLevels {
    fn grid(&self, level: usize) -> (u32, u32) {
        let (width, height) = self.dimensions[level];
        (width.div_ceil(TILE_SIZE), height.div_ceil(TILE_SIZE))
    }
}

/// 算看得见哪些块要的几何：和面板摆放图片用的是同一套（见 `image_preview_panel`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TileView {
    /// 媒体区大小（逻辑像素）。
    pub(crate) panel: Size,
    /// 适应窗口时图片的大小。
    pub(crate) fit: Size,
    pub(crate) scale: f32,
    /// 图片中心相对媒体区中心的偏移。
    pub(crate) offset: Point,
    /// 逻辑像素换物理像素的倍数。
    pub(crate) display_scale: f32,
}

/// 一级里的一片块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TileRange {
    level: usize,
    columns: Range<u32>,
    rows: Range<u32>,
}

impl TileRange {
    pub(crate) fn keys(&self) -> impl Iterator<Item = TileKey> + '_ {
        self.rows.clone().flat_map(move |row| {
            self.columns.clone().map(move |column| TileKey {
                level: self.level,
                column,
                row,
            })
        })
    }

    fn contains(&self, key: TileKey) -> bool {
        key.level == self.level
            && self.columns.contains(&key.column)
            && self.rows.contains(&key.row)
    }
}

impl TileView {
    fn scaled(&self) -> Size {
        Size::new(self.fit.width * self.scale, self.fit.height * self.scale)
    }

    /// 放大后的图左上角在媒体区里的位置。
    fn image_origin(&self) -> Point {
        let scaled = self.scaled();
        Point::new(
            self.panel.width / 2.0 + self.offset.x - scaled.width / 2.0,
            self.panel.height / 2.0 + self.offset.y - scaled.height / 2.0,
        )
    }

    /// 用哪一级：一个原图像素显示不到半个屏幕像素时用一半那级，不用读四倍的块。
    fn level(&self, levels: &TileLevels) -> usize {
        let shown = self.scaled().width * self.display_scale;
        usize::from(shown < levels.dimensions[0].0 as f32 * 0.5)
    }

    /// 视口里看得见的块，外加 `margin` 圈；图完全在视口外时没有。
    pub(crate) fn range(&self, levels: &TileLevels, margin: u32) -> Option<TileRange> {
        let level = self.level(levels);
        let (level_width, level_height) = levels.dimensions[level];
        let (columns, rows) = levels.grid(level);
        let origin = self.image_origin();
        let scaled = self.scaled();
        let visible_left = origin.x.max(0.0);
        let visible_top = origin.y.max(0.0);
        let visible_right = (origin.x + scaled.width).min(self.panel.width);
        let visible_bottom = (origin.y + scaled.height).min(self.panel.height);
        if visible_right <= visible_left || visible_bottom <= visible_top {
            return None;
        }
        // 逻辑像素换成这一级的像素，再换成块号。
        let per_logical_x = level_width as f32 / scaled.width;
        let per_logical_y = level_height as f32 / scaled.height;
        let tile = TILE_SIZE as f32;
        let span = |from: f32, to: f32, per: f32, count: u32| {
            let first = ((from * per) / tile).floor().max(0.0) as u32;
            let last = ((to * per) / tile).ceil().max(0.0) as u32;
            first.saturating_sub(margin)..(last + margin).min(count)
        };
        Some(TileRange {
            level,
            columns: span(
                visible_left - origin.x,
                visible_right - origin.x,
                per_logical_x,
                columns,
            ),
            rows: span(
                visible_top - origin.y,
                visible_bottom - origin.y,
                per_logical_y,
                rows,
            ),
        })
    }
}

/// 读进来的、视口里看得见的块，摆在放大后的图上（大小是放大后的图）。块的边界按同一套取整，
/// 相邻两块正好接上，不留缝。
pub(crate) fn tile_layer<Message>(
    tiles: &ZoomTiles,
    view: &TileView,
    filter: FilterMethod,
) -> Option<Element<'static, Message>>
where
    Message: 'static + Clone,
{
    let range = view.range(&tiles.levels, 0)?;
    let (level_width, level_height) = tiles.levels.dimensions[range.level];
    let scaled = view.scaled();
    let x_of =
        |pixel: u32| (pixel.min(level_width) as f32 * scaled.width / level_width as f32).round();
    let y_of =
        |pixel: u32| (pixel.min(level_height) as f32 * scaled.height / level_height as f32).round();
    let mut layer = iced::widget::Stack::new()
        .width(Length::Fixed(scaled.width))
        .height(Length::Fixed(scaled.height));
    for key in range.keys() {
        let Some(handle) = tiles.loaded.get(&key) else {
            continue;
        };
        let (left, top) = (x_of(key.column * TILE_SIZE), y_of(key.row * TILE_SIZE));
        let (right, bottom) = (
            x_of((key.column + 1) * TILE_SIZE),
            y_of((key.row + 1) * TILE_SIZE),
        );
        let tile = image::Image::new(handle.clone())
            .width(Length::Fixed(right - left))
            .height(Length::Fixed(bottom - top))
            .content_fit(iced::ContentFit::Fill)
            .filter_method(filter);
        layer = layer.push(translated_surface(tile, Vector::new(left, top)));
    }
    Some(layer.into())
}

#[cfg(test)]
#[path = "zoom_tiles_tests.rs"]
mod tests;
