//! Mos 式惯性按真实时间推进。
//!
//! 原来的做法每 60 Hz 一帧：剩余距离走掉 12.5%，输出再过权重 0.23 的一阶滤波。换成连续时间
//! 的同一模型：剩余距离 `r` 以 λ 指数衰减，输出速度 `v` 以 μ 一阶跟随 `λ·r`；任意时长的位移
//! 有闭式解，一步算出。帧多帧少、掉不掉帧，同一次拨动走的总距离和时长都一样。

use std::time::{Duration, Instant};

use iced::{window, Vector};

use super::{SmoothScrollDelta, MOS_SCROLL_SPEED};

/// 每 1/60 秒走掉剩余的 12.5%：λ = −ln(1 − 0.125) × 60。
const DECAY_PER_SECOND: f32 = 8.012_08;
/// 每 1/60 秒向目标速度靠 23%：μ = −ln(1 − 0.23) × 60。
const FILTER_PER_SECOND: f32 = 15.681_6;
/// 剩下要走的不足这么多像素就停（原来每帧 0.1 像素的死区）。
const REST_DISTANCE: f32 = 0.1;
/// 每帧最多推进这么久：窗口被挡住、切回来时不一下滚完。
const MAX_STEP_SECONDS: f32 = 0.1;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Axis {
    /// 还要走的距离。
    remaining: f32,
    /// 滤波后的速度（像素/秒）。
    velocity: f32,
}

impl Axis {
    /// 拨一下：同向累加；反向从新的一格重新开始（速度保留，跟原来一样平滑转向）。
    fn push(&mut self, delta: f32) {
        let scaled = delta * MOS_SCROLL_SPEED;
        if scaled * self.remaining > 0.0 {
            self.remaining += scaled;
        } else {
            self.remaining = scaled;
        }
    }

    /// 推进 `dt` 秒，返回这段时间走的距离。
    fn advance(&mut self, dt: f32) -> f32 {
        let (lambda, mu) = (DECAY_PER_SECOND, FILTER_PER_SECOND);
        let decay = (-lambda * dt).exp();
        let filter = (-mu * dt).exp();
        let coupling = mu * lambda * self.remaining / (mu - lambda);
        let moved = self.velocity * (1.0 - filter) / mu
            + coupling * ((1.0 - decay) / lambda - (1.0 - filter) / mu);
        self.velocity = self.velocity * filter + coupling * (decay - filter);
        self.remaining *= decay;
        moved
    }

    /// 之后还会走的距离（剩余的加上滤波里还没放出来的）。
    fn pending(self) -> f32 {
        self.remaining + self.velocity / FILTER_PER_SECOND
    }
}

/// 一个滚动区的惯性：两轴各自推进，最后一次推进的时刻决定下一步走多久。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MosInertia {
    x: Axis,
    y: Axis,
    last_advance: Option<Instant>,
    /// 走过但还没滚出去的不足一像素的部分。
    carry: Vector,
}

/// 一帧该滚的整像素，以及什么时候再来。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InertiaStep {
    pub scroll: Vector,
    /// `None`：惯性停了，不用再画。
    pub next: Option<window::RedrawRequest>,
}

impl MosInertia {
    pub fn push(&mut self, delta: SmoothScrollDelta, now: Instant) {
        self.x.push(delta.x);
        self.y.push(delta.y);
        self.last_advance.get_or_insert(now);
    }

    /// 推进到 `now`：只滚整像素，余下的留到以后；这一帧挪不到一像素就不要下一帧，约在
    /// 攒够一像素的时刻（中间的帧画出来都一样）。
    pub fn step(&mut self, now: Instant) -> InertiaStep {
        let moved = self.advance(now);
        self.carry += Vector::new(moved.x, moved.y);
        let scroll = Vector::new(self.carry.x.trunc(), self.carry.y.trunc());
        self.carry -= scroll;
        let next = if !self.is_active() {
            self.stop();
            None
        } else if scroll == Vector::ZERO {
            Some(window::RedrawRequest::At(now + self.until_next_pixel()))
        } else {
            Some(window::RedrawRequest::NextFrame)
        };
        InertiaStep { scroll, next }
    }

