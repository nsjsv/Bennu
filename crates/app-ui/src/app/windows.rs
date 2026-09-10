use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::widget::scrollable;
use iced::{event, mouse, window, Rectangle, Size, Task};

use super::FileBrowser;
use crate::app::runtime::preview_window_initial_chrome_command;
use crate::app::smooth_scroll::smooth_scroll_id;
use crate::model::{
    Message, PreviewContent, PreviewSize, PreviewState, PreviewWindowProfile, ScrollbarRegion,
    SettingsCategory, SettingsSubpage, WINDOW_TOP_BAR_HEIGHT,
};
use crate::view::{address_input_id, rename_input_id};

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
const DEFAULT_SETTINGS_WIDTH: f32 = 760.0;
const DEFAULT_SETTINGS_HEIGHT: f32 = 560.0;
const MIN_SETTINGS_WIDTH: f32 = 640.0;
const MIN_SETTINGS_HEIGHT: f32 = 420.0;
const DEFAULT_PROPERTIES_WIDTH: f32 = 760.0;
const DEFAULT_PROPERTIES_HEIGHT: f32 = 560.0;
const MIN_PROPERTIES_WIDTH: f32 = 680.0;
const MIN_PROPERTIES_HEIGHT: f32 = 440.0;
pub(super) const MAIN_WINDOW_INITIAL_WIDTH: f32 = 1180.0;
pub(super) const MAIN_WINDOW_INITIAL_HEIGHT: f32 = 680.0;
const MAIN_WINDOW_APP_ID: &str = "bennu";
const SETTINGS_WINDOW_APP_ID: &str = "bennu-settings";
const PROPERTIES_WINDOW_APP_ID: &str = "bennu-properties";
const PREVIEW_WINDOW_APP_ID: &str = "bennu-preview";
const PREVIEW_RESIZE_MATCH_TOLERANCE: f32 = 1.0;

pub(super) fn main_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(MAIN_WINDOW_INITIAL_WIDTH, MAIN_WINDOW_INITIAL_HEIGHT),
        decorations: false,
        exit_on_close_request: true,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = MAIN_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
}

fn settings_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(
            DEFAULT_SETTINGS_WIDTH,
            DEFAULT_SETTINGS_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        ),
        min_size: Some(Size::new(
            MIN_SETTINGS_WIDTH,
            MIN_SETTINGS_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        )),
        decorations: false,
        exit_on_close_request: true,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = SETTINGS_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
}

fn properties_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(
            DEFAULT_PROPERTIES_WIDTH,
            DEFAULT_PROPERTIES_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        ),
        min_size: Some(Size::new(
            MIN_PROPERTIES_WIDTH,
            MIN_PROPERTIES_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        )),
        decorations: false,
        exit_on_close_request: true,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = PROPERTIES_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
}

fn preview_window_settings(profile: PreviewWindowProfile, size: PreviewSize) -> window::Settings {
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
    settings.platform_specific.application_id = PREVIEW_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
}

fn preview_window_size_for_content(content_size: PreviewSize) -> Size {
    Size::new(content_size.width, content_size.height)
}

fn preview_content_size_from_window(
    profile: PreviewWindowProfile,
    width: f32,
    height: f32,
) -> PreviewSize {
    clamp_preview_size_to_minimum(profile, PreviewSize { width, height })
}

pub(super) fn default_preview_size(profile: PreviewWindowProfile) -> PreviewSize {
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

enum TextInputFocusCheckResult {
    Address(crate::model::BrowserPaneId),
    Rename,
    Search(crate::model::SearchInputFocusCheckRequest),
}

struct TextInputFocusCheck {
    target: advanced_widget::Id,
    is_focused: bool,
    result: TextInputFocusCheckResult,
}

impl TextInputFocusCheck {
    fn new(target: iced::widget::Id, result: TextInputFocusCheckResult) -> Self {
        Self {
            target: target.into(),
            is_focused: false,
            result,
        }
    }
}

impl Operation<Message> for TextInputFocusCheck {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&advanced_widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if id == Some(&self.target) {
            self.is_focused = state.is_focused();
        }
    }

    fn finish(&self) -> Outcome<Message> {
        let message = match self.result {
            TextInputFocusCheckResult::Address(pane_id) => {
                Message::AddressInputFocusChecked(pane_id, self.is_focused)
            }
            TextInputFocusCheckResult::Rename => Message::RenameInputFocusChecked(self.is_focused),
            TextInputFocusCheckResult::Search(request) => {
                Message::SearchInputFocusChecked(request, self.is_focused.into())
            }
        };
        Outcome::Some(message)
    }
}

