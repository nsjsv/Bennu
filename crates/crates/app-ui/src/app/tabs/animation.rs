use std::time::{Duration, Instant};

use crate::animation::{ease_out_cubic, elapsed_fraction as animation_progress};

const TAB_BAR_REVEAL_DURATION: Duration = Duration::from_millis(180);
const TAB_BAR_HIDE_DURATION: Duration = Duration::from_millis(140);
const TAB_INTRO_DURATION: Duration = Duration::from_millis(180);
const TAB_CLOSE_DURATION: Duration = Duration::from_millis(150);
const TAB_REORDER_DURATION: Duration = Duration::from_millis(160);
pub(super) const TAB_REORDER_HORIZONTAL_PADDING: f32 = 36.0;
pub(super) const TAB_REORDER_SPACING: f32 = 6.0;
pub(super) const TAB_REORDER_MIN_SLOT_WIDTH: f32 = 48.0;

#[derive(Debug, Clone, Copy)]
pub(crate) enum TabBarReveal {
    Hidden,
    Opening {
        started_at: Instant,
        initial_fraction: f32,
    },
    Visible,
    Closing {
        started_at: Instant,
        initial_fraction: f32,
    },
}

impl Default for TabBarReveal {
    fn default() -> Self {
        Self::Hidden
    }
}

impl TabBarReveal {
    pub(super) fn is_animating(self) -> bool {
        matches!(self, Self::Opening { .. } | Self::Closing { .. })
    }

    pub(super) fn fraction(self) -> f32 {
        match self {
            Self::Hidden => 0.0,
            Self::Visible => 1.0,
            Self::Opening {
                started_at,
                initial_fraction,
            } => {
                let progress = animation_progress(started_at, TAB_BAR_REVEAL_DURATION);
                initial_fraction + (1.0 - initial_fraction) * ease_out_cubic(progress)
            }
            Self::Closing {
                started_at,
                initial_fraction,
            } => {
                let progress = animation_progress(started_at, TAB_BAR_HIDE_DURATION);
                initial_fraction * (1.0 - ease_out_cubic(progress))
            }
        }
        .clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TabAnimationState {
    intro_started_at: Option<Instant>,
    close: Option<TabCloseAnimation>,
    shift: Option<TabShiftAnimation>,
}

impl TabAnimationState {
    pub(crate) fn width_fraction(self) -> f32 {
        if let Some(close) = self.close {
            let progress = ease_out_cubic(animation_progress(close.started_at, TAB_CLOSE_DURATION));
            return (close.initial_fraction * (1.0 - progress)).clamp(0.0, 1.0);
        }

        self.intro_fraction()
    }

    fn intro_fraction(self) -> f32 {
        let Some(started_at) = self.intro_started_at else {
            return 1.0;
        };
        ease_out_cubic(animation_progress(started_at, TAB_INTRO_DURATION))
    }

    pub(crate) fn shift_offset(self) -> f32 {
        let Some(shift) = self.shift else {
            return 0.0;
        };
        let progress = ease_out_cubic(animation_progress(shift.started_at, TAB_REORDER_DURATION));
        shift.initial_offset * (1.0 - progress)
    }

    pub(super) fn is_animating(self) -> bool {
        self.intro_started_at
            .is_some_and(|started_at| animation_progress(started_at, TAB_INTRO_DURATION) < 1.0)
            || self
                .close
                .is_some_and(|close| animation_progress(close.started_at, TAB_CLOSE_DURATION) < 1.0)
            || self.shift.is_some_and(|shift| {
                animation_progress(shift.started_at, TAB_REORDER_DURATION) < 1.0
            })
    }

    pub(super) fn is_closing(self) -> bool {
        self.close.is_some()
    }

    pub(super) fn close_is_finished(self) -> bool {
        self.close
            .is_some_and(|close| animation_progress(close.started_at, TAB_CLOSE_DURATION) >= 1.0)
    }

    pub(super) fn start_intro(&mut self) {
        self.intro_started_at = Some(Instant::now());
    }

    pub(super) fn start_close(&mut self, initial_fraction: f32) {
        self.close = Some(TabCloseAnimation {
            started_at: Instant::now(),
            initial_fraction,
        });
    }

    pub(super) fn start_shift(&mut self, initial_offset: f32) {
        self.shift = Some(TabShiftAnimation {
            started_at: Instant::now(),
            initial_offset,
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct TabCloseAnimation {
    started_at: Instant,
    initial_fraction: f32,
}

#[derive(Debug, Clone, Copy)]
struct TabShiftAnimation {
    started_at: Instant,
    initial_offset: f32,
}
