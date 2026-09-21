//! Mos 式惯性滚动共享层：滚轮增量换算（按 [`SmoothScrollAxis`] 参数化）、
//! Mos 累积/滤波状态机，以及捕获滚轮事件的包装 widget。本模块不依赖
//! 任何应用的 Message/区域模型：区域泛型由调用方注入，滚轮事件经
//! `on_wheel` 闭包交回调用方构造自己的消息。

use iced::advanced::widget as advanced_widget;
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Rectangle, Size, Vector};

pub const MOS_SCROLL_STEP: f32 = 33.6;
pub const MOS_SCROLL_SPEED: f32 = 2.70;
pub const MOS_SCROLL_DURATION_TRANSITION: f32 = 0.125;
pub const MOS_SCROLL_DEAD_ZONE: f32 = 0.1;
pub const MOS_SCROLL_FILTER_WEIGHT: f32 = 0.23;

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

#[derive(Debug, Clone)]
pub struct MosScrollState<Region> {
    active_region: Option<Region>,
    current: SmoothScrollDelta,
    buffer: SmoothScrollDelta,
    last_input: SmoothScrollDelta,
    filter: MosScrollFilter,
}

// 手写 Default 避免派生给 Region 强加 Default 约束：初始态里 Region
// 只会以 None 出现，不需要 Region 有默认值。
impl<Region> Default for MosScrollState<Region> {
    fn default() -> Self {
        Self {
            active_region: None,
            current: SmoothScrollDelta::default(),
            buffer: SmoothScrollDelta::default(),
            last_input: SmoothScrollDelta::default(),
            filter: MosScrollFilter::default(),
        }
    }
}

impl<Region: Clone + PartialEq> MosScrollState<Region> {
    pub fn push_wheel_delta(&mut self, region: Region, delta: SmoothScrollDelta) {
        if self.active_region.as_ref() != Some(&region) {
            self.active_region = Some(region);
            self.current = SmoothScrollDelta::default();
            self.buffer = SmoothScrollDelta::default();
            self.last_input = SmoothScrollDelta::default();
            self.filter = MosScrollFilter::default();
        }

        update_mos_axis(
            &mut self.current.y,
            &mut self.buffer.y,
            self.last_input.y,
            delta.y,
        );
        update_mos_axis(
            &mut self.current.x,
            &mut self.buffer.x,
            self.last_input.x,
            delta.x,
        );
        self.last_input = delta;
    }

    pub fn next_frame_delta(&mut self) -> Option<(Region, SmoothScrollDelta)> {
        let frame = SmoothScrollDelta {
            x: (self.buffer.x - self.current.x) * MOS_SCROLL_DURATION_TRANSITION,
            y: (self.buffer.y - self.current.y) * MOS_SCROLL_DURATION_TRANSITION,
        };

        self.current.x += frame.x;
        self.current.y += frame.y;

        let filtered = self.filter.fill(frame);
        if filtered.magnitude() <= MOS_SCROLL_DEAD_ZONE {
            return None;
        }

        self.active_region.clone().map(|region| (region, filtered))
    }

    pub fn is_active(&self) -> bool {
        if self.active_region.is_none() {
            return false;
        }

        let residual = SmoothScrollDelta {
            x: self.buffer.x - self.current.x,
            y: self.buffer.y - self.current.y,
        };

        residual.magnitude() > MOS_SCROLL_DEAD_ZONE
            || self.filter.pending().magnitude() > MOS_SCROLL_DEAD_ZONE
    }

    pub fn stop(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Default)]
pub struct MosScrollFilter {
    next_output: SmoothScrollDelta,
}

impl MosScrollFilter {
    fn fill(&mut self, frame: SmoothScrollDelta) -> SmoothScrollDelta {
        let output = self.next_output;
        self.next_output.x = polish_filter_axis(self.next_output.x, frame.x);
        self.next_output.y = polish_filter_axis(self.next_output.y, frame.y);
        output
    }

    fn pending(&self) -> SmoothScrollDelta {
        self.next_output
    }
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

pub struct SmoothScrollArea<'a, Message> {
    content: Element<'a, Message>,
    axis: SmoothScrollAxis,
    shift_pressed: bool,
    on_wheel: Box<dyn Fn(mouse::ScrollDelta) -> Message + 'a>,
}

impl<'a, Message> SmoothScrollArea<'a, Message> {
    pub fn new(
        content: impl Into<Element<'a, Message>>,
        axis: SmoothScrollAxis,
        shift_pressed: bool,
        on_wheel: impl Fn(mouse::ScrollDelta) -> Message + 'a,
    ) -> Self {
        Self {
            content: content.into(),
            axis,
            shift_pressed,
            on_wheel: Box::new(on_wheel),
        }
    }
}

