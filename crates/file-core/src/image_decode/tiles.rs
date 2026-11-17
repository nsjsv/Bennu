//! 超大图放大时不整张解：读一遍原图，切成 256×256 的块存进临时文件，显示时只读看得见的块。
//! 1.08 亿像素的照片整张解要 432 MB，交给显卡又是一份；切块后内存只跟窗口大小有关。
//!
//! 块按文件里存的方向写（顺序写、宽高不是块大小的倍数也不用对齐），读的时候按显示方向要一块，
//! 换算回存储图上的区域，从一到四个存储块里拼出来再转正。同时写两级：原尺寸和每边一半，缩到一半
//! 以下时用一半那级，不用读四倍的块。

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use image::metadata::Orientation;
use image::{ImageDecoder, ImageFormat, RgbaImage};
use tokio_util::sync::CancellationToken;

use super::row_reader::{self, RowReader, BAND_ROWS};
use super::{guessed_reader, oriented_dimensions, scaled_jpeg, ImageDecodeError};
use crate::decode_budget;

pub const TILE_SIZE: u32 = 256;
const TILE: usize = TILE_SIZE as usize;
const TILE_BYTES: u64 = (TILE * TILE * 4) as u64;
/// 整张解码超过这么大才切块（用户定的门槛）；小的照旧整张解，快也简单。
pub const TILING_THRESHOLD_BYTES: u64 = 64 << 20;
/// 切完还要给磁盘留这么多：块文件有原图像素那么大（1.08 亿像素约 540 MB）。
const FREE_SPACE_MARGIN_BYTES: u64 = 512 << 20;

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// 整张解码会不会超过门槛（转正前后像素数一样）。
pub fn needs_tiling(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) * 4 > TILING_THRESHOLD_BYTES
}

pub struct TiledImage {
    file: File,
    path: PathBuf,
    orientation: Orientation,
    /// 每级在存储方向上的大小和在文件里的起点：0 级原尺寸，1 级每边一半。
    levels: [Level; 2],
}

#[derive(Debug, Clone, Copy)]
struct Level {
    stored: (u32, u32),
    offset: u64,
}

impl Level {
    fn columns(self) -> u32 {
        self.stored.0.div_ceil(TILE_SIZE)
    }

    fn tile_offset(self, column: u32, row: u32) -> u64 {
        self.offset + (u64::from(row) * u64::from(self.columns()) + u64::from(column)) * TILE_BYTES
    }

    fn bytes(self) -> u64 {
        u64::from(self.columns()) * u64::from(self.stored.1.div_ceil(TILE_SIZE)) * TILE_BYTES
    }
}

