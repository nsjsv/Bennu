//! 预览画面上宿主要把指针的哪些移动作为消息交过来。
//!
//! 每条消息都要重建、重画所有窗口。大多数时候指针移动什么都不改变，宿主只记位置和时刻；只有底部
//! 控制条要按指针在不在底部那一条显隐、全屏时藏着的控件要动一下就出现，才需要消息。

use bennu_theme::pointer_motion::PointerMotion;

use super::video_fullscreen::VideoFullscreen;
use crate::preview::PreviewBottomControlsState;
use crate::window_shell::HEADER_BAR_HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerInterest {
    /// 没有跟着指针走的东西（文本、表格这类没有底部控制条的预览）。
    Nothing,
    /// 第一次移动、以及进出这一条（窗口坐标的纵向范围）时：底部控制条移到这里才出现。
    Band { top: f32, bottom: f32 },
    /// 每次移动：全屏时控件或光标藏着，动一下就出现。
    Moves,
}

impl super::PreviewEngine {
    /// 宿主窗口根部的指针记录器写入这里，全屏控件到点时按它顺延（见 `video_fullscreen_idle_elapsed`）。
    pub fn share_pointer_motion(&mut self, motion: PointerMotion) {
        self.pointer_motion = motion;
    }

    pub(super) fn last_pointer_motion(&self) -> Option<std::time::Instant> {
        self.pointer_motion.last()
    }

    /// 独立预览窗口要的指针移动。
    pub fn preview_window_pointer_interest(&self) -> PointerInterest {
        if self.video_fullscreen == Some(VideoFullscreen::PreviewWindow) {
            return self.fullscreen_pointer_interest();
        }
        if !self.preview_window_has_bottom_media_controls() {
            return PointerInterest::Nothing;
        }
        // 与 `update_for_bottom_cursor_y` 同一口径：内容区底部 `REVEAL_HEIGHT` 那一条，换算到窗口坐标。
        let panel_height = self.preview_size.height;
        PointerInterest::Band {
            top: HEADER_BAR_HEIGHT
                + (panel_height - PreviewBottomControlsState::REVEAL_HEIGHT).max(0.0),
            bottom: HEADER_BAR_HEIGHT + panel_height,
        }
    }

    /// 主窗口要的指针移动：只有右侧面板的视频铺满主窗口时。
    pub fn main_window_pointer_interest(&self) -> PointerInterest {
        if self.video_fullscreen == Some(VideoFullscreen::MainWindow) {
            self.fullscreen_pointer_interest()
        } else {
            PointerInterest::Nothing
        }
    }

    fn fullscreen_pointer_interest(&self) -> PointerInterest {
        let cursor_hidden = self
            .video_preview
            .as_ref()
            .is_some_and(|playback| playback.cursor_hidden);
        if cursor_hidden || !self.preview_window_bottom_controls.target_is_visible() {
            PointerInterest::Moves
        } else {
            PointerInterest::Nothing
        }
    }
}
