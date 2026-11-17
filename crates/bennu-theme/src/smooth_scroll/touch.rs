//! 触控板（像素增量）滚动的运动学，曲线照 iOS/macOS：
//!
//! - 手指在板上：内容跟着手指走；
//! - 松手：按最后 0.1 秒的手指速度继续滑，每毫秒速度乘 0.998；
//! - 拉过边缘：显示的越界 = `(1 − 1/(x·0.55/d + 1))·d`（x 是拉过的距离，d 是视口长度），
//!   越拉越紧；松手或滑到边时用临界阻尼弹簧（质量 1、刚度 100）弹回。
//!
//! 滚动区怎么滚、边缘在哪由它自己说了算（[`ScrollTarget`]）：这一轴不能滚、或者那头还有
//! 没读进来的内容时不越界。这里只管运动学，不碰控件。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use iced::{window, Vector};

/// 每毫秒速度乘 0.998：λ = −ln(0.998) × 1000。
const DECELERATION_PER_SECOND: f32 = 2.002_002;
/// 越界拉伸系数。
const RUBBER_BAND: f32 = 0.55;
/// 弹回的弹簧：√(刚度 / 质量)。
const SPRING_FREQUENCY: f32 = 10.0;
/// 弹回时越界和速度都小于这些就算回位。
const SPRING_REST_DISTANCE: f32 = 0.5;
const SPRING_REST_SPEED: f32 = 5.0;
/// 松手速度只看最后这段时间的手指移动：中途停住再松手不会飞出去。
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);
/// 拉过边缘后这么久既没有新输入、也没有松手信号，就当松手（高精度滚轮的半格像素滚动没有
/// 结束信号，不能一直拉着）。
const RELEASE_AFTER: Duration = Duration::from_millis(250);
/// 每帧最多推进这么久：窗口被挡住、切回来时不一下滑完。
const MAX_STEP_SECONDS: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// 触控板滚动作用的滚动区。
pub trait ScrollTarget {
    /// 沿 `orientation` 滚 `delta`：返回滚过的，和越过边缘的（这根轴不能滚、或者那头还有没读
    /// 进来的内容时是 0）。
    fn scroll(&mut self, orientation: Orientation, delta: f32) -> (f32, f32);
    /// 视口在这根轴上的长度（越界拉伸按它算）。
    fn extent(&self, orientation: Orientation) -> f32;
}

