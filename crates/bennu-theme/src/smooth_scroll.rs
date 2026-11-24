//! 惯性滚动共享层：滚轮增量换算（按 [`SmoothScrollAxis`] 参数化）、按真实时间推进的
//! 滚轮惯性（Mos 式，[`MosInertia`]，按转速加速）、触控板的跟手/滑行/边缘回弹，以及把这些
//! 放在控件里的一对包装（[`smooth_scroll`]）。不依赖任何应用的 Message/区域模型。

use iced::mouse;

mod acceleration;
mod driver;
mod inertia;
mod scroller;
mod touch;

pub use driver::{smooth_scroll, SmoothScroll};
pub use inertia::{InertiaStep, MosInertia};
pub use touch::{Orientation, ScrollTarget, TouchScroll};

pub const MOS_SCROLL_STEP: f32 = 33.6;
pub const MOS_SCROLL_SPEED: f32 = 2.70;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SmoothScrollDelta {
    pub x: f32,
    pub y: f32,
}

impl SmoothScrollDelta {
    pub fn is_resting(self) -> bool {
        self.x.abs() <= f32::EPSILON && self.y.abs() <= f32::EPSILON
    }

    pub fn magnitude(self) -> f32 {
        self.x.abs().max(self.y.abs())
    }
}

/// 触控板手势结束（手指离开）：约定是零位移的像素滚动，见 vendored iced_winit 的事件转换。
pub fn is_gesture_end(delta: mouse::ScrollDelta) -> bool {
    matches!(delta, mouse::ScrollDelta::Pixels { x, y } if x == 0.0 && y == 0.0)
}

/// 滚轮往哪边拨：缩放、翻句这类一次走一档的按它走。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDirection {
    /// 往上（远离用户），y > 0。
    Up,
    Down,
}

/// 竖向拨动的方向；手势结束（[`is_gesture_end`]）没有方向，不是一次往下。
pub fn wheel_direction(delta: mouse::ScrollDelta) -> Option<WheelDirection> {
    let (mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. }) = delta;
    if y > 0.0 {
        Some(WheelDirection::Up)
    } else if y < 0.0 {
        Some(WheelDirection::Down)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelScrollMode {
    MosAnimated,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelScrollDelta {
    pub delta: SmoothScrollDelta,
    pub mode: WheelScrollMode,
}

/// 滚动区参与的滚动轴向：区域到轴向的映射属于各应用的区域语义，
/// 由调用方完成；本层只认轴向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothScrollAxis {
    Vertical,
    /// 两个方向都能滚（不换行的文本）：触控板、小红点、倾斜滚轮的横向分量滚横向，竖向分量
    /// 滚竖向，斜着划两个方向一起动；shift+竖向滚轮转成横向。
    Both,
    Horizontal,
    HorizontalFromPrimaryWheel,
    /// 竖向滚动，但 shift 按下时不产生增量（返回 None）：多栏单列等
    /// 嵌套滚动区在 shift 横滚时必须让事件冒泡到外层横向区。包装层
    /// 不能靠"按 shift 切换是否包 wrapper"表达该语义——树形状逐帧
    /// 变化会让 iced diff 重建子树、滚动位置跳顶。
    VerticalIgnoringShift,
}

pub fn wheel_delta_for_axis(
    axis: SmoothScrollAxis,
    shift_pressed: bool,
    delta: mouse::ScrollDelta,
) -> Option<WheelScrollDelta> {
    let delta = match axis {
        SmoothScrollAxis::Vertical => vertical_wheel_delta(shift_pressed, delta),
        SmoothScrollAxis::Both => both_wheel_delta(shift_pressed, delta),
        SmoothScrollAxis::Horizontal => horizontal_wheel_delta(shift_pressed, delta),
        SmoothScrollAxis::HorizontalFromPrimaryWheel => horizontal_primary_wheel_delta(delta),
        // shift 横滚交给外层滚动区，本区连换算都不做，保持与旧
        // wheel_delta_for_region 的 None 分支逐位一致。
        SmoothScrollAxis::VerticalIgnoringShift if shift_pressed => return None,
        SmoothScrollAxis::VerticalIgnoringShift => vertical_wheel_delta(false, delta),
    };

    (!delta.delta.is_resting()).then_some(delta)
}

fn vertical_wheel_delta(shift_pressed: bool, delta: mouse::ScrollDelta) -> WheelScrollDelta {
    // winit 不做 shift 竖转横，滚轮事件 x 恒为 0；shift 时必须取 y 转
    // 横向输出，否则增量恒为零被 is_resting 丢弃。
    if shift_pressed {
        return horizontal_wheel_delta(true, delta);
    }
    match delta {
        mouse::ScrollDelta::Lines { x: _, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: 0.0,
                y: -y * MOS_SCROLL_STEP,
            },
            mode: WheelScrollMode::MosAnimated,
        },
        mouse::ScrollDelta::Pixels { x: _, y } => WheelScrollDelta {
            delta: SmoothScrollDelta { x: 0.0, y: -y },
            mode: WheelScrollMode::Direct,
        },
    }
}

fn both_wheel_delta(shift_pressed: bool, delta: mouse::ScrollDelta) -> WheelScrollDelta {
    if shift_pressed {
        return horizontal_wheel_delta(true, delta);
    }
    match delta {
        mouse::ScrollDelta::Lines { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: -x * MOS_SCROLL_STEP,
                y: -y * MOS_SCROLL_STEP,
            },
            mode: WheelScrollMode::MosAnimated,
        },
        mouse::ScrollDelta::Pixels { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta { x: -x, y: -y },
            mode: WheelScrollMode::Direct,
        },
    }
}

