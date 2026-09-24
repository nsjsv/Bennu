use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::widget::scrollable;
use iced::{event, mouse, window, Rectangle, Size, Task};

use super::FileBrowser;
use crate::app::smooth_scroll::smooth_scroll_id;
use crate::model::{
    Message, ScrollbarRegion, SettingsCategory, SettingsSubpage, WINDOW_TOP_BAR_HEIGHT,
};
use crate::view::{address_input_id, rename_input_id};

// 预览窗口机械（设置/尺寸档位常量/开关适配状态机/初始 chrome 计时）
// 已迁 bennu-preview 的 PreviewEngine（engine/windows.rs）；此处
// re-export 维持本模块与 windows/tests 的既有调用路径。
pub(crate) use bennu_preview::engine::{
    default_preview_size, image_preview_size_from_dimensions, preview_content_size_from_window,
    preview_size_matches, preview_window_size_for_content,
};
// 仅供 windows/tests 使用的尺寸档位纯函数（自引擎 re-export，保持测试
// 既有调用路径）。
use bennu_preview::engine::PreviewWindowIdentity;
#[cfg(test)]
pub(crate) use bennu_preview::engine::{
    animated_image_preview_size_from_dimensions, clamp_preview_size_to_minimum,
    image_preview_initial_fit_max_size, preview_min_size, video_preview_initial_fit_max_size,
    video_preview_size_from_frame,
};

const DEFAULT_SETTINGS_WIDTH: f32 = 760.0;
const DEFAULT_SETTINGS_HEIGHT: f32 = 560.0;
const MIN_SETTINGS_WIDTH: f32 = 640.0;
const MIN_SETTINGS_HEIGHT: f32 = 420.0;
const DEFAULT_PROPERTIES_WIDTH: f32 = 760.0;
const DEFAULT_PROPERTIES_HEIGHT: f32 = 560.0;
const MIN_PROPERTIES_WIDTH: f32 = 680.0;
const MIN_PROPERTIES_HEIGHT: f32 = 440.0;
const DEFAULT_TRANSFER_WIDTH: f32 = 420.0;
const DEFAULT_TRANSFER_HEIGHT: f32 = 560.0;
const MIN_TRANSFER_WIDTH: f32 = 360.0;
const MIN_TRANSFER_HEIGHT: f32 = 420.0;
pub(super) const MAIN_WINDOW_INITIAL_WIDTH: f32 = 1180.0;
pub(super) const MAIN_WINDOW_INITIAL_HEIGHT: f32 = 680.0;
const MAIN_WINDOW_APP_ID: &str = "bennu";
const SETTINGS_WINDOW_APP_ID: &str = "bennu-settings";
const PROPERTIES_WINDOW_APP_ID: &str = "bennu-properties";
const TRANSFER_WINDOW_APP_ID: &str = "bennu-transfer";
/// 预览窗口的宿主身份：app id 与启动图标是 app-ui 进程属性，开窗
/// Settings 由引擎按此身份构造。
pub(crate) fn preview_window_identity() -> PreviewWindowIdentity {
    PreviewWindowIdentity {
        app_id: "bennu-preview".to_owned(),
        icon: crate::app_icon::startup_window_icon(),
    }
}

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

fn transfer_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(
            DEFAULT_TRANSFER_WIDTH,
            DEFAULT_TRANSFER_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        ),
        min_size: Some(Size::new(
            MIN_TRANSFER_WIDTH,
            MIN_TRANSFER_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        )),
        decorations: false,
        // 关闭请求交给状态机：关窗同时要收尾二维码服务，不能让 winit 直接关。
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = TRANSFER_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
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
        } else if self.transfer_window == Some(window) {
            crate::localization::translate_current("Send to Phone - Bennu")
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
            | SettingsCategory::Transfer
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
            | SettingsCategory::Transfer
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

    /// 打开传输窗口前清理主窗口同层输入状态（与 Properties 打开先例一致）。
    pub(super) fn prepare_transfer_window_open(&mut self) -> Task<Message> {
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
        self.commit_rename_if_active()
    }

    pub(super) fn ensure_transfer_window(&mut self) -> Task<Message> {
        if let Some(window) = self.transfer_window {
            self.focused_window = window;
            return window::gain_focus(window);
        }

        let (window, command) = window::open(transfer_window_settings());
        self.transfer_window = Some(window);
        self.focused_window = window;
        command.discard()
    }

    /// 关闭传输窗口：二维码服务随会话取消令牌收尾，状态整体清空。
    ///
    /// 失焦不关闭（传输期间窗口必须稳定），仅 close request / Escape /
    /// 业务完成后的手动关闭走这里。
    pub(super) fn close_transfer_window(&mut self) -> Task<Message> {
        if let Some(session) = self.transfer_session.take() {
            session.cancel.cancel();
        }
        let Some(window) = self.transfer_window.take() else {
            return Task::none();
        };
        self.clear_closed_window_focus(window);
        window::close(window)
    }

    /// 预览窗口机械已迁 PreviewEngine（engine/windows.rs）；宿主转发层
    /// 只补引擎不持有的宿主簿记：焦点登记（含引擎内部路径的出口同步）
    /// 与关闭时的焦点/滚动条视口收尾。
    /// 引擎呈现预览窗口后把它登记为应用焦点；引擎不持有宿主焦点簿记，
    /// 经 take_pending 同步（引擎内部路径由 update 出口统一兜底）。
    pub(super) fn sync_preview_window_focus(&mut self) {
        if let Some(window) = self.preview_engine.take_pending_preview_window_focus() {
            self.focused_window = window;
        }
    }

    pub(super) fn handle_captured_preview_shortcut(&mut self) -> Task<Message> {
        if self.preview_window == Some(self.focused_window) {
            self.close_preview_window()
        } else {
            Task::none()
        }
    }

    pub(super) fn toggle_preview_window_pin(&mut self) -> Task<Message> {
        self.preview_engine
            .toggle_preview_window_pin()
            .map(Message::Preview)
    }

    #[cfg(test)]
    pub(super) fn open_image_preview_window_for_dimensions(
        &mut self,
        width: u32,
        height: u32,
    ) -> Task<Message> {
        let command = self
            .preview_engine
            .open_image_preview_window_for_dimensions(width, height);
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(super) fn close_preview_window(&mut self) -> Task<Message> {
        let (closed_window, command) = self.preview_engine.close_preview_window();
        self.forget_scrollbar_viewport(&ScrollbarRegion::PreviewSqliteTables);
        self.forget_scrollbar_viewport(&ScrollbarRegion::PreviewSqliteData);
        if let Some(window) = closed_window {
            self.clear_closed_window_focus(window);
        }
        command.map(Message::Preview)
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
        if self.preview_engine.should_close_unfocused_window(window) {
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
        if self.transfer_window == Some(self.focused_window) {
            return self.close_transfer_window();
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
                if let Some(editing) = &self.address_editing {
                    input_focus_checks.push(address_input_focus_check_command(editing.pane_id));
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

        // 接收确认 Modal 的 Escape/外部清理按拒绝收敛：服务侧挂起会话
        // 要及时释放，不能静默丢弃。
        if self.incoming_transfer.is_some() {
            return self.reject_pending_incoming_transfer();
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

        if self.archive_member_password.is_some() {
            self.archive_member_password = None;
            return Task::none();
        }

        if self.batch_rename.is_some() {
            self.batch_rename = None;
            return Task::none();
        }

        if self.advanced_new_folder.is_some() {
            self.advanced_new_folder = None;
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
        } else if self.transfer_window == Some(window_id) {
            self.close_transfer_window()
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