impl TiledImage {
    /// 读一遍原图切块，块文件放在缓存目录。不能逐行读的格式和写法（WebP、CMYK JPEG、隔行 PNG…）、
    /// 磁盘空间不够、中途取消，返回 `None`：调用方停在屏幕大小那张。
    pub fn build(
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<Self>, ImageDecodeError> {
        let Some(format) = guessed_reader(path)?.and_then(|reader| reader.format()) else {
            return Ok(None);
        };
        let orientation = stored_orientation(path, format)?;
        let Some(reader) = row_reader::open(path, format)? else {
            return Ok(None);
        };
        let directory = crate::app_dirs::app_cache_dir().join("zoom-tiles");
        Self::build_in(&directory, reader, orientation, cancellation)
    }

    pub(super) fn build_in(
        directory: &Path,
        mut reader: Box<dyn RowReader>,
        orientation: Orientation,
        cancellation: &CancellationToken,
    ) -> Result<Option<Self>, ImageDecodeError> {
        let stored = reader.dimensions();
        let full = Level { stored, offset: 0 };
        let half = Level {
            stored: (stored.0.div_ceil(2), stored.1.div_ceil(2)),
            offset: full.bytes(),
        };
        fs::create_dir_all(directory)?;
        remove_files_of_exited_processes(directory);
        if available_bytes(directory)? < full.bytes() + half.bytes() + FREE_SPACE_MARGIN_BYTES {
            return Ok(None);
        }
        let path = directory.join(format!(
            "{}-{}.tiles",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        // 先放进会在 Drop 时删文件的结构里：切到一半出错、取消也不留下半个文件。
        let tiled = Self {
            file,
            path,
            orientation,
            levels: [full, half],
        };
        let finished =
            TileWriter::new(&tiled.file, full, half).write(reader.as_mut(), cancellation)?;
        Ok(finished.then_some(tiled))
    }

    /// 转正后的原图大小。
    pub fn dimensions(&self) -> (u32, u32) {
        self.level_dimensions(0)
    }

    /// 第 `level` 级（0 原尺寸，1 每边一半）转正后的大小。
    pub fn level_dimensions(&self, level: usize) -> (u32, u32) {
        oriented_dimensions(self.levels[level].stored, self.orientation)
    }

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// 第 `level` 级转正后有几列、几行块。
    pub fn tile_grid(&self, level: usize) -> (u32, u32) {
        let (width, height) = self.level_dimensions(level);
        (width.div_ceil(TILE_SIZE), height.div_ceil(TILE_SIZE))
    }

    /// 第 `level` 级转正后第 `column` 列、第 `row` 行的块（在 [`Self::tile_grid`] 之内）；
    /// 右边和底边的块小一些。
    pub fn tile(&self, level: usize, column: u32, row: u32) -> Result<RgbaImage, ImageDecodeError> {
        let layout = self.levels[level];
        let (display_width, display_height) = self.level_dimensions(level);
        let left = column * TILE_SIZE;
        let top = row * TILE_SIZE;
        let width = TILE_SIZE.min(display_width - left);
        let height = TILE_SIZE.min(display_height - top);
        let map = StoredPosition::new(self.orientation, layout.stored);
        let mut source = TileCache::default();
        let mut tile = RgbaImage::new(width, height);
        for (x, y, pixel) in tile.enumerate_pixels_mut() {
            let (stored_x, stored_y) = map.of(left + x, top + y);
            let block = source.get(
                &self.file,
                layout,
                stored_x / TILE_SIZE,
                stored_y / TILE_SIZE,
            )?;
            let at = ((stored_y % TILE_SIZE) as usize * TILE + (stored_x % TILE_SIZE) as usize) * 4;
            pixel.0.copy_from_slice(&block[at..at + 4]);
        }
        Ok(tile)
    }
}

impl Drop for TiledImage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 拼一个显示块时读过的存储块（最多四个），按块坐标留着，同一块不重复读。
#[derive(Default)]
struct TileCache {
    blocks: Vec<((u32, u32), Vec<u8>)>,
}

impl TileCache {
    fn get(
        &mut self,
        file: &File,
        level: Level,
        column: u32,
        row: u32,
    ) -> Result<&[u8], ImageDecodeError> {
        let position = match self
            .blocks
            .iter()
            .position(|(key, _)| *key == (column, row))
        {
            Some(position) => position,
            None => {
                let mut block = vec![0; TILE_BYTES as usize];
                file.read_exact_at(&mut block, level.tile_offset(column, row))?;
                self.blocks.push(((column, row), block));
                self.blocks.len() - 1
            }
        };
        Ok(&self.blocks[position].1)
    }
}

/// 显示方向上的像素在存储图上的位置，和 image 库 `apply_orientation` 的转法互逆。
struct StoredPosition {
    orientation: Orientation,
    last_x: u32,
    last_y: u32,
}

impl StoredPosition {
    fn new(orientation: Orientation, (width, height): (u32, u32)) -> Self {
        Self {
            orientation,
            last_x: width - 1,
            last_y: height - 1,
        }
    }

    fn of(&self, x: u32, y: u32) -> (u32, u32) {
        match self.orientation {
            Orientation::NoTransforms => (x, y),
            Orientation::Rotate90 => (y, self.last_y - x),
            Orientation::Rotate180 => (self.last_x - x, self.last_y - y),
            Orientation::Rotate270 => (self.last_x - y, x),
            Orientation::FlipHorizontal => (self.last_x - x, y),
            Orientation::FlipVertical => (x, self.last_y - y),
            Orientation::Rotate90FlipH => (y, x),
            Orientation::Rotate270FlipH => (self.last_x - y, self.last_y - x),
        }
    }
}

/// 一遍写完两级块：攒满一排块高的行就切成块顺序写出；每两行平均成一半那级的一行，攒满也写出。
struct TileWriter<'a> {
    file: &'a File,
    full: BandWriter,
    half: BandWriter,
}

impl<'a> TileWriter<'a> {
    fn new(file: &'a File, full: Level, half: Level) -> Self {
        Self {
            file,
            full: BandWriter::new(full),
            half: BandWriter::new(half),
        }
    }

    /// 写完返回 true；取消返回 false。
    fn write(
        mut self,
        reader: &mut dyn RowReader,
        cancellation: &CancellationToken,
    ) -> Result<bool, ImageDecodeError> {
        // 两条带加读行的缓冲按预算申请，和缩略图共用一份预算。
        let _budget = decode_budget::reserve(
            (self.full.band.len() + self.half.band.len()) as u64
                + u64::from(self.full.level.stored.0) * 4 * BAND_ROWS as u64,
        );
        let row_bytes = self.full.level.stored.0 as usize * 4;
        let mut rows = vec![0; row_bytes * BAND_ROWS];
        // 一半那级每次要上下两行：上一行先留着，等下一行来了一起平均。
        let mut upper = vec![0; row_bytes];
        let mut has_upper = false;
        let mut half = vec![0; self.half.level.stored.0 as usize * 4];
        loop {
            if cancellation.is_cancelled() {
                return Ok(false);
            }
            let read = reader.read_rows(&mut rows)?;
            if read == 0 {
                break;
            }
            for row in rows[..read * row_bytes].chunks_exact(row_bytes) {
                self.full.push_row(self.file, row)?;
                if has_upper {
                    half_row(&upper, Some(row), &mut half);
                    self.half.push_row(self.file, &half)?;
                } else {
                    upper.copy_from_slice(row);
                }
                has_upper = !has_upper;
            }
        }
        if has_upper {
            half_row(&upper, None, &mut half);
            self.half.push_row(self.file, &half)?;
        }
        self.full.flush(self.file)?;
        self.half.flush(self.file)?;
        Ok(true)
    }
}

/// 一级的一排块：攒行、满了切块写出。
struct BandWriter {
    level: Level,
    band: Vec<u8>,
    rows: usize,
    tile_row: u32,
}

impl BandWriter {
    fn new(level: Level) -> Self {
        Self {
            level,
            band: vec![0; level.stored.0 as usize * 4 * TILE],
            rows: 0,
            tile_row: 0,
        }
    }

    fn push_row(&mut self, file: &File, row: &[u8]) -> std::io::Result<()> {
        let row_bytes = self.level.stored.0 as usize * 4;
        self.band[self.rows * row_bytes..][..row_bytes].copy_from_slice(row);
        self.rows += 1;
        if self.rows == TILE {
            self.flush(file)?;
        }
        Ok(())
    }

    /// 把攒着的行切块写出（最后一排可能不满）；块里没有图的部分留着不管，读的时候用不到。
    fn flush(&mut self, file: &File) -> std::io::Result<()> {
        if self.rows == 0 {
            return Ok(());
        }
        let row_bytes = self.level.stored.0 as usize * 4;
        let mut block = vec![0; TILE_BYTES as usize];
        for column in 0..self.level.columns() {
            let left = column as usize * TILE * 4;
            let width = (row_bytes - left).min(TILE * 4);
            for y in 0..self.rows {
                block[y * TILE * 4..][..width]
                    .copy_from_slice(&self.band[y * row_bytes + left..][..width]);
            }
            file.write_all_at(&block, self.level.tile_offset(column, self.tile_row))?;
        }
        self.tile_row += 1;
        self.rows = 0;
        Ok(())
    }
}

/// 上下两行（最后一行可能落单）每 2×2 平均成一行写进 `half`，颜色按不透明度加权（透明的边
/// 不会把颜色带黑）。
fn half_row(upper: &[u8], lower: Option<&[u8]>, half: &mut [u8]) {
    let rows = [Some(upper), lower];
    for (x, pixel) in half.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mut sums = [0u32; 5];
        for source in rows.into_iter().flatten() {
            let pixels = source.as_chunks::<4>().0;
            for &[red, green, blue, alpha] in pixels[2 * x..].iter().take(2) {
                let alpha_weight = u32::from(alpha);
                sums[0] += u32::from(red) * alpha_weight;
                sums[1] += u32::from(green) * alpha_weight;
                sums[2] += u32::from(blue) * alpha_weight;
                sums[3] += alpha_weight;
                sums[4] += 1;
            }
        }
        let [red, green, blue, alpha, count] = sums;
        let color = |channel: u32| {
            (channel + alpha / 2)
                .checked_div(alpha)
                .map_or(0, |value| value as u8)
        };
        *pixel = [
            color(red),
            color(green),
            color(blue),
            ((alpha + count / 2) / count) as u8,
        ];
    }
}

/// 文件里记的方向。JPEG 用缩小解码那边读文件头（image 的 JPEG 解码器一建就把整个文件读进内存）。
fn stored_orientation(path: &Path, format: ImageFormat) -> Result<Orientation, ImageDecodeError> {
    if format == ImageFormat::Jpeg {
        return Ok(scaled_jpeg::read_header(path)
            .map_or(Orientation::NoTransforms, |header| header.orientation()));
    }
    let Some(reader) = guessed_reader(path)? else {
        return Ok(Orientation::NoTransforms);
    };
    let mut decoder = reader.into_decoder()?;
    Ok(decoder.orientation().unwrap_or(Orientation::NoTransforms))
}

fn available_bytes(directory: &Path) -> std::io::Result<u64> {
    let stats = rustix::fs::statvfs(directory)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

/// 上次崩溃、被杀留下的块文件（文件名开头是进程号、那个进程已经不在）删掉。
fn remove_files_of_exited_processes(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.split('-').next()) else {
            continue;
        };
        if pid.parse::<u32>().is_ok() && !Path::new("/proc").join(pid).exists() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
#[path = "tiles_tests.rs"]
mod tests;