fn horizontal_wheel_delta(shift_pressed: bool, delta: mouse::ScrollDelta) -> WheelScrollDelta {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: -if shift_pressed { y } else { x } * MOS_SCROLL_STEP,
                y: 0.0,
            },
            mode: WheelScrollMode::MosAnimated,
        },
        mouse::ScrollDelta::Pixels { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: -if shift_pressed { y } else { x },
                y: 0.0,
            },
            mode: WheelScrollMode::Direct,
        },
    }
}

fn horizontal_primary_wheel_delta(delta: mouse::ScrollDelta) -> WheelScrollDelta {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: -if x.abs() > f32::EPSILON { x } else { y } * MOS_SCROLL_STEP,
                y: 0.0,
            },
            mode: WheelScrollMode::MosAnimated,
        },
        mouse::ScrollDelta::Pixels { x, y } => WheelScrollDelta {
            delta: SmoothScrollDelta {
                x: -if x.abs() > f32::EPSILON { x } else { y },
                y: 0.0,
            },
            mode: WheelScrollMode::Direct,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_line_delta_matches_native_wheel_direction() {
        let delta = vertical_wheel_delta(false, mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 });

        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: 0.0,
                    y: MOS_SCROLL_STEP,
                },
                mode: WheelScrollMode::MosAnimated,
            },
        );
    }

    #[test]
    fn shifted_vertical_area_converts_wheel_to_horizontal_scroll() {
        // winit 滚轮事件 x 恒为 0,shift 必须取 y 转横向输出,否则增量
        // 恒为零被丢弃。
        let delta = vertical_wheel_delta(true, mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 });
        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            }
        );
        let delta = vertical_wheel_delta(true, mouse::ScrollDelta::Pixels { x: 0.0, y: -30.0 });
        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta { x: 30.0, y: 0.0 },
                mode: WheelScrollMode::Direct,
            }
        );
    }

    #[test]
    fn vertical_ignoring_shift_axis_rejects_shifted_wheel_but_keeps_plain_vertical() {
        // shift 横滚必须冒泡到外层横向区：本轴对 shift 事件不认领。
        assert_eq!(
            wheel_delta_for_axis(
                SmoothScrollAxis::VerticalIgnoringShift,
                true,
                mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            ),
            None
        );

        // 普通（无 shift）滚轮与纯 Vertical 完全一致。
        assert_eq!(
            wheel_delta_for_axis(
                SmoothScrollAxis::VerticalIgnoringShift,
                false,
                mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            ),
            wheel_delta_for_axis(
                SmoothScrollAxis::Vertical,
                false,
                mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            )
        );
    }

    #[test]
    fn both_axis_keeps_the_sideways_component_of_a_touchpad_or_tilt_wheel() {
        // 竖向轴把横向分量丢了：触控板横划、小红点横推在两向区里一点反应都没有。
        let diagonal = both_wheel_delta(false, mouse::ScrollDelta::Pixels { x: -6.0, y: -4.0 });
        assert_eq!(
            diagonal,
            WheelScrollDelta {
                delta: SmoothScrollDelta { x: 6.0, y: 4.0 },
                mode: WheelScrollMode::Direct,
            }
        );
        let tilt = wheel_delta_for_axis(
            SmoothScrollAxis::Both,
            false,
            mouse::ScrollDelta::Lines { x: 1.0, y: 0.0 },
        );
        assert_eq!(
            tilt,
            Some(WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: -MOS_SCROLL_STEP,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            })
        );
    }

    #[test]
    fn both_axis_turns_a_shifted_vertical_wheel_into_horizontal_scroll() {
        let delta = both_wheel_delta(true, mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 });
        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            }
        );
    }

    #[test]
    fn vertical_pixel_delta_preserves_native_wheel_delta() {
        let delta = vertical_wheel_delta(false, mouse::ScrollDelta::Pixels { x: 0.0, y: -4.0 });

        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta { x: 0.0, y: 4.0 },
                mode: WheelScrollMode::Direct,
            }
        );
    }

    #[test]
    fn horizontal_axis_ignores_unshifted_vertical_wheel() {
        let delta = wheel_delta_for_axis(
            SmoothScrollAxis::Horizontal,
            false,
            mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        );

        assert_eq!(delta, None);
    }

    #[test]
    fn horizontal_from_primary_wheel_axis_uses_vertical_wheel_for_horizontal_scroll() {
        let delta = wheel_delta_for_axis(
            SmoothScrollAxis::HorizontalFromPrimaryWheel,
            false,
            mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
        );

        assert_eq!(
            delta,
            Some(WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            })
        );
    }

    #[test]
    fn shifted_horizontal_axis_uses_vertical_wheel_for_horizontal_delta() {
        let delta = wheel_delta_for_axis(
            SmoothScrollAxis::Horizontal,
            true,
            mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
        );

        assert_eq!(
            delta,
            Some(WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            })
        );
    }

    #[test]
    fn shifted_horizontal_axis_uses_vertical_wheel_for_horizontal_scroll() {
        let delta = horizontal_wheel_delta(true, mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 });

        assert_eq!(
            delta,
            WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            }
        );
    }
}
