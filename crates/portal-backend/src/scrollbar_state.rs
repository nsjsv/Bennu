//! 区域泛化的 mac 式滚动条显隐状态机：单活跃区域、代数戳防旧 hide、
//! 视口快照按区域缓存、淡入/淡出透明度推进。自 picker_session/scrollbar.rs
//! 抽出的公共核（选择窗与预览窗两个滚动管线家族共用），时序常量与
//! 缓动函数保持主软件节奏。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bennu_theme::scrollbar::ScrollbarViewport;
use bennu_theme::styles::ScrollbarVisibility;
use iced::Task;

/// 滚动条淡入时长：与主软件 reveal 节奏一致。
pub(crate) const SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(96);
/// 滚动条淡出时长：与主软件 hide 节奏一致。
pub(crate) const SCROLLBAR_HIDE_DURATION: Duration = Duration::from_millis(300);
/// 淡入起始透明度下限：滚动中途反向时滑块不从零闪现。
pub(crate) const SCROLLBAR_MIN_REVEAL_OPACITY: f32 = 0.12;
/// 无输入后自动隐藏的延迟：与主软件一致。
pub(crate) const SCROLLBAR_AUTO_HIDE_DURATION: Duration = Duration::from_millis(650);

#[derive(Debug, Clone, Copy)]
enum ScrollbarOpacityAnimation {
    Revealing {
        started_at: Instant,
        initial_opacity: f32,
    },
    Hiding {
        started_at: Instant,
        initial_opacity: f32,
    },
}

/// 滚动条显隐状态机（主软件 ScrollbarState 的区域泛化移植）：单活跃
/// 区域、代数戳防旧 hide、视口快照按区域缓存。
#[derive(Debug)]
pub(crate) struct ScrollbarStateMachine<R> {
    active_region: Option<R>,
    visibility: ScrollbarVisibility,
    auto_hide_generation: u64,
    animation: Option<ScrollbarOpacityAnimation>,
    viewport_by_region: HashMap<R, ScrollbarViewport>,
}

impl<R: Copy + Eq + std::hash::Hash> Default for ScrollbarStateMachine<R> {
    fn default() -> Self {
        Self {
            active_region: None,
            visibility: ScrollbarVisibility::Hidden,
            auto_hide_generation: 0,
            animation: None,
            viewport_by_region: HashMap::new(),
        }
    }
}

impl<R: Copy + Eq + std::hash::Hash> ScrollbarStateMachine<R> {
    /// 滚动条淡入/淡出是否在播放（帧时钟挂载条件之一）。
    pub(crate) fn is_animating(&self) -> bool {
        self.animation.is_some()
    }

    /// 失效某区域视口缓存：内容整体更换后旧几何是错误数据，由下一次
    /// 布局探针回信重填。
    pub(crate) fn invalidate_viewport(&mut self, region: R) {
        self.viewport_by_region.remove(&region);
    }

    /// 写入区域视口快照（on_scroll 回传与布局探针共用入口）。
    pub(crate) fn remember_viewport(&mut self, region: R, viewport: ScrollbarViewport) {
        self.viewport_by_region.insert(region, viewport);
    }

    pub(crate) fn viewport_for(&self, region: &R) -> Option<ScrollbarViewport> {
        self.viewport_by_region.get(region).copied()
    }

    pub(crate) fn visibility_for(&self, region: &R) -> ScrollbarVisibility {
        if self.active_region.as_ref() == Some(region) {
            self.visibility
        } else {
            ScrollbarVisibility::Hidden
        }
    }

    /// 当前代数戳（测试断言用：生产代码由 start_reveal 返回值携带）。
    #[cfg(test)]
    pub(crate) fn auto_hide_generation(&self) -> u64 {
        self.auto_hide_generation
    }

    /// 溢出核实后的淡入入口：推进代数戳并返回，供 auto-hide 延迟回信
    /// 校验（期间又有滚动则代数不匹配，旧回信丢弃）。
    pub(crate) fn start_reveal(&mut self, region: R) -> u64 {
        self.active_region = Some(region);
        self.auto_hide_generation = self.auto_hide_generation.wrapping_add(1);

        let current_opacity = self.visibility.opacity();
        if (1.0 - current_opacity) <= f32::EPSILON {
            self.visibility = ScrollbarVisibility::Visible;
            self.animation = None;
        } else if !matches!(
            self.animation,
            Some(ScrollbarOpacityAnimation::Revealing { .. })
        ) {
            let initial_opacity = current_opacity.max(SCROLLBAR_MIN_REVEAL_OPACITY);
            self.visibility = ScrollbarVisibility::with_opacity(initial_opacity);
            self.animation = Some(ScrollbarOpacityAnimation::Revealing {
                started_at: Instant::now(),
                initial_opacity,
            });
        }
        self.auto_hide_generation
    }

    /// auto-hide 延迟回信：代数不匹配（期间又有 reveal）直接丢弃。
    pub(crate) fn start_hide(&mut self, generation: u64) {
        if self.auto_hide_generation != generation {
            return;
        }

        let initial_opacity = self.visibility.opacity();
        if initial_opacity <= f32::EPSILON {
            self.active_region = None;
            self.visibility = ScrollbarVisibility::Hidden;
            self.animation = None;
            return;
        }

        self.animation = Some(ScrollbarOpacityAnimation::Hiding {
            started_at: Instant::now(),
            initial_opacity,
        });
    }