/// 一根轴上能滚到 `0..=max`（`max` 不大于 0 就是不能滚）时，滚 `delta` 滚过的和越过边缘的。
pub(super) fn split_at_edges(offset: f32, max: f32, delta: f32) -> (f32, f32) {
    if max <= 0.0 {
        return (0.0, 0.0);
    }
    let target = offset + delta;
    let reached = target.clamp(0.0, max);
    (reached - offset, target - reached)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Motion {
    #[default]
    Still,
    /// 手指按着；`stretch` 是拉过边缘的距离（滚动方向，正 = 越过末端）。
    Held { stretch: f32 },
    /// 松手后的惯性（像素/秒）。
    Coasting { velocity: f32 },
    /// 弹回：`displacement` 是画出来的越界。
    Settling { displacement: f32, velocity: f32 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct AxisMotion {
    motion: Motion,
    /// 最近一次量到的视口长度（越界拉伸按它算）。
    extent: f32,
    /// 惯性走过但还没滚出去的不足一像素的部分。
    carry: f32,
}

impl AxisMotion {
    fn displacement(self) -> f32 {
        match self.motion {
            Motion::Held { stretch } => rubber_band(stretch, self.extent),
            Motion::Settling { displacement, .. } => displacement,
            Motion::Still | Motion::Coasting { .. } => 0.0,
        }
    }

    /// 手指放上来：惯性停下；正在弹回的越界被手指接住，换算回拉过的距离。
    fn grab(&mut self) {
        let stretch = match self.motion {
            Motion::Held { stretch } => stretch,
            Motion::Settling { displacement, .. } => unstretch(displacement, self.extent),
            Motion::Still | Motion::Coasting { .. } => 0.0,
        };
        self.motion = Motion::Held { stretch };
        self.carry = 0.0;
    }

    /// 手指移动 `delta`：先收回拉过边缘的，余下的滚内容，滚不动的部分拉过边缘。
    fn follow(&mut self, delta: f32, target: &mut impl ScrollTarget, orientation: Orientation) {
        let Motion::Held { stretch } = self.motion else {
            return;
        };
        self.extent = target.extent(orientation);
        let pulled = stretch + delta;
        if stretch != 0.0 && pulled * stretch > 0.0 {
            self.motion = Motion::Held { stretch: pulled };
            return;
        }
        // 没拉着，或者拉回来越过了边缘：剩下的交给内容。
        let remaining = if stretch == 0.0 { delta } else { pulled };
        let (_, beyond) = target.scroll(orientation, remaining);
        self.motion = Motion::Held { stretch: beyond };
    }

    fn release(&mut self, velocity: f32) {
        let Motion::Held { stretch } = self.motion else {
            return;
        };
        self.motion = if stretch != 0.0 {
            Motion::Settling {
                displacement: rubber_band(stretch, self.extent),
                velocity: 0.0,
            }
        } else {
            Motion::Coasting { velocity }
        };
        self.stop_coasting_without_another_pixel();
    }

    /// 惯性往后再也走不出一个整像素就停（只滚整像素，余下的画出来都一样）。
    fn stop_coasting_without_another_pixel(&mut self) {
        if matches!(self.motion, Motion::Coasting { .. }) && self.until_next_pixel().is_none() {
            self.motion = Motion::Still;
        }
    }

    /// 惯性攒够下一个整像素要多久：x(t) = v/λ·(1 − e^(−λt)) 解出来；永远攒不够是 `None`。
    fn until_next_pixel(self) -> Option<Duration> {
        let Motion::Coasting { velocity } = self.motion else {
            return None;
        };
        let needed = 1.0 - self.carry.abs();
        let fraction = DECELERATION_PER_SECOND * needed / velocity.abs();
        (fraction < 1.0)
            .then(|| Duration::from_secs_f32(-(1.0 - fraction).ln() / DECELERATION_PER_SECOND))
    }

    /// 推进 `dt` 秒：滑行攒够整像素就滚，弹回只改画的位置。
    fn advance(&mut self, dt: f32, target: &mut impl ScrollTarget, orientation: Orientation) {
        match self.motion {
            Motion::Still | Motion::Held { .. } => {}
            Motion::Coasting { velocity } => {
                let decay = (-DECELERATION_PER_SECOND * dt).exp();
                self.carry += velocity * (1.0 - decay) / DECELERATION_PER_SECOND;
                let velocity = velocity * decay;
                let whole = self.carry.trunc();
                self.carry -= whole;
                if whole == 0.0 {
                    self.motion = Motion::Coasting { velocity };
                } else {
                    let (scrolled, beyond) = target.scroll(orientation, whole);
                    self.motion = if beyond != 0.0 {
                        // 滑到边：带着这时的速度冲出去，再被弹簧拉回来。
                        Motion::Settling {
                            displacement: beyond,
                            velocity,
                        }
                    } else if scrolled == 0.0 {
                        // 滚不动、又不是边（不能滚，或者那头的内容还没读进来）：停。
                        Motion::Still
                    } else {
                        Motion::Coasting { velocity }
                    };
                }
                self.stop_coasting_without_another_pixel();
            }
            Motion::Settling {
                displacement,
                velocity,
            } => {
                // 临界阻尼弹簧的闭式解：x(t) = (x₀ + (v₀ + ωx₀)t)·e^(−ωt)。
                let omega = SPRING_FREQUENCY;
                let decay = (-omega * dt).exp();
                let coupled = velocity + omega * displacement;
                let next = (displacement + coupled * dt) * decay;
                let speed = (velocity - omega * coupled * dt) * decay;
                self.motion =
                    if next.abs() < SPRING_REST_DISTANCE && speed.abs() < SPRING_REST_SPEED {
                        Motion::Still
                    } else {
                        Motion::Settling {
                            displacement: next,
                            velocity: speed,
                        }
                    };
            }
        }
    }

    fn is_stretched(self) -> bool {
        matches!(self.motion, Motion::Held { stretch } if stretch != 0.0)
    }
}

/// 拉过 `stretch` 时画出来的越界。
fn rubber_band(stretch: f32, extent: f32) -> f32 {
    if extent <= 0.0 {
        return 0.0;
    }
    let pulled = stretch.abs();
    (1.0 - 1.0 / (pulled * RUBBER_BAND / extent + 1.0)) * extent * stretch.signum()
}

/// [`rubber_band`] 的反函数：画出来越界 `displacement` 时拉过了多远。
fn unstretch(displacement: f32, extent: f32) -> f32 {
    if extent <= 0.0 {
        return 0.0;
    }
    let shown = (displacement.abs() / extent).min(0.99);
    shown * extent / (RUBBER_BAND * (1.0 - shown)) * displacement.signum()
}

/// 一个滚动区的触控板滚动。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchScroll {
    x: AxisMotion,
    y: AxisMotion,
    /// 手指按着时最近的移动（时刻、位移），算松手速度。
    recent: VecDeque<(Instant, Vector)>,
    last_advance: Option<Instant>,
}

impl TouchScroll {
    /// 手指移动了 `delta`（滚动方向）：内容跟着滚，滚不动的拉过边缘。
    pub fn follow(&mut self, delta: Vector, now: Instant, target: &mut impl ScrollTarget) {
        if !self.is_held() {
            self.x.grab();
            self.y.grab();
            self.recent.clear();
        }
        self.recent.push_back((now, delta));
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > VELOCITY_WINDOW)
        {
            self.recent.pop_front();
        }
        self.x.follow(delta.x, target, Orientation::Horizontal);
        self.y.follow(delta.y, target, Orientation::Vertical);
    }

    /// 手指离开：按最后一段的速度滑出去，拉过边缘的弹回。
    pub fn release(&mut self, now: Instant) {
        if !self.is_held() {
            return;
        }
        let velocity = self.release_velocity(now);
        self.x.release(velocity.x);
        self.y.release(velocity.y);
        self.recent.clear();
        self.last_advance = Some(now);
    }

    /// 最后 [`VELOCITY_WINDOW`] 里手指走的距离除以时间；第一笔是它之前那段时间走的，不算。
    fn release_velocity(&self, now: Instant) -> Vector {
        let mut recent = self
            .recent
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= VELOCITY_WINDOW);
        let Some((first, _)) = recent.next() else {
            return Vector::ZERO;
        };
        let moved = recent.fold(Vector::ZERO, |moved, (_, delta)| moved + *delta);
        let seconds = now.saturating_duration_since(*first).as_secs_f32();
        if seconds <= 0.0 {
            return Vector::ZERO;
        }
        moved * (1.0 / seconds)
    }

    /// 推进到 `now`。
    pub fn advance(&mut self, now: Instant, target: &mut impl ScrollTarget) {
        if self.is_held() {
            let idle = self
                .recent
                .back()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= RELEASE_AFTER);
            if idle && (self.x.is_stretched() || self.y.is_stretched()) {
                // 不再有输入了：只弹回，不按很久以前的速度滑。
                self.recent.clear();
                self.release(now);
            }
            return;
        }
        let Some(last) = self.last_advance.replace(now) else {
            return;
        };
        let dt = now
            .saturating_duration_since(last)
            .as_secs_f32()
            .min(MAX_STEP_SECONDS);
        self.x.advance(dt, target, Orientation::Horizontal);
        self.y.advance(dt, target, Orientation::Vertical);
    }

    /// 画内容时要挪开的越界（滚动方向，正 = 越过末端），取整像素。
    pub fn displacement(&self) -> Vector {
        Vector::new(self.x.displacement().round(), self.y.displacement().round())
    }

    /// 还要不要再画：滑着、弹着要下一帧；拉着边缘不动时到点自动松手。
    pub fn next_redraw(&self) -> Option<window::RedrawRequest> {
        let settling = |axis: AxisMotion| matches!(axis.motion, Motion::Settling { .. });
        if settling(self.x) || settling(self.y) {
            return Some(window::RedrawRequest::NextFrame);
        }
        let next_pixel = [self.x, self.y]
            .into_iter()
            .filter_map(AxisMotion::until_next_pixel)
            .min();
        if let (Some(wait), Some(last)) = (next_pixel, self.last_advance) {
            return Some(window::RedrawRequest::At(last + wait));
        }
        if self.x.is_stretched() || self.y.is_stretched() {
            let (last_input, _) = self.recent.back()?;
            return Some(window::RedrawRequest::At(*last_input + RELEASE_AFTER));
        }
        None
    }

    /// 只在惯性滑动时为真：别处挪过滚动区（拖滚动条、程序跳转）时，驱动器据此停下它。
    pub fn is_coasting(&self) -> bool {
        matches!(self.x.motion, Motion::Coasting { .. })
            || matches!(self.y.motion, Motion::Coasting { .. })
    }

    /// 手指按着，或者还在滑、在弹。
    pub fn is_active(&self) -> bool {
        self.is_held() || self.next_redraw().is_some()
    }

    pub fn is_held(&self) -> bool {
        matches!(self.x.motion, Motion::Held { .. }) || matches!(self.y.motion, Motion::Held { .. })
    }

    pub fn stop(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
#[path = "touch_tests.rs"]
mod tests;
