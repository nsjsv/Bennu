use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::advanced::widget as advanced_widget;
use iced::Task;

use super::runtime::scrollbar_auto_hide_command;
use super::smooth_scroll::smooth_scroll_id;
use super::FileBrowser;
use crate::animation::{ease_out_cubic, elapsed_fraction as scrollbar_animation_progress};
use crate::model::{Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility};

pub(super) const SCROLLBAR_ANIMATION_INTERVAL: Duration = crate::ui_pacing::FRAME_INTERVAL_60HZ;

const SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(96);
const SCROLLBAR_HIDE_DURATION: Duration = Duration::from_millis(300);
const SCROLLBAR_MIN_REVEAL_OPACITY: f32 = 0.12;

// mac 式滚动条视觉层（透明原生 thumb + canvas 浮层拇指 + hover 帧驱动
// 展开动画 + 布局探针 + 视口快照模型）已下沉到共享 crate
// `bennu-theme::scrollbar`（portal-backend 复用同一实现）；这里重导出，
// 调用点保持原名零改动。拇指颜色经共享层内部 `bennu_theme::ui_colors`
// 读取，与主软件 `crate::matugen_theme::ui_colors`（即 `bennu_theme::*`
// 的重导出）同源，视觉零变化。
pub(crate) use bennu_theme::scrollbar::{
    enhanced_scrollbar, enhanced_scrollbar_both, scrollbar_layout_probe,
    scrollbar_viewport_has_overflow, ScrollbarAxis,
};