    /// 帧推进淡入/淡出透明度；播完后回收显隐状态。
    pub(crate) fn advance_animation(&mut self) {
        let Some(animation) = self.animation else {
            return;
        };

        let still_active = match animation {
            ScrollbarOpacityAnimation::Revealing {
                started_at,
                initial_opacity,
            } => advance_reveal(self, started_at, initial_opacity),
            ScrollbarOpacityAnimation::Hiding {
                started_at,
                initial_opacity,
            } => advance_hide(self, started_at, initial_opacity),
        };

        if !still_active {
            self.active_region = None;
            self.visibility = ScrollbarVisibility::Hidden;
            self.animation = None;
        }
    }

    /// 整体复位（会话结束）：区域/显隐/动画/视口缓存全清。
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 650ms 无输入后回信 auto-hide；回信携带代数，重排后的旧回信被丢弃。
pub(crate) fn scrollbar_auto_hide_task<Message>(
    generation: u64,
    elapsed: impl Fn(u64) -> Message + Send + 'static,
) -> Task<Message>
where
    Message: Send + 'static,
{
    Task::perform(
        async move {
            tokio::time::sleep(SCROLLBAR_AUTO_HIDE_DURATION).await;
            generation
        },
        elapsed,
    )
}

fn advance_reveal<R>(
    scrollbar: &mut ScrollbarStateMachine<R>,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = elapsed_fraction(started_at, SCROLLBAR_REVEAL_DURATION);
    if progress >= 1.0 {
        scrollbar.visibility = ScrollbarVisibility::Visible;
        scrollbar.animation = None;
        return true;
    }

    let opacity = initial_opacity + (1.0 - initial_opacity) * ease_out_cubic(progress);
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn advance_hide<R>(
    scrollbar: &mut ScrollbarStateMachine<R>,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = elapsed_fraction(started_at, SCROLLBAR_HIDE_DURATION);
    if progress >= 1.0 {
        return false;
    }

    let opacity = initial_opacity * (1.0 - smoothstep(progress));
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn elapsed_fraction(started_at: Instant, duration: Duration) -> f32 {
    (started_at.elapsed().as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

// 与主软件动画模块同式的缓动：视觉节奏保持一致，不为单个纯函数
// 把 app-ui 的动画模块拖进依赖。
fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    progress * progress * (3.0 - 2.0 * progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Machine = ScrollbarStateMachine<u8>;

    fn overflowing_viewport() -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 400.0,
            viewport_height: 300.0,
            content_width: 400.0,
            content_height: 900.0,
        }
    }

    #[test]
    fn reveal_and_hide_curves_keep_endpoints() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(smoothstep(0.0), 0.0);
        assert!((smoothstep(0.5) - 0.5).abs() <= f32::EPSILON);
        assert_eq!(smoothstep(1.0), 1.0);
    }

    #[test]
    fn visibility_follows_active_region_and_reveal_ends_visible() {
        let mut machine = Machine::default();
        let _ = machine.start_reveal(1);
        assert!(machine.visibility_for(&1).opacity() > 0.0);
        // 非活跃区域恒为隐藏。
        assert_eq!(machine.visibility_for(&2), ScrollbarVisibility::Hidden);

        machine.advance_animation();
        std::thread::sleep(SCROLLBAR_REVEAL_DURATION + Duration::from_millis(5));
        machine.advance_animation();
        assert_eq!(machine.visibility_for(&1), ScrollbarVisibility::Visible);
        assert!(!machine.is_animating());
    }

    #[test]
    fn stale_auto_hide_generation_is_ignored() {
        let mut machine = Machine::default();
        let _ = machine.start_reveal(1);
        let generation = machine.start_reveal(1);

        // 旧代数回信：必须被丢弃，透明度保持。
        machine.start_hide(generation - 1);
        assert!(machine.visibility_for(&1).opacity() > 0.0);

        // 当前代数回信：进入淡出。
        machine.start_hide(generation);
        assert!(machine.is_animating());
    }

    #[test]
    fn hide_runs_to_completion_and_resets_state() {
        let mut machine = Machine::default();
        let generation = machine.start_reveal(1);

        machine.start_hide(generation);
        std::thread::sleep(SCROLLBAR_HIDE_DURATION + Duration::from_millis(5));
        machine.advance_animation();

        assert_eq!(machine.visibility_for(&1), ScrollbarVisibility::Hidden);
        assert!(!machine.is_animating());
    }

    #[test]
    fn viewport_cache_and_reset() {
        let mut machine = Machine::default();
        machine.remember_viewport(1, overflowing_viewport());
        assert_eq!(machine.viewport_for(&1), Some(overflowing_viewport()));
        machine.invalidate_viewport(1);
        assert_eq!(machine.viewport_for(&1), None);

        machine.remember_viewport(2, overflowing_viewport());
        let _ = machine.start_reveal(2);
        machine.reset();
        assert_eq!(machine.viewport_for(&2), None);
        assert_eq!(machine.visibility_for(&2), ScrollbarVisibility::Hidden);
        assert!(!machine.is_animating());
    }
}