fn rename_input_focus_check_command() -> Task<Message> {
    advanced_widget::operate(TextInputFocusCheck::new(
        rename_input_id(),
        TextInputFocusCheckResult::Rename,
    ))
}

fn address_input_focus_check_command(pane_id: crate::model::BrowserPaneId) -> Task<Message> {
    advanced_widget::operate(TextInputFocusCheck::new(
        address_input_id(pane_id),
        TextInputFocusCheckResult::Address(pane_id),
    ))
}

fn search_input_focus_check_command(
    request: crate::model::SearchInputFocusCheckRequest,
) -> Task<Message> {
    advanced_widget::operate(TextInputFocusCheck::new(
        crate::view::search_input_id(),
        TextInputFocusCheckResult::Search(request),
    ))
}

fn clamp_preview_size_to_minimum(profile: PreviewWindowProfile, size: PreviewSize) -> PreviewSize {
    let min_size = preview_min_size(profile);
    PreviewSize {
        width: size.width.max(min_size.width),
        height: size.height.max(min_size.height),
    }
}

fn preview_min_size(profile: PreviewWindowProfile) -> Size {
    match profile {
        PreviewWindowProfile::Regular => Size::new(MIN_PREVIEW_WIDTH, MIN_PREVIEW_HEIGHT),
        PreviewWindowProfile::Image => Size::new(MIN_IMAGE_PREVIEW_WIDTH, MIN_IMAGE_PREVIEW_HEIGHT),
        PreviewWindowProfile::Audio => Size::new(MIN_AUDIO_PREVIEW_WIDTH, MIN_AUDIO_PREVIEW_HEIGHT),
        PreviewWindowProfile::Video => Size::new(MIN_VIDEO_PREVIEW_WIDTH, MIN_VIDEO_PREVIEW_HEIGHT),
    }
}

fn image_preview_initial_fit_max_size() -> Size {
    Size::new(
        IMAGE_PREVIEW_INITIAL_FIT_MAX_WIDTH,
        IMAGE_PREVIEW_INITIAL_FIT_MAX_HEIGHT,
    )
}

fn video_preview_initial_fit_max_size() -> Size {
    Size::new(
        VIDEO_PREVIEW_INITIAL_FIT_MAX_WIDTH,
        VIDEO_PREVIEW_INITIAL_FIT_MAX_HEIGHT,
    )
}