#[derive(Debug, Clone, Copy)]
enum ScrollbarAnimation {
    Revealing {
        started_at: Instant,
        initial_opacity: f32,
    },
    Hiding {
        started_at: Instant,
        initial_opacity: f32,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ScrollbarState {
    active_region: Option<ScrollbarRegion>,
    visibility: ScrollbarVisibility,
    auto_hide_generation: u64,
    animation: Option<ScrollbarAnimation>,
    viewport_by_region: HashMap<ScrollbarRegion, ScrollbarViewport>,
}

impl Default for ScrollbarState {
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

impl FileBrowser {
    pub(super) fn scrollbar_animation_is_active(&self) -> bool {
        self.scrollbar.animation.is_some()
    }

    pub(crate) fn scrollbar_visibility_for(&self, region: &ScrollbarRegion) -> ScrollbarVisibility {
        if self.scrollbar.active_region.as_ref() == Some(region) {
            self.scrollbar.visibility
        } else {
            ScrollbarVisibility::Hidden
        }
    }

    pub(crate) fn scrollbar_viewport_for(
        &self,
        region: &ScrollbarRegion,
    ) -> Option<ScrollbarViewport> {
        self.scrollbar.viewport_by_region.get(region).copied()
    }

    pub(super) fn remember_scrollbar_viewport(
        &mut self,
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
    ) {
        self.scrollbar.viewport_by_region.insert(region, viewport);
    }

    pub(super) fn forget_scrollbar_viewport(&mut self, region: &ScrollbarRegion) {
        self.scrollbar.viewport_by_region.remove(region);
    }

    // 显示入口保持唯一：先探实时布局再决定是否淡入。iced 在内容塞得下时永不发布
    // on_scroll，缓存里的溢出数据可能过期，"能不能滚"必须以探针读到的当帧布局为准。
    pub(super) fn show_scrollbars_temporarily(&mut self, region: ScrollbarRegion) -> Task<Message> {
        self.verify_scrollbar_layout(region)
    }

    // 只探测不预设显示意图：把当帧布局写回缓存自愈过期数据；是否淡入由
    // 回信核实决定（无溢出只刷新缓存，thumb 随之消失）。视口重钉等让
    // 内容高度骤变的路径用它同步滚动条，不经过 hover 显示入口。
    pub(super) fn verify_scrollbar_layout(&mut self, region: ScrollbarRegion) -> Task<Message> {
        advanced_widget::operate(scrollbar_layout_probe(
            smooth_scroll_id(&region).into(),
            move |viewport| Message::ScrollbarLayoutVerified {
                region: region.clone(),
                viewport,
            },
        ))
    }

    // 探针回信：把当帧布局写回缓存（自愈过期数据），核实溢出后才淡入。
    pub(super) fn handle_scrollbar_layout_verified(
        &mut self,
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
    ) -> Task<Message> {
        self.remember_scrollbar_viewport(region.clone(), viewport);
        if !scrollbar_viewport_has_overflow(viewport) {
            return Task::none();
        }
        self.start_scrollbar_reveal(region)
    }

    pub(super) fn start_scrollbar_reveal(&mut self, region: ScrollbarRegion) -> Task<Message> {
        let scrollbar = &mut self.scrollbar;
        scrollbar.active_region = Some(region);
        scrollbar.auto_hide_generation = scrollbar.auto_hide_generation.wrapping_add(1);

        let current_opacity = scrollbar.visibility.opacity();
        if (1.0 - current_opacity) <= f32::EPSILON {
            scrollbar.visibility = ScrollbarVisibility::Visible;
            scrollbar.animation = None;
        } else if !matches!(
            scrollbar.animation,
            Some(ScrollbarAnimation::Revealing { .. })
        ) {
            let initial_opacity = current_opacity.max(SCROLLBAR_MIN_REVEAL_OPACITY);
            scrollbar.visibility = ScrollbarVisibility::with_opacity(initial_opacity);
            scrollbar.animation = Some(ScrollbarAnimation::Revealing {
                started_at: Instant::now(),
                initial_opacity,
            });
        }

        scrollbar_auto_hide_command(scrollbar.auto_hide_generation)
    }

    pub(super) fn start_global_scrollbar_hide(&mut self, generation: u64) {
        if self.scrollbar.auto_hide_generation != generation {
            return;
        }

        let initial_opacity = self.scrollbar.visibility.opacity();
        if initial_opacity <= f32::EPSILON {
            self.scrollbar.active_region = None;
            self.scrollbar.visibility = ScrollbarVisibility::Hidden;
            self.scrollbar.animation = None;
            return;
        }

        self.scrollbar.animation = Some(ScrollbarAnimation::Hiding {
            started_at: Instant::now(),
            initial_opacity,
        });
    }

    pub(super) fn advance_scrollbar_animation(&mut self) -> Task<Message> {
        let Some(animation) = self.scrollbar.animation else {
            return Task::none();
        };

        let still_active = match animation {
            ScrollbarAnimation::Revealing {
                started_at,
                initial_opacity,
            } => advance_scrollbar_reveal(&mut self.scrollbar, started_at, initial_opacity),
            ScrollbarAnimation::Hiding {
                started_at,
                initial_opacity,
            } => advance_scrollbar_hide(&mut self.scrollbar, started_at, initial_opacity),
        };

        if !still_active {
            self.scrollbar.active_region = None;
            self.scrollbar.visibility = ScrollbarVisibility::Hidden;
            self.scrollbar.animation = None;
        }

        Task::none()
    }
}

pub(crate) fn scrollbar_on_scroll(
    region: ScrollbarRegion,
    event: impl Fn(iced::widget::scrollable::Viewport) -> Message + 'static,
) -> impl Fn(iced::widget::scrollable::Viewport) -> Message {
    move |viewport| {
        let absolute_offset = viewport.absolute_offset();
        let bounds = viewport.bounds();
        let content_bounds = viewport.content_bounds();
        Message::ScrollbarViewportChanged {
            region: region.clone(),
            viewport: ScrollbarViewport {
                offset_x: absolute_offset.x,
                offset_y: absolute_offset.y,
                viewport_width: bounds.width,
                viewport_height: bounds.height,
                content_width: content_bounds.width,
                content_height: content_bounds.height,
            },
            event: Box::new(event(viewport)),
        }
    }
}

fn advance_scrollbar_reveal(
    scrollbar: &mut ScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = scrollbar_animation_progress(started_at, SCROLLBAR_REVEAL_DURATION);
    if progress >= 1.0 {
        scrollbar.visibility = ScrollbarVisibility::Visible;
        scrollbar.animation = None;
        return true;
    }

    let opacity = initial_opacity + (1.0 - initial_opacity) * ease_out_cubic(progress);
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn advance_scrollbar_hide(
    scrollbar: &mut ScrollbarState,
    started_at: Instant,
    initial_opacity: f32,
) -> bool {
    let progress = scrollbar_animation_progress(started_at, SCROLLBAR_HIDE_DURATION);
    if progress >= 1.0 {
        return false;
    }

    let opacity = initial_opacity * (1.0 - smoothstep(progress));
    scrollbar.visibility = ScrollbarVisibility::with_opacity(opacity);
    true
}

fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    progress * progress * (3.0 - 2.0 * progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn reveal_curve_starts_fast_and_finishes_at_one() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!(ease_out_cubic(0.35) > 0.65);
        assert_eq!(ease_out_cubic(1.0), 1.0);
    }

    #[test]
    fn hide_curve_keeps_endpoints_stable() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert!((smoothstep(0.5) - 0.5).abs() <= f32::EPSILON);
        assert_eq!(smoothstep(1.0), 1.0);
    }

    #[test]
    fn scrollbar_visibility_only_applies_to_active_region() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let active_region = ScrollbarRegion::Sidebar;
        let inactive_region = ScrollbarRegion::Settings;

        drop(browser.start_scrollbar_reveal(active_region.clone()));

        assert!(browser.scrollbar_visibility_for(&active_region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_visibility_for(&inactive_region),
            ScrollbarVisibility::Hidden
        );
    }

    #[test]
    fn changing_scrollbar_region_moves_active_visibility() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let first_region = ScrollbarRegion::Sidebar;
        let second_region = ScrollbarRegion::Settings;

        drop(browser.start_scrollbar_reveal(first_region.clone()));
        drop(browser.start_scrollbar_reveal(second_region.clone()));

        assert_eq!(
            browser.scrollbar_visibility_for(&first_region),
            ScrollbarVisibility::Hidden
        );
        assert!(browser.scrollbar_visibility_for(&second_region).opacity() > 0.0);
    }

    #[test]
    fn stale_scrollbar_hide_does_not_hide_region_after_new_scroll() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::Sidebar;

        drop(browser.start_scrollbar_reveal(region.clone()));
        drop(browser.start_scrollbar_reveal(region.clone()));
        browser.start_global_scrollbar_hide(1);

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
    }

