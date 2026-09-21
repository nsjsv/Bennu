use std::time::{Duration, Instant};

// 时间进度/缓动算法已下沉 bennu-theme 共享（portal FileChooser 复用）；
// re-export 保持 crate 内引用路径不变。elapsed_fraction 是“取当前时刻”
// 的便捷封装，只被主程序动画调用点使用，随留本地。
pub(crate) use bennu_theme::animation::{ease_out_cubic, elapsed_fraction_at};

pub(crate) fn elapsed_fraction(started_at: Instant, duration: Duration) -> f32 {
    elapsed_fraction_at(started_at, Instant::now(), duration)
}