pub(super) fn image_preview_size_from_dimensions(width: u32, height: u32) -> PreviewSize {
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

fn animated_image_preview_size_from_dimensions(width: u32, height: u32) -> PreviewSize {
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

fn video_preview_size_from_frame(width: u32, height: u32) -> PreviewSize {
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

fn preview_size_matches(actual: PreviewSize, expected: PreviewSize) -> bool {
    (actual.width - expected.width).abs() <= PREVIEW_RESIZE_MATCH_TOLERANCE
        && (actual.height - expected.height).abs() <= PREVIEW_RESIZE_MATCH_TOLERANCE
}

impl FileBrowser {
    pub(super) fn request_search_input_focus_check(
        &mut self,
        origin: crate::model::SearchInputFocusCheckOrigin,
    ) -> Task<Message> {
        let request = self
            .search_history_interaction
            .begin_input_focus_check(origin);
        search_input_focus_check_command(request)
    }

    pub(crate) fn window_title(&self, window: window::Id) -> String {
        if self.settings_window == Some(window) {
            crate::localization::translate_current("Settings - Bennu")
        } else if self.properties_window == Some(window) {
            crate::localization::translate_current("Properties - Bennu")
        } else if self.preview_window == Some(window) {
            crate::localization::translate_current("Preview - Bennu")
        } else if self.search_workspace.is_some() {
            crate::localization::translate_current("Search - Bennu")
        } else {
            crate::localization::translate_current("Bennu")
        }
    }

    pub(super) fn open_settings(&mut self) -> Task<Message> {
        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        let refresh_selected_category = match self.selected_settings_category {
            SettingsCategory::Search => Task::batch([
                self.refresh_search_service_status(),
                self.refresh_search_path_configuration(),
            ]),
            SettingsCategory::Logs => self.refresh_application_logs(),
            SettingsCategory::General
            | SettingsCategory::Appearance
            | SettingsCategory::Files
            | SettingsCategory::Shortcuts
            | SettingsCategory::About => Task::none(),
        };
        Task::batch([
            self.commit_rename_if_active(),
            self.ensure_settings_window(),
            refresh_selected_category,
        ])
    }

    pub(super) fn select_settings_category(&mut self, category: SettingsCategory) -> Task<Message> {
        self.shortcut_capture = None;
        self.search_service.cancel_force_restart_confirmation();
        if category != SettingsCategory::General {
            self.invalidate_startup_directory_validation();
        }
        self.selected_settings_category = category;
        self.settings_subpage = None;
        let reset_scroll = self.reset_settings_detail_scroll();
        let refresh = match category {
            SettingsCategory::Search => Task::batch([
                self.refresh_search_service_status(),
                self.refresh_search_path_configuration(),
            ]),
            SettingsCategory::Logs => self.refresh_application_logs(),
            SettingsCategory::General
            | SettingsCategory::Appearance
            | SettingsCategory::Files
            | SettingsCategory::Shortcuts
            | SettingsCategory::About => Task::none(),
        };
        Task::batch([reset_scroll, refresh])
    }

    pub(super) fn select_settings_subpage(&mut self, subpage: SettingsSubpage) -> Task<Message> {
        self.settings_subpage = Some(subpage);
        self.reset_settings_detail_scroll()
    }

    pub(super) fn close_settings_subpage(&mut self) -> Task<Message> {
        self.settings_subpage = None;
        self.reset_settings_detail_scroll()
    }

    /// 六个分类页与二级页面共用 ScrollbarRegion::Settings 的 scrollable 状态，
    /// 任何切换都必须归零，否则上一页的滚动偏移、平滑滚动动画和 thumb 几何
    /// 会泄漏到新页。
    fn reset_settings_detail_scroll(&mut self) -> Task<Message> {
        self.smooth_scroll.stop();
        self.forget_scrollbar_viewport(&ScrollbarRegion::Settings);
        iced::widget::operation::scroll_to(
            smooth_scroll_id(&ScrollbarRegion::Settings),
            scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
        )
    }

    pub(super) fn ensure_settings_window(&mut self) -> Task<Message> {
        if let Some(window) = self.settings_window {
            self.focused_window = window;
            return window::gain_focus(window);
        }

        let (window, command) = window::open(settings_window_settings());
        self.settings_window = Some(window);
        self.focused_window = window;
        command.discard()
    }

    pub(super) fn close_settings_window(&mut self) -> Task<Message> {
        self.shortcut_capture = None;
        self.search_service.cancel_force_restart_confirmation();
        self.invalidate_startup_directory_validation();
        self.expanded_color_scheme_family = None;
        let Some(window) = self.settings_window.take() else {
            return Task::none();
        };
        self.clear_closed_window_focus(window);
        window::close(window)
    }

    pub(super) fn ensure_properties_window(&mut self) -> Task<Message> {
        if let Some(window) = self.properties_window {
            self.focused_window = window;
            return window::gain_focus(window);
        }

        let (window, command) = window::open(properties_window_settings());
        self.properties_window = Some(window);
        self.focused_window = window;
        command.discard()
    }

    pub(super) fn close_properties_window(&mut self) -> Task<Message> {
        self.clear_file_properties_state();
        let Some(window) = self.properties_window.take() else {
            return Task::none();
        };
        self.clear_closed_window_focus(window);
        window::close(window)
    }

    pub(super) fn ensure_preview_window(&mut self, profile: PreviewWindowProfile) -> Task<Message> {
        let size = clamp_preview_size_to_minimum(profile, default_preview_size(profile));
        self.preview_window_profile = profile;
        self.preview_size = size;
        self.pending_preview_resize = Some(size);

        if let Some(window) = self.preview_window {
            self.reset_preview_window_bottom_controls();
            self.focused_window = window;
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
            return Task::batch([resize, window::gain_focus(window), initial_chrome]);
        }

        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.reset_preview_window_bottom_controls();
        let initial_chrome = self.start_preview_window_initial_chrome();
        let (window, command) = window::open(preview_window_settings(profile, size));
        self.preview_window = Some(window);
        self.focused_window = window;
        Task::batch([command.discard(), initial_chrome])
    }

    pub(super) fn handle_captured_preview_shortcut(&mut self) -> Task<Message> {
        if self.preview_window == Some(self.focused_window) {
            self.close_preview_window()
        } else {
            Task::none()
        }
    }

    pub(super) fn toggle_preview_window_pin(&mut self) -> Task<Message> {
        self.preview_window_pinned = !self.preview_window_pinned;
        Task::none()
    }

    pub(super) fn open_image_preview_window_for_dimensions(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<Message> {
        if width == 0 || height == 0 {
            return self.open_image_preview_error_window();
        }
        self.update_preview_window(
            PreviewWindowProfile::Image,
            image_preview_size_from_dimensions(width, height),
        )
    }
    pub(super) fn open_animated_image_preview_window_for_dimensions(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<Message> {
        self.update_preview_window(
            PreviewWindowProfile::Image,
            animated_image_preview_size_from_dimensions(width, height),
        )
    }

    pub(super) fn open_image_preview_window_with_default_size(&mut self) -> Task<Message> {
        self.update_preview_window(
            PreviewWindowProfile::Image,
            default_preview_size(PreviewWindowProfile::Image),
        )
    }

    pub(super) fn open_image_preview_error_window(&mut self) -> Task<Message> {
        self.open_image_preview_window_with_default_size()
    }

    fn update_preview_window(
        &mut self,
        profile: PreviewWindowProfile,
        size: PreviewSize,
    ) -> Task<Message> {
        self.preview_window_profile = profile;
        self.preview_size = clamp_preview_size_to_minimum(profile, size);
        self.pending_preview_resize = Some(self.preview_size);

        if let Some(window) = self.preview_window {
            self.reset_preview_window_bottom_controls();
            self.focused_window = window;
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
            return Task::batch([resize, window::gain_focus(window), initial_chrome]);
        }

        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.reset_preview_window_bottom_controls();
        let initial_chrome = self.start_preview_window_initial_chrome();
        let (window, command) = window::open(preview_window_settings(profile, self.preview_size));
        self.preview_window = Some(window);
        self.focused_window = window;
        Task::batch([command.discard(), initial_chrome])
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

    pub(super) fn refresh_preview_window_bottom_controls(&mut self) {
        if !self.preview_window_has_bottom_media_controls() {
            self.preview_window_bottom_controls.reset_hidden();
            return;
        }

        if let Some(pointer_y) = self.preview_window_pointer_y {
            self.preview_window_bottom_controls
                .update_for_bottom_cursor_y(pointer_y, self.preview_size.height);
        }
    }

    pub(super) fn fit_preview_window_to_video_frame(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<Message> {
        if width == 0 || height == 0 {
            return Task::none();
        }

        self.update_preview_window(
            PreviewWindowProfile::Video,
            video_preview_size_from_frame(width, height),
        )
    }

    // 文本与 SQLite 预览使用标准窗口壳（标题栏 + 控制按钮）；媒体类内容保持悬浮 chrome。
    pub(crate) fn preview_window_uses_window_chrome(&self) -> bool {
        matches!(
            self.preview,
            Some(PreviewState::Ready(PreviewContent::Text { .. }))
                | Some(PreviewState::Ready(PreviewContent::Sqlite(_)))
        )
    }

    fn start_preview_window_initial_chrome(&mut self) -> Task<Message> {
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

    pub(super) fn cancel_preview_window_initial_chrome_hide(&mut self) {
        self.preview_window_initial_chrome_generation = self
            .preview_window_initial_chrome_generation
            .wrapping_add(1);
    }

    pub(super) fn hide_preview_window_initial_chrome(&mut self, generation: u64) {
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

    pub(super) fn close_preview_window(&mut self) -> Task<Message> {
        // 先复位固定，保证 clear_preview 的固定守卫不会跳过内容清理。
        self.preview_window_pinned = false;
        self.preview_shown_path = None;
        self.clear_preview();
        self.forget_scrollbar_viewport(&ScrollbarRegion::PreviewSqliteTables);
        self.forget_scrollbar_viewport(&ScrollbarRegion::PreviewSqliteData);
        self.pending_preview_resize = None;
        self.preview_window_drag_active = false;
        self.preview_window_pointer_y = None;
        self.cancel_preview_window_initial_chrome_hide();
        self.preview_window_chrome.reset_hidden();
        self.preview_window_bottom_controls.reset_hidden();
        let Some(window) = self.preview_window.take() else {
            return Task::none();
        };
        self.clear_closed_window_focus(window);
        window::close(window)
    }

    fn clear_closed_window_focus(&mut self, window: window::Id) {
        self.maximized_windows.remove(&window);
        if self.focused_window == window {
            self.focused_window = self.main_window;
        }
        if self.system_focused_window == Some(window) {
            self.system_focused_window = None;
        }
    }

    pub(super) fn handle_window_focused(&mut self, window: window::Id) -> Task<Message> {
        self.focused_window = window;
        self.system_focused_window = Some(window);
        if window == self.main_window {
            self.request_search_input_focus_check(
                crate::model::SearchInputFocusCheckOrigin::MainWindowFocused,
            )
        } else {
            Task::none()
        }
    }

    pub(super) fn handle_window_unfocused(&mut self, window: window::Id) -> Task<Message> {
        if self.system_focused_window == Some(window) {
            self.system_focused_window = None;
        }
        if window == self.main_window {
            self.search_history_interaction.reset();
        }
        if self.preview_window == Some(window) && !self.preview_window_pinned {
            self.close_preview_window()
        } else {
            Task::none()
        }
    }

    pub(super) fn handle_focused_window_escape_pressed(&mut self) -> Task<Message> {
        if self.settings_window == Some(self.focused_window) {
            if self.search_service.cancel_force_restart_confirmation() {
                return Task::none();
            }
            if self.settings_subpage.is_some() {
                return self.close_settings_subpage();
            }
            return self.close_settings_window();
        }
        if self.properties_window == Some(self.focused_window) {
            return self.close_properties_window();
        }
        if self.preview_window == Some(self.focused_window) {
            return self.close_preview_window();
        }
        if self.address_editing.is_some() {
            return self.cancel_address_editing();
        }
        self.dismiss_floating()
    }

    pub(super) fn handle_window_pointer_pressed(
        &mut self,
        window: window::Id,
        button: mouse::Button,
        status: event::Status,
    ) -> Task<Message> {
        if self.settings_window == Some(window) {
            return Task::none();
        }

        if window != self.main_window {
            return Task::none();
        }
        if button == mouse::Button::Left {
            self.clear_global_error();
        }

        if self.preview_window == Some(self.focused_window) {
            return Task::none();
        }

        if button == mouse::Button::Left && self.start_ctrl_shift_pane_drag(status) {
            return Task::none();
        }

        // 点击终端抽屉以外区域时释放其键盘焦点,文件区快捷键随之恢复。
        self.release_terminal_panel_focus_if_outside();

        let pointer_command = match (button, status) {
            (
                mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle,
                event::Status::Captured,
            ) => {
                let mut input_focus_checks = Vec::with_capacity(3);
                if self.renaming.is_some() {
                    input_focus_checks.push(rename_input_focus_check_command());
                }
                if let Some(session) = &self.address_editing {
                    input_focus_checks.push(address_input_focus_check_command(session.pane_id));
                }
                if self.search_history_interaction.pointer_is_over_popup() {
                    Task::batch(input_focus_checks)
                } else {
                    input_focus_checks.push(self.request_search_input_focus_check(
                        crate::model::SearchInputFocusCheckOrigin::Pointer,
                    ));
                    Task::batch(input_focus_checks)
                }
            }
            (mouse::Button::Left, event::Status::Ignored) => {
                let focus_check = self.request_search_input_focus_check(
                    crate::model::SearchInputFocusCheckOrigin::Pointer,
                );
                Task::batch([self.dismiss_floating(), focus_check])
            }
            (mouse::Button::Right | mouse::Button::Middle, event::Status::Ignored) => self
                .request_search_input_focus_check(
                    crate::model::SearchInputFocusCheckOrigin::Pointer,
                ),
            _ => Task::none(),
        };

        if self.preview_window.is_some() && !self.preview_window_pinned {
            Task::batch([self.close_preview_window(), pointer_command])
        } else {
            pointer_command
        }
    }

    pub(super) fn dismiss_floating(&mut self) -> Task<Message> {
        if self.destructive_action_confirmation.is_some() {
            self.destructive_action_confirmation = None;
            return Task::none();
        }

        if self.file_drop_prompt.is_some() {
            self.file_drop_prompt = None;
            return Task::none();
        }

        if self.transfer_conflict.is_some() {
            self.transfer_conflict = None;
            return Task::none();
        }

        if self.archive_creation.is_some() {
            self.archive_creation = None;
            return Task::none();
        }

        if self.convert.is_some() {
            self.convert = None;
            return Task::none();
        }

        if let Some(checksum) = self.checksum.take() {
            crate::app::checksum::cancel_running_checksum(&checksum);
            return Task::none();
        }

        if self.archive_extraction.is_some() {
            self.archive_extraction = None;
            return Task::none();
        }

        if self.batch_rename.is_some() {
            self.batch_rename = None;
            return Task::none();
        }

        if self.network_connection_editor.is_some() {
            self.network_connection_editor = None;
            return Task::none();
        }

        if self.open_with.is_some() {
            self.open_with = None;
            return Task::none();
        }

        if self
            .search_history_interaction
            .popup_is_visible(&self.user_config.search_history)
        {
            self.search_history_interaction.dismiss_popup();
            return Task::none();
        }

        let dismissed_existing_interaction = self.context_menu.is_some()
            || self.address_editing.is_some()
            || self.shortcut_capture.is_some()
            || self.operation_queue.is_panel_open()
            || self.file_drag.is_some()
            || self.file_drop_session.is_some()
            || self.sidebar_bookmark_drag.is_some()
            || self.sidebar_bookmark_drop_slot.is_some()
            || self.selection_marquee.is_some()
            || self.renaming.is_some();
        self.context_menu = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let dismiss_command = Task::batch([
            self.cancel_address_editing(),
            self.commit_rename_if_active(),
        ]);
        if !dismissed_existing_interaction {
            if let Some(expansion_command) = self.escape_icon_grid_expansion() {
                return Task::batch([dismiss_command, expansion_command]);
            }
        }
        dismiss_command
    }

    pub(super) fn close_auxiliary_window(&mut self, window_id: window::Id) -> Task<Message> {
        if !self.application_shutdown_phase.is_running() {
            return Task::none();
        }

        if window_id == self.main_window {
            return self.begin_application_shutdown();
        }

        if self.settings_window == Some(window_id) {
            self.close_settings_window()
        } else if self.properties_window == Some(window_id) {
            self.close_properties_window()
        } else if self.preview_window == Some(window_id) {
            self.close_preview_window()
        } else {
            Task::none()
        }
    }

    pub(super) fn handle_auxiliary_window_resized(
        &mut self,
        window: window::Id,
        width: f32,
        height: f32,
    ) -> Task<Message> {
        if window == self.main_window {
            self.main_window_width = width.max(1.0);
            self.main_window_height = height.max(1.0);
            return Task::batch([
                self.request_breadcrumb_drop_target_bounds_measurement(),
                self.schedule_thumbnail_refresh(),
            ]);
        }

        if self.preview_window == Some(window) {
            let resized_size =
                preview_content_size_from_window(self.preview_window_profile, width, height);
            if let Some(pending_size) = self.pending_preview_resize {
                if preview_size_matches(resized_size, pending_size) {
                    self.pending_preview_resize = None;
                    self.preview_size = resized_size;
                } else {
                    tracing::debug!(
                        target: "app_ui::preview",
                        window = ?window,
                        actual_width = resized_size.width,
                        actual_height = resized_size.height,
                        pending_width = pending_size.width,
                        pending_height = pending_size.height,
                        "preview resize still pending"
                    );
                    return window::resize(window, preview_window_size_for_content(pending_size));
                }
            } else {
                self.preview_size = resized_size;
            }
            self.refresh_preview_window_bottom_controls();
            tracing::debug!(
                target: "app_ui::preview",
                window = ?window,
                width = self.preview_size.width,
                height = self.preview_size.height,
                "preview window resized"
            );
            return Task::batch([
                self.resize_document_preview(),
                self.refresh_preview_thumbnail_for_size(),
            ]);
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests;