    #[test]
    fn scrollbar_hide_uses_global_generation() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.start_scrollbar_reveal(ScrollbarRegion::Sidebar));
        browser.start_global_scrollbar_hide(1);

        assert!(matches!(
            browser.scrollbar.animation,
            Some(ScrollbarAnimation::Hiding { .. })
        ));
    }

    #[test]
    fn icon_grid_uses_global_auto_hide_scrollbar_state() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneIcons(crate::model::BrowserPaneId::PRIMARY);

        drop(browser.start_scrollbar_reveal(region.clone()));

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_visibility_for(&ScrollbarRegion::Sidebar),
            ScrollbarVisibility::Hidden
        );
    }

    fn viewport_for(content_height: f32, viewport_height: f32) -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 800.0,
            viewport_height,
            content_width: 800.0,
            content_height,
        }
    }

    #[test]
    fn verified_viewport_without_overflow_heals_cache_and_skips_reveal() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneList(crate::model::BrowserPaneId::PRIMARY);
        // 预置过期缓存：旧布局曾溢出，真实布局已塞得下。
        browser.remember_scrollbar_viewport(region.clone(), viewport_for(1500.0, 600.0));

        drop(browser.handle_scrollbar_layout_verified(region.clone(), viewport_for(600.0, 600.0)));

        assert_eq!(
            browser.scrollbar_visibility_for(&region),
            ScrollbarVisibility::Hidden
        );
        assert_eq!(
            browser.scrollbar_viewport_for(&region),
            Some(viewport_for(600.0, 600.0))
        );
    }

    #[test]
    fn verified_viewport_with_overflow_reveals_scrollbar() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let region = ScrollbarRegion::PaneList(crate::model::BrowserPaneId::PRIMARY);

        drop(browser.handle_scrollbar_layout_verified(region.clone(), viewport_for(1500.0, 600.0)));

        assert!(browser.scrollbar_visibility_for(&region).opacity() > 0.0);
        assert_eq!(
            browser.scrollbar_viewport_for(&region),
            Some(viewport_for(1500.0, 600.0))
        );
    }
}
