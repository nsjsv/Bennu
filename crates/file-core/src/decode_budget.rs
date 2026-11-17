//! 同时在解的图最多占这么多内存：整图解码的像素，加上解码用的 ffmpeg 进程。缩略图几路并行、
//! 预览也在解时，几张大图一起整解、几个 ffmpeg 一起跑会把内存顶到几百 MB；占不到位置的等前面
//! 的解完。

use std::sync::{Condvar, Mutex};

/// 共享的上限。缩小解码（JPEG）和逐行读的 PNG 用不到整图，不占这里。
pub const BUDGET_BYTES: u64 = 64 << 20;

/// 一个解码用的外部进程（ffmpeg、ffmpegthumbnailer）按这么多算：同时只跑一个，旁边还能整解
/// 16 MB 以内的图。单线程的 ffmpeg 出一张 1080p 视频的缩略图实测私有内存 43 MB，
/// ffmpegthumbnailer 30 MB。
pub const DECODER_PROCESS_BYTES: u64 = 48 << 20;

static IN_USE: Mutex<u64> = Mutex::new(0);
static RELEASED: Condvar = Condvar::new();

/// 占着的这部分预算，丢掉时还回去。
pub struct Reservation(u64);

/// 占 `bytes`；比整个预算还大的（放大预览时解原图、没法边读边缩的大图）占满整个预算，独自解。
pub fn reserve(bytes: u64) -> Reservation {
    let bytes = bytes.min(BUDGET_BYTES);
    let mut in_use = IN_USE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *in_use + bytes > BUDGET_BYTES {
        in_use = RELEASED
            .wait(in_use)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    *in_use += bytes;
    Reservation(bytes)
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut in_use = IN_USE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *in_use -= self.0;
        RELEASED.notify_all();
    }
}