    fn advance(&mut self, now: Instant) -> SmoothScrollDelta {
        let Some(last) = self.last_advance.replace(now) else {
            return SmoothScrollDelta::default();
        };
        let dt = now
            .saturating_duration_since(last)
            .as_secs_f32()
            .min(MAX_STEP_SECONDS);
        SmoothScrollDelta {
            x: self.x.advance(dt),
            y: self.y.advance(dt),
        }
    }

    fn until_next_pixel(&self) -> Duration {
        let carried = self.carry.x.abs().max(self.carry.y.abs());
        let speed = self.x.velocity.abs().max(self.y.velocity.abs()).max(1.0);
        Duration::from_secs_f32(((1.0 - carried) / speed).clamp(0.004, 0.1))
    }

    pub fn is_active(&self) -> bool {
        self.x.pending().abs().max(self.y.pending().abs()) > REST_DISTANCE
    }

    pub fn stop(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::smooth_scroll::MOS_SCROLL_STEP;

    fn line_down() -> SmoothScrollDelta {
        SmoothScrollDelta {
            x: 0.0,
            y: MOS_SCROLL_STEP,
        }
    }

    /// 按固定帧间隔推进到停下，返回总位移和用了多少帧。
    fn run(frame: Duration) -> (f32, usize) {
        let start = Instant::now();
        let mut inertia = MosInertia::default();
        inertia.push(line_down(), start);
        let (mut moved, mut frames) = (0.0, 0);
        loop {
            frames += 1;
            let step = inertia.step(start + frame * frames as u32);
            moved += step.scroll.y;
            if step.next.is_none() {
                return (moved, frames);
            }
        }
    }

    #[test]
    fn distance_and_duration_do_not_depend_on_the_frame_rate() {
        // 整像素滚，停下时丢掉不足一像素的余数。
        let target = (MOS_SCROLL_STEP * MOS_SCROLL_SPEED).trunc();
        let (at_60, frames_60) = run(Duration::from_micros(16_667));
        let (at_165, frames_165) = run(Duration::from_micros(6_061));
        let (at_20, frames_20) = run(Duration::from_millis(50));
        for moved in [at_60, at_165, at_20] {
            assert!((moved - target).abs() <= 1.0, "{moved} vs {target}");
        }
        // 同一段时长：帧数与帧率成正比。
        let duration = |frames: usize, frame_ms: f32| frames as f32 * frame_ms;
        assert!((duration(frames_60, 16.667) - duration(frames_165, 6.061)).abs() < 20.0);
        assert!((duration(frames_60, 16.667) - duration(frames_20, 50.0)).abs() < 60.0);
    }

    #[test]
    fn one_line_covers_ninety_percent_within_four_hundred_milliseconds() {
        let start = Instant::now();
        let mut inertia = MosInertia::default();
        inertia.push(line_down(), start);
        let moved: f32 = (1..=25)
            .map(|frame| {
                inertia
                    .step(start + Duration::from_millis(16 * frame))
                    .scroll
                    .y
            })
            .sum();
        assert!(
            moved >= (MOS_SCROLL_STEP * MOS_SCROLL_SPEED * 0.9).floor(),
            "{moved}"
        );
    }

    #[test]
    fn same_direction_adds_up_and_reversing_starts_over() {
        let start = Instant::now();
        let mut inertia = MosInertia::default();
        inertia.push(line_down(), start);
        inertia.push(line_down(), start);
        assert!((inertia.y.remaining - 2.0 * MOS_SCROLL_STEP * MOS_SCROLL_SPEED).abs() < 1e-3);

        let up = SmoothScrollDelta {
            x: 0.0,
            y: -MOS_SCROLL_STEP,
        };
        inertia.push(up, start);
        assert!((inertia.y.remaining + MOS_SCROLL_STEP * MOS_SCROLL_SPEED).abs() < 1e-3);
    }

    #[test]
    fn a_long_pause_moves_at_most_one_capped_step() {
        let start = Instant::now();
        let mut inertia = MosInertia::default();
        inertia.push(line_down(), start);
        let capped = inertia
            .clone()
            .advance(start + Duration::from_millis(100))
            .y;
        let paused = inertia.advance(start + Duration::from_secs(5)).y;
        assert_eq!(paused, capped);
    }
}
