//! 右侧面板的视频全屏：主窗口全屏、只显示这个视频（同一个播放会话，退出接着原来的状态）。
//! 引擎记「要不要全屏」（`PreviewEngine::video_fullscreen`），主窗口归这里：update 出口收敛一次，
//! 把主窗口模式跟过去。

use bennu_preview::engine::video_fullscreen::VideoFullscreen;
use bennu_preview::preview::PreviewSize;
use iced::keyboard::{self, key};
use iced::{window, Element, Task};

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    /// 内容已经不是视频了（换了选中、预览被清掉）就连引擎的状态一起退出全屏。
    pub(super) fn sync_main_window_video_fullscreen(&mut self) -> Task<Message> {
        let engine = &mut self.preview_engine;
        let requested = engine.video_fullscreen == Some(VideoFullscreen::MainWindow);
        let showing_video = engine.video_preview.is_some();
        if requested && !showing_video {
            engine.video_fullscreen = None;
        }
        let wanted = requested && showing_video;
        if wanted == self.main_window_video_fullscreen {
            return Task::none();
        }
        self.main_window_video_fullscreen = wanted;
        let mode = if wanted {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        window::set_mode(self.main_window, mode)
    }

    /// 视频全屏时的按键：Esc 退出全屏（不关窗口）；视频铺满主窗口时其余键只归视频的按键壳，
    /// 不进文件管理器快捷键（看不见文件列表，Space、方向键不该去动它）。
    pub(super) fn video_fullscreen_key(
        &mut self,
        pressed: &keyboard::Key,
    ) -> Option<Task<Message>> {
        let fullscreen = self.preview_engine.video_fullscreen?;
        if matches!(pressed, keyboard::Key::Named(key::Named::Escape)) {
            return Some(
                self.preview_engine
                    .leave_video_fullscreen()
                    .map(Message::Preview),
            );
        }
        (fullscreen == VideoFullscreen::MainWindow).then(Task::none)
    }

    /// 主窗口全屏时的视图：只有视频，铺满整个窗口。
    pub(super) fn main_window_video_fullscreen_view(&self) -> Element<'_, Message> {
        self.preview_window_content(
            PreviewSize {
                width: self.main_window_width,
                height: self.main_window_height,
            },
            self.preview_engine.preview_window_bottom_controls.opacity(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bennu_preview::preview::VideoPreviewPlayback;
    use iced::keyboard::{self, key};

    use super::*;
    use crate::config;

    fn browser_playing_in_the_panel() -> FileBrowser {
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.preview_engine.video_preview = Some(VideoPreviewPlayback::ready(
            PathBuf::from("/videos/clip.mkv"),
            None,
            false,
        ));
        browser.preview_engine.video_fullscreen = Some(VideoFullscreen::MainWindow);
        browser
    }

    #[test]
    fn main_window_follows_the_engine_and_drops_fullscreen_without_a_video() {
        let mut browser = browser_playing_in_the_panel();
        drop(browser.sync_main_window_video_fullscreen());
        assert!(browser.main_window_video_fullscreen);

        browser.preview_engine.video_preview = None;
        drop(browser.sync_main_window_video_fullscreen());
        assert!(!browser.main_window_video_fullscreen);
        assert_eq!(browser.preview_engine.video_fullscreen, None);
    }

    #[test]
    fn escape_leaves_fullscreen_and_other_keys_stay_with_the_video() {
        let mut browser = browser_playing_in_the_panel();
        assert!(browser
            .video_fullscreen_key(&keyboard::Key::Named(key::Named::Space))
            .is_some());
        assert_eq!(
            browser.preview_engine.video_fullscreen,
            Some(VideoFullscreen::MainWindow)
        );
        assert!(browser
            .video_fullscreen_key(&keyboard::Key::Named(key::Named::Escape))
            .is_some());
        assert_eq!(browser.preview_engine.video_fullscreen, None);
        // 不在全屏：照常交给文件管理器快捷键。
        assert!(browser
            .video_fullscreen_key(&keyboard::Key::Named(key::Named::Escape))
            .is_none());
    }
}
