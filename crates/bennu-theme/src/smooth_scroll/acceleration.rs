//! 滚轮按转速加速：转得快一格走得远，慢慢拨一格还是一格。
//!
//! 速度项照搬 macOS IOHIPointing：最近 8 次滚轮的平均间隔 t（毫秒），`A·t² − B·t + C`，
//! t ≥ 150 ms 时约为 0，越快越接近 1.5；再乘最近几次的平均格数。那边拿它去查设备自己的加速
//! 曲线，这里没有设备曲线，直接换成倍数 `1 + GAIN × 速度项`：慢拨 1 倍，最快约 3.4 倍。

use std::time::Instant;

use super::SmoothScrollDelta;

/// 记最近几次滚轮的间隔。
const HISTORY: usize = 8;
/// 间隔到这么长算慢拨，不加速；更早的不再看。
const SLOW_GAP_MS: f32 = 150.0;
/// 停这么久再拨，或者换了方向，从头算。
const CLEAR_GAP_MS: f32 = 500.0;
/// 速度项的系数（原始定点数 / 65536）。
const A: f32 = 2.0 / 65536.0;
const B: f32 = 955.0 / 65536.0;
const C: f32 = 98369.0 / 65536.0;
/// 速度项换成倍数的增益。
const GAIN: f32 = 1.6;
const MAX_MULTIPLIER: f32 = 1.0 + GAIN * C;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Notch {
    /// 和上一次的间隔（毫秒）。
    gap_ms: f32,
    /// 这次拨了几格。
    notches: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WheelAcceleration {
    /// 新的在前。
    recent: Vec<Notch>,
    last: Option<(Instant, Direction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Direction {
    x: i8,
    y: i8,
}

impl Direction {
    fn of(delta: SmoothScrollDelta) -> Self {
        let sign = |value: f32| (value > 0.0) as i8 - (value < 0.0) as i8;
        Self {
            x: sign(delta.x),
            y: sign(delta.y),
        }
    }
}

impl WheelAcceleration {
    /// 这次拨动（`notches` 格）的距离该乘几倍。
    pub fn multiplier(&mut self, delta: SmoothScrollDelta, notches: f32, now: Instant) -> f32 {
        let direction = Direction::of(delta);
        let gap_ms = self
            .last
            .replace((now, direction))
            .filter(|(_, previous)| *previous == direction)
            .map(|(at, _)| now.saturating_duration_since(at).as_secs_f32() * 1000.0)
            .filter(|gap| *gap < CLEAR_GAP_MS);
        let Some(gap_ms) = gap_ms else {
            self.recent.clear();
            return 1.0;
        };
        self.recent.insert(0, Notch { gap_ms, notches });
        self.recent.truncate(HISTORY);

        // 从最近的往前加：遇到慢的那次按 150 算、到此为止；加满 500 ms 也到此为止。
        let (mut total_ms, mut total_notches, mut count) = (0.0, 0.0, 0.0);
        for notch in &self.recent {
            count += 1.0;
            total_notches += notch.notches;
            if notch.gap_ms >= SLOW_GAP_MS {
                total_ms += SLOW_GAP_MS;
                break;
            }
            total_ms += notch.gap_ms;
            if total_ms >= CLEAR_GAP_MS {
                break;
            }
        }
        let t = (total_ms / count).clamp(1.0, SLOW_GAP_MS);
        let speed = (A * t * t - B * t + C).max(0.0) * (total_notches / count);
        (1.0 + GAIN * speed).min(MAX_MULTIPLIER)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const DOWN: SmoothScrollDelta = SmoothScrollDelta { x: 0.0, y: 1.0 };
    const UP: SmoothScrollDelta = SmoothScrollDelta { x: 0.0, y: -1.0 };

    /// 每隔 `gap` 拨一格，拨 `count` 格，返回最后一格的倍数。
    fn spin(gap: Duration, count: u32) -> f32 {
        let start = Instant::now();
        let mut acceleration = WheelAcceleration::default();
        (0..count)
            .map(|index| acceleration.multiplier(DOWN, 1.0, start + gap * index))
            .last()
            .expect("at least one notch")
    }

    #[test]
    fn a_single_or_slow_notch_is_not_accelerated() {
        assert_eq!(spin(Duration::from_millis(400), 1), 1.0);
        assert!((spin(Duration::from_millis(200), 10) - 1.0).abs() < 0.01);
    }

    #[test]
    fn spinning_faster_goes_further_up_to_the_cap() {
        let at_10_per_second = spin(Duration::from_millis(100), 10);
        let at_20_per_second = spin(Duration::from_millis(50), 10);
        let at_50_per_second = spin(Duration::from_millis(20), 10);
        assert!(
            at_10_per_second > 1.3 && at_10_per_second < 1.8,
            "{at_10_per_second}"
        );
        assert!(at_20_per_second > at_10_per_second);
        assert!(at_50_per_second > at_20_per_second);
        assert!(spin(Duration::ZERO, 10) <= MAX_MULTIPLIER);
    }

    /// 换方向、停顿半秒都从 1 倍重新开始，不会一换向就飞出去。
    #[test]
    fn reversing_or_pausing_starts_over() {
        let start = Instant::now();
        let mut acceleration = WheelAcceleration::default();
        for index in 0..8 {
            acceleration.multiplier(DOWN, 1.0, start + Duration::from_millis(20 * index));
        }
        let reversed = start + Duration::from_millis(180);
        assert_eq!(acceleration.multiplier(UP, 1.0, reversed), 1.0);
        let paused = reversed + Duration::from_millis(600);
        assert_eq!(acceleration.multiplier(UP, 1.0, paused), 1.0);
    }
}
