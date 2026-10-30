use std::time::{Duration, Instant};

use bennu_theme::animation::elapsed_fraction_at;

// 共享层的 elapsed_fraction_at 带 now 参数以便测试注入时钟；主程序的
// 动画调用点总是取当前时刻，在这里收口。
pub(crate) fn elapsed_fraction(started_at: Instant, duration: Duration) -> f32 {
    elapsed_fraction_at(started_at, Instant::now(), duration)
}
