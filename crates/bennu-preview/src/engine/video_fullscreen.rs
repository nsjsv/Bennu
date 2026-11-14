//! 视频全屏。独立预览窗口（含文件选择对话框）是窗口自己切全屏；右侧面板的视频是主窗口
//! 切全屏、只显示这个视频（同一个播放会话，退出时接着原来的状态，不用另开窗口搬状态）。
//! 主窗口归宿主：引擎只记「要全屏」，宿主按它切窗口模式、换视图。
//!
//! 全屏时控件不用移到底部，动一下鼠标就出现，2 秒不动连同鼠标一起藏起来；拖进度条、开着
//! 菜单时不藏。

use std::time::{Duration, Instant};

use iced::window;
use iced::Task;

use super::PreviewLoadSurface;
use crate::preview_message::PreviewMessage;
use crate::text_preview_viewer::focus_preview_keyboard;

/// 全屏时鼠标不动多久藏控件和鼠标。
const FULLSCREEN_IDLE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoFullscreen {
    PreviewWindow,
    /// 右侧面板的视频铺满主窗口；宿主据此切主窗口模式和视图。
    MainWindow,
}

/// 全屏时「鼠标多久没动」：每次移动只推后截止时间，同一时间最多一个计时任务在等。
#[derive(Debug, Default)]
pub struct FullscreenIdle {
    deadline: Option<Instant>,
    timer_pending: bool,
}

impl super::PreviewEngine {
    /// F 键、双击画面。
    pub fn toggle_video_fullscreen(&mut self) -> Task<PreviewMessage> {
        if self.video_fullscreen.is_some() {
            return self.leave_video_fullscreen();
        }
        if self.video_preview.is_none() {
            return Task::none();
        }
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => {
                let Some(preview_window) = self.preview_window else {
                    return Task::none();
                };
                self.video_fullscreen = Some(VideoFullscreen::PreviewWindow);
                Task::batch([
                    window::set_mode(preview_window, window::Mode::Fullscreen),
                    self.show_fullscreen_controls(),
                ])
            }
            PreviewLoadSurface::RightDockedPanel => {
                self.video_fullscreen = Some(VideoFullscreen::MainWindow);
                // 视图整个换了，按键壳要重新聚焦，F/方向键才接着归视频。
                Task::batch([self.show_fullscreen_controls(), focus_preview_keyboard()])
            }
        }
    }

    /// F、Esc、双击退出全屏；没在全屏时什么都不做（宿主的 Esc 照常关窗口）。
    pub fn leave_video_fullscreen(&mut self) -> Task<PreviewMessage> {
        let Some(fullscreen) = self.video_fullscreen.take() else {
            return Task::none();
        };
        self.fullscreen_idle = FullscreenIdle::default();
        if let Some(playback) = self.video_preview.as_mut() {
            playback.cursor_hidden = false;
        }
        self.preview_window_bottom_controls.reset_hidden();
        match fullscreen {
            VideoFullscreen::PreviewWindow => match self.preview_window {
                Some(preview_window) => {
                    self.refresh_preview_window_bottom_controls();
                    window::set_mode(preview_window, window::Mode::Windowed)
                }
                None => Task::none(),
            },
            VideoFullscreen::MainWindow => focus_preview_keyboard(),
        }
    }

    /// 预览画面上的指针移动（独立预览窗口，或铺满主窗口的全屏视频）：平时控件「移到底部才出现」，
    /// 全屏时动一下就出现。
    pub fn preview_pointer_moved(&mut self, pointer_y: f32) -> Task<PreviewMessage> {
        self.cancel_preview_window_initial_controls_hide();
        self.preview_window_pointer_y = Some(pointer_y);
        if self.video_fullscreen.is_some() {
            return self.show_fullscreen_controls();
        }
        self.refresh_preview_window_bottom_controls();
        Task::none()
    }

    /// 计时到了：鼠标这期间又动过就按新的截止时间再等，否则藏控件和鼠标。
    pub fn video_fullscreen_idle_elapsed(&mut self) -> Task<PreviewMessage> {
        self.fullscreen_idle.timer_pending = false;
        if self.video_fullscreen.is_none() {
            return Task::none();
        }
        let Some(deadline) = self.fullscreen_idle.deadline else {
            return Task::none();
        };
        let now = Instant::now();
        if now < deadline {
            return self.schedule_fullscreen_idle(deadline - now);
        }
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.seek_drag.is_some() || playback.track_menu.is_some() {
            return Task::none();
        }
        playback.cursor_hidden = true;
        self.preview_window_bottom_controls.start_hide();
        Task::none()
    }

    fn show_fullscreen_controls(&mut self) -> Task<PreviewMessage> {
        if let Some(playback) = self.video_preview.as_mut() {
            playback.cursor_hidden = false;
        }
        if !self.preview_window_bottom_controls.target_is_visible() {
            self.preview_window_bottom_controls.start_reveal();
        }
        self.fullscreen_idle.deadline = Some(Instant::now() + FULLSCREEN_IDLE);
        if self.fullscreen_idle.timer_pending {
            return Task::none();
        }
        self.schedule_fullscreen_idle(FULLSCREEN_IDLE)
    }

    fn schedule_fullscreen_idle(&mut self, after: Duration) -> Task<PreviewMessage> {
        self.fullscreen_idle.timer_pending = true;
        Task::perform(tokio::time::sleep(after), |()| {
            PreviewMessage::VideoFullscreenIdleElapsed
        })
    }
}

#[cfg(test)]
#[path = "video_fullscreen_tests.rs"]
mod tests;