impl<Message: 'static> Widget<Message, iced::Theme, iced::Renderer>
    for SmoothScrollArea<'_, Message>
{
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut advanced_widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn children(&self) -> Vec<advanced_widget::Tree> {
        vec![advanced_widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut advanced_widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn operate(
        &mut self,
        tree: &mut advanced_widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut advanced_widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let cursor_over_area = cursor.is_over(layout.bounds());

        // 嵌套滚动区必须让内层先认领 wheel，外层再兜底处理。
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        if shell.is_event_captured() {
            return;
        }

        if cursor_over_area {
            if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event {
                if wheel_delta_for_axis(self.axis, self.shift_pressed, *delta).is_some() {
                    shell.publish((self.on_wheel)(*delta));
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &advanced_widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &advanced_widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut advanced_widget::Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
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

fn update_mos_axis(current: &mut f32, buffer: &mut f32, last_input: f32, incoming: f32) {
    let scaled = incoming * MOS_SCROLL_SPEED;
    if incoming * last_input > 0.0 {
        *buffer += scaled;
    } else {
        *buffer = scaled;
        *current = 0.0;
    }
}

fn polish_filter_axis(current: f32, next: f32) -> f32 {
    current + MOS_SCROLL_FILTER_WEIGHT * (next - current)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试专用区域占位：状态机对 Region 只要求 Clone + PartialEq，
    // 同区累积、跨区重置的语义与具体区域枚举无关。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestRegion {
        First,
        Second,
    }

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

    #[test]
    fn mos_global_state_accumulates_same_region_buffer() {
        let mut state = MosScrollState::default();
        let region = TestRegion::First;
        state.push_wheel_delta(
            region,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );
        state.push_wheel_delta(
            region,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );

        assert!((state.buffer.y - MOS_SCROLL_STEP * MOS_SCROLL_SPEED * 2.0).abs() <= 0.0001);
        assert_eq!(state.current.y, 0.0);
    }

    #[test]
    fn mos_global_state_resets_current_on_opposite_direction() {
        let mut state = MosScrollState::default();
        let region = TestRegion::First;
        state.current.y = 12.0;
        state.push_wheel_delta(
            region,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );
        state.push_wheel_delta(
            region,
            SmoothScrollDelta {
                x: 0.0,
                y: -MOS_SCROLL_STEP,
            },
        );

        assert!((state.buffer.y + MOS_SCROLL_STEP * MOS_SCROLL_SPEED).abs() <= 0.0001);
        assert_eq!(state.current.y, 0.0);
    }

    #[test]
    fn mos_filter_delays_first_scroll_frame() {
        let mut state = MosScrollState::default();
        state.push_wheel_delta(
            TestRegion::First,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );

        assert_eq!(state.next_frame_delta(), None);
        let (_, second_frame) = state.next_frame_delta().expect("second frame");

        assert!(second_frame.y > MOS_SCROLL_DEAD_ZONE);
    }

    #[test]
    fn standard_line_reaches_ninety_percent_within_twenty_four_frames() {
        let mut state = MosScrollState::default();
        let mut input = SmoothScrollDelta::default();
        input.y = MOS_SCROLL_STEP;
        state.push_wheel_delta(TestRegion::First, input);
        let target = MOS_SCROLL_STEP * MOS_SCROLL_SPEED;
        let mut scrolled = 0.0;
        for _ in 0..24 {
            scrolled += state.next_frame_delta().map_or(0.0, |frame| frame.1.y);
        }
        assert!((target - 90.72).abs() <= 0.0001);
        assert!(scrolled >= target * 0.9);
    }

    #[test]
    fn mos_filter_preserves_fractional_line_wheel_delta() {
        let mut state = MosScrollState::default();
        state.push_wheel_delta(
            TestRegion::First,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP * 0.1,
            },
        );

        assert_eq!(state.next_frame_delta(), None);
        let (_, second_frame) = state.next_frame_delta().expect("second frame");

        assert!(second_frame.y > MOS_SCROLL_DEAD_ZONE);
    }

    #[test]
    fn mos_global_state_switches_active_region() {
        let mut state = MosScrollState::default();
        state.push_wheel_delta(
            TestRegion::First,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );
        state.current.y = 12.0;

        state.push_wheel_delta(
            TestRegion::Second,
            SmoothScrollDelta {
                x: 0.0,
                y: MOS_SCROLL_STEP,
            },
        );

        assert_eq!(state.active_region, Some(TestRegion::Second));
        assert_eq!(state.current.y, 0.0);
        assert!((state.buffer.y - MOS_SCROLL_STEP * MOS_SCROLL_SPEED).abs() <= 0.0001);
    }
}
