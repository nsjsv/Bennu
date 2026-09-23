//! 预览窗口机械：窗口设置、尺寸档位常量与开/关/适配状态机，自 app-ui
//! 的 app/windows.rs 预览分支迁入（iced window Task 与宿主无关，任务
//! design.md「窗口命令由引擎直接产出」）。宿主侧窗口管理（焦点簿记、
//! 最大化观测、滚动条视口回收、Escape 路由）留在宿主转发层。

use iced::window::{self, Id};
use iced::{Size, Task};

use super::PreviewEngine;
use crate::preview::{
    PreviewContent, PreviewSize, PreviewState, PreviewWindowProfile,
    PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION,
};
use crate::preview_message::PreviewMessage;

const DEFAULT_PREVIEW_WIDTH: f32 = 720.0;
const DEFAULT_PREVIEW_HEIGHT: f32 = 900.0;
const MIN_PREVIEW_WIDTH: f32 = 420.0;
const MIN_PREVIEW_HEIGHT: f32 = 260.0;
const DEFAULT_IMAGE_PREVIEW_WIDTH: f32 = 748.0;
const DEFAULT_IMAGE_PREVIEW_HEIGHT: f32 = 636.0;
const MIN_IMAGE_PREVIEW_WIDTH: f32 = 360.0;
const MIN_IMAGE_PREVIEW_HEIGHT: f32 = 260.0;
const IMAGE_PREVIEW_INITIAL_FIT_MAX_WIDTH: f32 = 1080.0;
const IMAGE_PREVIEW_INITIAL_FIT_MAX_HEIGHT: f32 = 940.0;
const DEFAULT_AUDIO_PREVIEW_WIDTH: f32 = 780.0;
const DEFAULT_AUDIO_PREVIEW_HEIGHT: f32 = 168.0;
const MIN_AUDIO_PREVIEW_WIDTH: f32 = 560.0;
const MIN_AUDIO_PREVIEW_HEIGHT: f32 = 136.0;
const DEFAULT_VIDEO_PREVIEW_WIDTH: f32 = 748.0;
const DEFAULT_VIDEO_PREVIEW_HEIGHT: f32 = 501.0;
const MIN_VIDEO_PREVIEW_WIDTH: f32 = 360.0;
const MIN_VIDEO_PREVIEW_HEIGHT: f32 = 320.0;
const VIDEO_PREVIEW_INITIAL_FIT_MAX_WIDTH: f32 = 1080.0;
const VIDEO_PREVIEW_INITIAL_FIT_MAX_HEIGHT: f32 = 940.0;
const PREVIEW_RESIZE_MATCH_TOLERANCE: f32 = 1.0;

/// 预览窗口的宿主进程身份：开窗 Settings 需要 app id 与图标，二者是
/// 宿主进程属性（app-ui 与 portal 各自不同），由宿主构造引擎时注入。
#[derive(Debug, Clone)]
pub struct PreviewWindowIdentity {
    pub app_id: String,
    pub icon: Option<window::Icon>,
}

