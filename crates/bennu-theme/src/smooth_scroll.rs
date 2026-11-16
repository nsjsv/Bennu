//! Mos 式惯性滚动共享层：滚轮增量换算（按 [`SmoothScrollAxis`] 参数化）、按真实时间推进的
//! 惯性（[`MosInertia`]），以及把惯性放在控件里的一对包装（[`smooth_scroll`]）。不依赖任何
//! 应用的 Message/区域模型。

use iced::mouse;

mod driver;
mod inertia;

pub use driver::{smooth_scroll, SmoothScroll};
pub use inertia::{InertiaStep, MosInertia};

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