pub fn preview_window_settings(
    profile: PreviewWindowProfile,
    size: PreviewSize,
    identity: &PreviewWindowIdentity,
) -> window::Settings {
    let size = clamp_preview_size_to_minimum(profile, size);
    let min_size = preview_min_size(profile);
    let mut settings = window::Settings {
        size: preview_window_size_for_content(size),
        min_size: Some(preview_window_size_for_content(PreviewSize {
            width: min_size.width,
            height: min_size.height,
        })),
        decorations: false,
        exit_on_close_request: true,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = identity.app_id.clone();
    settings.icon = identity.icon.clone();
    settings
}

pub fn preview_window_size_for_content(content_size: PreviewSize) -> Size {
    Size::new(content_size.width, content_size.height)
}

pub fn preview_content_size_from_window(
    profile: PreviewWindowProfile,
    width: f32,
    height: f32,
) -> PreviewSize {
    clamp_preview_size_to_minimum(profile, PreviewSize { width, height })
}

pub fn default_preview_size(profile: PreviewWindowProfile) -> PreviewSize {
    match profile {
        PreviewWindowProfile::Regular => PreviewSize {
            width: DEFAULT_PREVIEW_WIDTH,
            height: DEFAULT_PREVIEW_HEIGHT,
        },
        PreviewWindowProfile::Image => PreviewSize {
            width: DEFAULT_IMAGE_PREVIEW_WIDTH,
            height: DEFAULT_IMAGE_PREVIEW_HEIGHT,
        },
        PreviewWindowProfile::Audio => PreviewSize {
            width: DEFAULT_AUDIO_PREVIEW_WIDTH,
            height: DEFAULT_AUDIO_PREVIEW_HEIGHT,
        },
        PreviewWindowProfile::Video => PreviewSize {
            width: DEFAULT_VIDEO_PREVIEW_WIDTH,
            height: DEFAULT_VIDEO_PREVIEW_HEIGHT,
        },
    }
}

pub fn clamp_preview_size_to_minimum(
    profile: PreviewWindowProfile,
    size: PreviewSize,
) -> PreviewSize {
    let min_size = preview_min_size(profile);
    PreviewSize {
        width: size.width.max(min_size.width),
        height: size.height.max(min_size.height),
    }
}

pub fn preview_min_size(profile: PreviewWindowProfile) -> Size {
    match profile {
        PreviewWindowProfile::Regular => Size::new(MIN_PREVIEW_WIDTH, MIN_PREVIEW_HEIGHT),
        PreviewWindowProfile::Image => Size::new(MIN_IMAGE_PREVIEW_WIDTH, MIN_IMAGE_PREVIEW_HEIGHT),
        PreviewWindowProfile::Audio => Size::new(MIN_AUDIO_PREVIEW_WIDTH, MIN_AUDIO_PREVIEW_HEIGHT),
        PreviewWindowProfile::Video => Size::new(MIN_VIDEO_PREVIEW_WIDTH, MIN_VIDEO_PREVIEW_HEIGHT),
    }
}

pub fn image_preview_initial_fit_max_size() -> Size {
    Size::new(
        IMAGE_PREVIEW_INITIAL_FIT_MAX_WIDTH,
        IMAGE_PREVIEW_INITIAL_FIT_MAX_HEIGHT,
    )
}

pub fn video_preview_initial_fit_max_size() -> Size {
    Size::new(
        VIDEO_PREVIEW_INITIAL_FIT_MAX_WIDTH,
        VIDEO_PREVIEW_INITIAL_FIT_MAX_HEIGHT,
    )
}

pub fn image_preview_size_from_dimensions(width: u32, height: u32) -> PreviewSize {
    let max_size = image_preview_initial_fit_max_size();
    let image_width = width as f32;
    let image_height = height as f32;
    let scale = (max_size.width / image_width)
        .min(max_size.height / image_height)
        .min(1.0);

    PreviewSize {
        width: image_width * scale,
        height: image_height * scale,
    }
}

pub fn animated_image_preview_size_from_dimensions(width: u32, height: u32) -> PreviewSize {
    let min_size = preview_min_size(PreviewWindowProfile::Image);
    let image_width = width as f32;
    let image_height = height as f32;
    let scale_to_maximum = (IMAGE_PREVIEW_INITIAL_FIT_MAX_WIDTH / image_width)
        .min(IMAGE_PREVIEW_INITIAL_FIT_MAX_HEIGHT / image_height);
    let scale_to_minimum = (min_size.width / image_width).max(min_size.height / image_height);
    if scale_to_minimum > scale_to_maximum {
        return PreviewSize {
            width: min_size.width,
            height: min_size.height,
        };
    }

    let scale = scale_to_minimum.max(scale_to_maximum.min(1.0));
    PreviewSize {
        width: image_width * scale,
        height: image_height * scale,
    }
}

pub fn video_preview_size_from_frame(width: u32, height: u32) -> PreviewSize {
    let max_size = video_preview_initial_fit_max_size();
    let frame_width = width as f32;
    let frame_height = height as f32;
    let scale = (max_size.width / frame_width)
        .min(max_size.height / frame_height)
        .min(1.0);

    PreviewSize {
        width: frame_width * scale,
        height: frame_height * scale,
    }
}

pub fn preview_size_matches(actual: PreviewSize, expected: PreviewSize) -> bool {
    (actual.width - expected.width).abs() <= PREVIEW_RESIZE_MATCH_TOLERANCE
        && (actual.height - expected.height).abs() <= PREVIEW_RESIZE_MATCH_TOLERANCE
}

fn preview_window_initial_chrome_command(generation: u64) -> Task<PreviewMessage> {
    Task::perform(
        async move {
            tokio::time::sleep(PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION).await;
            generation
        },
        PreviewMessage::PreviewWindowInitialChromeElapsed,
    )
}

impl PreviewEngine {
    /// 呈现/聚焦预览窗口。返回窗口 id 供宿主完成焦点簿记（引擎不持有
    /// 宿主焦点状态，同时登记 pending_focus 供引擎内部路径出口同步）。
    pub fn ensure_preview_window(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> (Id, Task<PreviewMessage>) {
        let size = clamp_preview_size_to_minimum(profile, default_preview_size(profile));
        self.preview_window_profile = profile;
        self.preview_size = size;
        self.pending_preview_resize = Some(size);

        if let Some(window) = self.preview_window {
            self.reset_preview_window_bottom_controls();
            self.pending_focus_window = Some(window);
            let initial_chrome = self.start_preview_window_initial_chrome();
            let min_size = preview_min_size(profile);
            let resize = window::set_min_size(
                window,
                Some(preview_window_size_for_content(PreviewSize {
                    width: min_size.width,
                    height: min_size.height,
                })),
            )
            .chain(window::resize(
                window,
                preview_window_size_for_content(size),
            ));
            return (
                window,
                Task::batch([resize, window::gain_focus(window), initial_chrome]),
            );
        }

        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.reset_preview_window_bottom_controls();
        let initial_chrome = self.start_preview_window_initial_chrome();
        let (window, command) = window::open(preview_window_settings(
            profile,
            size,
            &self.window_identity,
        ));
        self.preview_window = Some(window);
        self.pending_focus_window = Some(window);
        (window, Task::batch([command.discard(), initial_chrome]))
    }

    pub fn toggle_preview_window_pin(&mut self) -> Task<PreviewMessage> {
        self.preview_window_pinned = !self.preview_window_pinned;
        Task::none()
    }

    pub fn open_image_preview_window_for_dimensions(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<PreviewMessage> {
        if width == 0 || height == 0 {
            return self.open_image_preview_error_window();
        }
        self.update_preview_window(
            PreviewWindowProfile::Image,
            image_preview_size_from_dimensions(width, height),
        )
        .1
    }

    pub fn open_animated_image_preview_window_for_dimensions(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<PreviewMessage> {
        self.update_preview_window(
            PreviewWindowProfile::Image,
            animated_image_preview_size_from_dimensions(width, height),
        )
        .1
    }

    pub fn open_image_preview_window_with_default_size(&mut self) -> Task<PreviewMessage> {
        self.update_preview_window(
            PreviewWindowProfile::Image,
            default_preview_size(PreviewWindowProfile::Image),
        )
        .1
    }

    pub fn open_image_preview_error_window(&mut self) -> Task<PreviewMessage> {
        self.open_image_preview_window_with_default_size()
    }

    fn update_preview_window(
        &mut self,
        profile: PreviewWindowProfile,
        size: PreviewSize,
    ) -> (Id, Task<PreviewMessage>) {
        self.preview_window_profile = profile;
        self.preview_size = clamp_preview_size_to_minimum(profile, size);
        self.pending_preview_resize = Some(self.preview_size);

        if let Some(window) = self.preview_window {
            self.reset_preview_window_bottom_controls();
            self.pending_focus_window = Some(window);
            let min_size = preview_min_size(profile);
            let initial_chrome = self.start_preview_window_initial_chrome();
            let resize = window::set_min_size(
                window,
                Some(preview_window_size_for_content(PreviewSize {
                    width: min_size.width,
                    height: min_size.height,
                })),
            )
            .chain(window::resize(
                window,
                preview_window_size_for_content(self.preview_size),
            ));
            return (
                window,
                Task::batch([resize, window::gain_focus(window), initial_chrome]),
            );
        }

        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.reset_preview_window_bottom_controls();
        let initial_chrome = self.start_preview_window_initial_chrome();
        let (window, command) = window::open(preview_window_settings(
            profile,
            self.preview_size,
            &self.window_identity,
        ));
        self.preview_window = Some(window);
        self.pending_focus_window = Some(window);
        (window, Task::batch([command.discard(), initial_chrome]))
    }

    fn preview_window_has_bottom_media_controls(&self) -> bool {
        match self.preview.as_ref() {
            Some(PreviewState::Ready(PreviewContent::Video { .. })) => true,
            Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) => {
                preview.playback_duration().is_some()
            }
            _ => false,
        }
    }

    fn reset_preview_window_bottom_controls(&mut self) {
        self.preview_window_bottom_controls.reset_hidden();
        self.refresh_preview_window_bottom_controls();
    }

    pub fn refresh_preview_window_bottom_controls(&mut self) {
        if !self.preview_window_has_bottom_media_controls() {
            self.preview_window_bottom_controls.reset_hidden();
            return;
        }

        if let Some(pointer_y) = self.preview_window_pointer_y {
            let panel_height = self.preview_size.height;
            self.preview_window_bottom_controls
                .update_for_bottom_cursor_y(pointer_y, panel_height);
        }
    }

    pub fn fit_preview_window_to_video_frame(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<PreviewMessage> {
        if width == 0 || height == 0 {
            return Task::none();
        }

        self.update_preview_window(
            PreviewWindowProfile::Video,
            video_preview_size_from_frame(width, height),
        )
        .1
    }

    // 文本与 SQLite 预览使用标准窗口壳（标题栏 + 控制按钮）；媒体类内容保持悬浮 chrome。
    pub fn preview_window_uses_window_chrome(&self) -> bool {
        matches!(
            self.preview,
            Some(PreviewState::Ready(PreviewContent::Text { .. }))
                | Some(PreviewState::Ready(PreviewContent::Sqlite(_)))
        )
    }

    pub fn start_preview_window_initial_chrome(&mut self) -> Task<PreviewMessage> {
        if self.preview_window_uses_window_chrome() {
            return Task::none();
        }

        self.cancel_preview_window_initial_chrome_hide();
        self.preview_window_chrome.start_reveal();
        if self.preview_window_has_bottom_media_controls() {
            self.preview_window_bottom_controls.start_reveal();
        } else {
            self.preview_window_bottom_controls.reset_hidden();
        }
        preview_window_initial_chrome_command(self.preview_window_initial_chrome_generation)
    }

    pub fn cancel_preview_window_initial_chrome_hide(&mut self) {
        self.preview_window_initial_chrome_generation = self
            .preview_window_initial_chrome_generation
            .wrapping_add(1);
    }

    pub fn hide_preview_window_initial_chrome(&mut self, generation: u64) {
        if generation != self.preview_window_initial_chrome_generation {
            return;
        }
        if self.preview_window_uses_window_chrome() {
            return;
        }
        self.preview_window_chrome.start_hide();
        if !self.preview_window_has_bottom_media_controls() {
            self.preview_window_bottom_controls.reset_hidden();
        } else if self.preview_window_pointer_y.is_some() {
            self.refresh_preview_window_bottom_controls();
        } else {
            self.preview_window_bottom_controls.start_hide();
        }
    }

    /// 关闭预览窗口会话：复位固定/清理内容/复位 chrome 后取走窗口。
    /// 返回被关闭的窗口 id（None = 本就没有窗口，状态复位照常执行），
    /// 宿主用它完成焦点与滚动条视口的收尾。
    pub fn close_preview_window(&mut self) -> (Option<Id>, Task<PreviewMessage>) {
        // 先复位固定，保证 clear_preview 的固定守卫不会跳过内容清理。
        self.preview_window_pinned = false;
        self.preview_shown_path = None;
        self.clear_preview();
        self.pending_preview_resize = None;
        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.cancel_preview_window_initial_chrome_hide();
        self.preview_window_chrome.reset_hidden();
        self.preview_window_bottom_controls.reset_hidden();
        let Some(window) = self.preview_window.take() else {
            return (None, Task::none());
        };
        (Some(window), window::close(window))
    }

    /// 失焦自动关闭的判定：预览窗口失焦且未被钉住时按 Space 语义关闭。
    pub fn should_close_unfocused_window(&self, window: Id) -> bool {
        self.preview_window == Some(window) && !self.preview_window_pinned
    }

    /// 加载管线中的窗口呈现步骤：独立窗口会话照常确保/聚焦窗口，
    /// 面板会话原样跳过，窗口保持关闭、不缩放、不抢焦点。
    pub fn preview_window_presentation_command(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> Task<PreviewMessage> {
        match self.preview_load_surface {
            super::PreviewLoadSurface::StandaloneWindow => self.ensure_preview_window(profile).1,
            super::PreviewLoadSurface::RightDockedPanel => Task::none(),
        }
    }

    /// 与 [`Self::preview_window_presentation_command`] 同一边界的异步回流
    /// 形态：窗口缺失时补开，仅对独立窗口会话生效。
    pub fn ensure_preview_window_for_standalone_load(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> Task<PreviewMessage> {
        if self.preview_load_surface == super::PreviewLoadSurface::StandaloneWindow
            && self.preview_window.is_none()
        {
            return self.ensure_preview_window(profile).1;
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION;
    use iced::futures::StreamExt;

    #[tokio::test]
    async fn preview_window_initial_controls_command_waits_for_configured_duration() {
        let mut task = iced_runtime::task::into_stream(preview_window_initial_chrome_command(7))
            .expect("initial controls task stream");
        assert!(
            tokio::time::timeout(PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION / 2, task.next())
                .await
                .is_err()
        );
        let message = tokio::time::timeout(
            PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION + std::time::Duration::from_millis(100),
            task.next(),
        )
        .await
        .expect("initial controls timer timed out")
        .expect("initial controls task ended");
        assert!(matches!(
            message,
            iced_runtime::Action::Output(PreviewMessage::PreviewWindowInitialChromeElapsed(7))
        ));
    }
}
