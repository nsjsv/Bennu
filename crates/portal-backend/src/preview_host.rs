//! 预览宿主：portal 侧的空格预览编排层（主软件 keyboard_navigation.rs
//! 的 request_preview/open_preview + preview_state.rs 转发层的对齐物）。
//! 守卫顺序逐条镜像 app-ui：InsideArchive 拦截（BrowsingRoot 放行，
//! fe877207 语义）→ StandaloneWindow 表面标记 → 先关后开（保留固定）→
//! 无选中静默 → 分类门禁 → 大小限制 → 分类分发。预览状态机本体在
//! `bennu_preview::PreviewEngine`（全进程单实例），本层只做宿主簿记：
//! 焦点跟踪、请求窗登记、会话级偏好重读与引擎不处理消息的回退路由
//! （内容回流在 [`content`] 子模块）。

use std::path::{Path, PathBuf};

use file_core::archive_path_identity;
use file_core::{ArchivePathIdentity, FileKind, ScanOptions};
use iced::window;
use iced::Point;
use iced::Task;

use bennu_preview::commands::preview::{
    animated_image_preview_command, image_preview_dimensions_command, preview_command,
    start_audio_preview_command,
};
use bennu_preview::engine::{
    default_preview_size, preview_content_size_from_window, preview_size_matches,
    preview_window_size_for_content, PreviewEngine, PreviewEngineConfig, PreviewLoadSurface,
    PreviewWindowIdentity,
};
use bennu_preview::formatting::format_file_size;
use bennu_preview::image_preview_viewport::ImagePreviewViewport;
use bennu_preview::preview::{AudioPreviewPlayback, PreviewState, PreviewWindowProfile};
use bennu_preview::preview_config::{
    load_preview_preferences_from_state_database, PreviewFileSizeKind, PreviewPreferences,
};
use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
use bennu_preview::preview_message::PreviewMessage;
use bennu_theme::window_controls::WindowControlKind;

use crate::picker_session::PickerSession;
use crate::preview_scroll::PreviewScrollPipeline;
use crate::{Message, PickerDaemon};

mod content;

pub(crate) use content::{handle_preview_message, handle_window_resized};

/// 预览窗的 WM 身份：沿用 portal 窗口家族的 bennu-filechooser 前缀
/// （app-ui 侧对应物是 bennu-preview）。
const PORTAL_PREVIEW_WINDOW_APP_ID: &str = "bennu-filechooser-preview";

/// 空格预览的目标条目（选择窗主选中行的快照：路径 + 种类 + 行缓存大小）。
pub(crate) struct PreviewSelection {
    pub(crate) path: PathBuf,
    pub(crate) kind: FileKind,
    pub(crate) file_bytes: u64,
}

pub(crate) struct PreviewHost {
    pub(crate) engine: PreviewEngine,
    /// 焦点簿记：最近聚焦的本进程窗口。预览窗的 Esc 分层与失焦自动
    /// 关闭（步骤 3）读它；本步先落跟踪。
    pub(crate) focused_window: Option<window::Id>,
    /// 发起当前预览会话的选择窗：预览窗内的空格作用于它的选中项，
    /// 预览窗关闭后焦点回到它；会话关闭即清空。
    pub(crate) request_window: Option<window::Id>,
    /// 会话级预览偏好快照：每次开预览会话重读主软件 state.sqlite，
    /// 读失败回落共享 crate 默认（design 决策 #2）。
    pub(crate) preferences: PreviewPreferences,
    /// 偏好来源库路径：构造注入，测试指向临时目录（缺失 → 默认回落）。
    preferences_database: PathBuf,
    /// 预览窗滚动管线（七区域：显隐状态机 + 滚轮惯性）。
    pub(crate) scroll: PreviewScrollPipeline,
    /// 预览窗最大化簿记（chrome 的最大化/还原按钮形态；仅观测自家
    /// toggle 后的回信，主软件同款边界）。
    pub(crate) preview_window_maximized: bool,
    /// 预览窗指针位置宿主簿记：SQLite 列宽拖拽起点读横坐标（主软件
    /// cursor_position.x 的预览对应物；纵坐标的引擎消费走
    /// engine.preview_window_pointer_y）。
    pub(crate) preview_window_pointer: Option<Point>,
}

impl PreviewHost {
    pub(crate) fn new(preferences_database: PathBuf) -> Self {
        let preferences = PreviewPreferences::default_preferences();
        let engine = PreviewEngine::new(
            Self::engine_config(&preferences),
            default_preview_size(PreviewWindowProfile::Regular),
            PreviewWindowIdentity {
                app_id: PORTAL_PREVIEW_WINDOW_APP_ID.to_owned(),
                // portal 不加载窗口图标（选择窗同例），拿不到就 None。
                icon: None,
            },
        );
        Self {
            engine,
            focused_window: None,
            request_window: None,
            preferences,
            preferences_database,
            scroll: PreviewScrollPipeline::default(),
            preview_window_maximized: false,
            preview_window_pointer: None,
        }
    }

    fn engine_config(preferences: &PreviewPreferences) -> PreviewEngineConfig {
        PreviewEngineConfig {
            directory_expand_levels: preferences.directory_expand_levels,
            extension_rules: preferences.extension_rules.clone(),
            size_limits: preferences.size_limits,
        }
    }

    /// 预览窗失焦自动关闭（未钉住）与 Esc 分层是步骤 3 的窗口生命周期
    /// 批次；本方法留给那时的 Unfocused/Closed 接线复用。
    pub(crate) fn is_preview_window(&self, window: window::Id) -> bool {
        self.engine.preview_window == Some(window)
    }

    /// 空格在预览窗内按下时，toggle 作用于发起会话的选择窗选中项
    /// （主软件单窗口下 Space 恒作用于主窗口选中项的同构展开）。
    pub(crate) fn selection_source_window(&self, pressed_window: window::Id) -> Option<window::Id> {
        if self.is_preview_window(pressed_window) {
            self.request_window
        } else {
            Some(pressed_window)
        }
    }

    /// Space toggle 入口（主软件 request_preview 的对齐物）：独立会话
    /// 活跃且展示路径 == 选中路径 → 关闭；否则走 open_preview。
    pub(crate) fn request_preview(
        &mut self,
        source: Option<window::Id>,
        selection: Option<&PreviewSelection>,
    ) -> Task<Message> {
        let task = if self.standalone_session_active()
            && selection.is_some_and(|candidate| {
                self.engine.preview_shown_path.as_deref() == Some(candidate.path.as_path())
            }) {
            self.close_preview_session()
        } else {
            self.open_preview(source, selection)
        };
        self.sync_focused_window();
        task.map(Message::Preview)
    }

    /// Space toggle 的不变量（主软件同款）：独立预览窗口会话正在活跃
    /// 显示——窗口存在与内容非空缺一不可。
    fn standalone_session_active(&self) -> bool {
        self.engine.preview_window.is_some() && self.engine.preview.is_some()
    }

    fn open_preview(
        &mut self,
        source: Option<window::Id>,
        selection: Option<&PreviewSelection>,
    ) -> Task<PreviewMessage> {
        // 包内成员按空格完全无动作（fe877207 语义）：虚拟路径读不了
        // 文件，加载必然失败；归档文件本身（BrowsingRoot）仍是真实
        // 文件，归档预览照常发起。
        if selection.is_some_and(|candidate| {
            archive_path_identity(&candidate.path) == ArchivePathIdentity::InsideArchive
        }) {
            return Task::none();
        }
        self.engine.preview_load_surface = PreviewLoadSurface::StandaloneWindow;
        // 内部“先关后开”会复位固定状态；切换预览内容时必须保留用户设定的固定。
        let pinned = self.engine.preview_window_pinned;
        let close_command = self.close_preview_session();

        // 无选中：静默（对齐主软件——不弹错误窗）。但“先关后开”的关窗
        // Task 必须交还：app-ui 里“预览活跃且选中已空”不可达（选择变化
        // 入口会先清预览），portal 预览期间可继续操作清空选中，若像
        // app-ui 那样丢弃 Task 会留下引擎已忘记录的实际窗口（孤儿窗）。
        let Some(selection) = selection else {
            return close_command;
        };

        self.refresh_session_preferences();
        self.engine.preview_shown_path = Some(selection.path.clone());
        self.request_window = source;
        if selection.kind == FileKind::File {
            // 门禁放在大小检查之前：不可预览的文件不应误报“文件太大”。
            if classify_preview_path(&selection.path, &self.preferences.extension_rules).is_none() {
                self.engine.preview_window_pinned = pinned;
                return close_command.chain(self.show_unpreviewable_file_preview());
            }
            if let Some(reject_command) = self.reject_oversized_file_preview(selection) {
                // 主软件此分支不复位 pinned（与不可预览分支不对称），
                // 逐字节对齐，不在 portal 侧擅自修正。
                return close_command.chain(reject_command);
            }
        }
        self.engine.preview_window_pinned = pinned;
        close_command
            .chain(self.open_preview_for_resolved_path(selection.path.clone(), selection.kind))
    }

    /// 关闭预览会话：引擎复位（固定/内容/shown_path/窗口）+ 宿主簿记
    /// 收尾（滚动管线复位 + 焦点回请求窗，design 生命周期合同；请求
    /// 窗已亡时 gain_focus 是 no-op）。域内主动关闭与外部关窗（X）共用
    /// 此路径。
    fn close_preview_session(&mut self) -> Task<PreviewMessage> {
        let (closed_window, command) = self.engine.close_preview_window();
        let request_window = self.request_window.take();
        self.scroll.reset();
        let Some(closed) = closed_window else {
            return command;
        };
        self.preview_window_maximized = false;
        if self.focused_window == Some(closed) {
            self.focused_window = None;
        }
        // 关闭后焦点回请求窗（design：预览窗关闭后焦点回到选择窗）。
        match request_window {
            Some(request) => command.chain(window::gain_focus(request)),
            None => command,
        }
    }

    /// 预览窗被外部关闭（X）或请求窗先亡：预览窗失去联动对象，一并
    /// 结束会话。引擎不复观窗口事件，这里补上复位；window::close 对
    /// 已消失的窗口是 no-op（app-ui 外部关窗同款路径）。
    pub(crate) fn handle_window_closed(&mut self, window: window::Id) -> Task<Message> {
        if self.is_preview_window(window) || self.request_window == Some(window) {
            return self.close_preview_session().map(Message::Preview);
        }
        Task::none()
    }

    /// QuickLook 式占位：不可预览类型弹预览窗口显示错误（主软件同款）。
    fn show_unpreviewable_file_preview(&mut self) -> Task<PreviewMessage> {
        let window_command = self
            .engine
            .preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.engine.clear_preview();
        self.engine.preview = Some(PreviewState::Error(
            "No preview available for this file type.".to_owned(),
        ));
        window_command
    }

    /// 超限拒绝：与不可预览同一表现（错误窗），文案逐字对齐主软件。
    fn reject_oversized_file_preview(
        &mut self,
        selection: &PreviewSelection,
    ) -> Option<Task<PreviewMessage>> {
        let max_bytes = self.file_size_limit_for(&selection.path);
        if max_bytes == 0 || selection.file_bytes <= max_bytes {
            return None;
        }
        let window_command = self
            .engine
            .preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.engine.clear_preview();
        self.engine.preview = Some(PreviewState::Error(format!(
            "File is too large to preview ({}). Maximum preview size is {}.",
            format_file_size(selection.file_bytes),
            format_file_size(max_bytes),
        )));
        Some(window_command)
    }

    /// 分类器 + 上限查询的单一事实源（主软件 preview_file_size_limit_for
    /// 的对齐物）：未识别扩展名兜底 Text 上限。
    fn file_size_limit_for(&self, path: &Path) -> u64 {
        let kind = classify_preview_path(path, &self.preferences.extension_rules)
            .map(|classified| classified.file_size_kind())
            .unwrap_or(PreviewFileSizeKind::Text);
        self.preferences.size_limits.limit(kind)
    }

    fn open_preview_for_resolved_path(
        &mut self,
        path: PathBuf,
        kind: FileKind,
    ) -> Task<PreviewMessage> {
        // 新预览会话不复用上一个文件的缩放/平移；同会话内的缩略图→
        // 原图替换不经过这里，视口得以保留。
        self.engine.preview_image_viewport = ImagePreviewViewport::default();
        if kind == FileKind::File {
            let Some(classification) =
                classify_preview_path(&path, &self.preferences.extension_rules)
            else {
                return self.show_unpreviewable_file_preview();
            };
            return self.start_classified_preview(path, classification);
        }

        // 目录与未识别类型不走文件分类：目录预览由 load_preview 展开，
        // Symlink/Other 的错误仍由 load_preview 边界返回。
        let window_command = self
            .engine
            .preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.engine.clear_preview();
        self.engine.preview = Some(PreviewState::Loading(path.clone()));
        let max_file_bytes = self.file_size_limit_for(&path);
        Task::batch([
            window_command,
            preview_command(
                path,
                kind,
                self.preferences.extension_rules.clone(),
                ScanOptions::default(),
                max_file_bytes,
            ),
        ])
    }

    /// 分类分发（主软件 start_classified_preview 的对齐物）：窗口档位与
    /// 加载命令逐分支对齐；媒体帧流订阅与缩略图首帧在步骤 4 接入。
    fn start_classified_preview(
        &mut self,
        path: PathBuf,
        classification: PreviewPathKind,
    ) -> Task<PreviewMessage> {
        let rules = self.preferences.extension_rules.clone();
        match classification {
            PreviewPathKind::Document => {
                let max_file_bytes = self.file_size_limit_for(&path);
                // 引擎内部完成 Regular 开窗 + Loading 态 + prepare 命令。
                self.engine.start_document_preview(path, max_file_bytes)
            }
            PreviewPathKind::AnimatedImage => {
                self.engine.preview = Some(PreviewState::Loading(path.clone()));
                let generation = self.engine.next_animated_image_preview_generation();
                let max_file_bytes = self.file_size_limit_for(&path);
                animated_image_preview_command(path, generation, max_file_bytes)
            }
            PreviewPathKind::Image => {
                // 图片先完成尺寸探测，窗口按内容尺寸在探测回流时开启
                // （accept_image_preview_dimensions）。
                self.engine.preview = Some(PreviewState::Loading(path.clone()));
                let generation = self.engine.next_original_image_preview_generation();
                image_preview_dimensions_command(path, generation)
            }
            PreviewPathKind::Video => {
                self.engine.preview = Some(PreviewState::Loading(path.clone()));
                let max_file_bytes = self.file_size_limit_for(&path);
                preview_command(
                    path,
                    FileKind::File,
                    rules,
                    ScanOptions::default(),
                    max_file_bytes,
                )
            }
            PreviewPathKind::Audio => {
                let window_command = self
                    .engine
                    .preview_window_presentation_command(PreviewWindowProfile::Audio);
                self.engine.clear_preview();
                self.engine.preview = Some(PreviewState::Loading(path.clone()));
                self.engine.audio_preview = Some(AudioPreviewPlayback::loading(path.clone()));
                let max_file_bytes = self.file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    preview_command(
                        path.clone(),
                        FileKind::File,
                        rules,
                        ScanOptions::default(),
                        max_file_bytes,
                    ),
                    start_audio_preview_command(path),
                ])
            }
            PreviewPathKind::Archive | PreviewPathKind::Sqlite | PreviewPathKind::Text => {
                let window_command = self
                    .engine
                    .preview_window_presentation_command(PreviewWindowProfile::Regular);
                self.engine.clear_preview();
                self.engine.preview = Some(PreviewState::Loading(path.clone()));
                let max_file_bytes = self.file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    preview_command(
                        path,
                        FileKind::File,
                        rules,
                        ScanOptions::default(),
                        max_file_bytes,
                    ),
                ])
            }
        }
    }

    /// 会话级偏好重读：读主软件 state.sqlite 的预览段，失败回落共享
    /// crate 默认并记日志；引擎快照同步刷新。
    fn refresh_session_preferences(&mut self) {
        self.preferences = load_preview_preferences_from_state_database(&self.preferences_database)
            .unwrap_or_else(|error| {
                tracing::warn!(
                    target: "portal::preview",
                    error = %error,
                    "预览偏好读取失败，回落共享 crate 默认规则"
                );
                PreviewPreferences::default_preferences()
            });
        let preferences = &self.preferences;
        self.engine.config = Self::engine_config(preferences);
    }

    /// 引擎呈现预览窗口后的焦点簿记（引擎登记、宿主认领，幂等）。
    fn sync_focused_window(&mut self) {
        if let Some(window) = self.engine.take_pending_preview_window_focus() {
            self.focused_window = Some(window);
        }
    }
}

/// 从选择窗会话提取空格预览目标：预览目标条目唯一读取口（列表/大
/// 图 = 主选中行；多栏 = 焦点栏锚点条目，见 view_mode 子模块）。
pub(crate) fn preview_selection(session: &PickerSession) -> Option<PreviewSelection> {
    let entry = session.preview_target_entry()?;
    Some(PreviewSelection {
        path: entry.path.clone(),
        kind: entry.kind,
        file_bytes: entry.metadata.len,
    })
}

/// Space toggle 的 update 入口（keyboard_route 经延迟消息转入）：
/// 来源窗的选中项驱动 toggle；来源窗已亡时仅执行关闭路径。
pub(crate) fn handle_space_pressed(
    daemon: &mut PickerDaemon,
    source: Option<window::Id>,
) -> Task<Message> {
    let selection = source
        .and_then(|window| daemon.windows.get(&window))
        .and_then(preview_selection);
    daemon.preview.request_preview(source, selection.as_ref())
}

/// 预览窗生命周期与指针/滚动事件的宿主侧入口（app-ui windows.rs /
/// pointer_interactions.rs 预览分支的对齐物）。非预览窗一律 no-op，
/// main 层无需预过滤。
impl PreviewHost {
    /// Esc（预览窗聚焦，无视 captured）与 chrome 关闭按钮共用：只关
    /// 预览（主软件 handle_focused_window_escape_pressed 的 Preview 分支）。
    pub(crate) fn close_focused_preview(&mut self) -> Task<Message> {
        self.close_preview_session().map(Message::Preview)
    }

    /// chrome 控制按钮：最小化 / 最大化切换（带最大化观测）/ 关闭。
    pub(crate) fn handle_window_control(
        &mut self,
        window: window::Id,
        kind: WindowControlKind,
    ) -> Task<Message> {
        if !self.is_preview_window(window) {
            return Task::none();
        }
        match kind {
            WindowControlKind::Minimize => window::minimize(window, true),
            WindowControlKind::MaximizeRestore => self.toggle_window_maximized(window),
            WindowControlKind::Close => self.close_focused_preview(),
        }
    }

    /// 标题栏按下：单击拖动（取消初始 chrome 淡出计时），双击最大化。
    pub(crate) fn handle_title_bar_pressed(
        &mut self,
        window: window::Id,
        double_click: bool,
    ) -> Task<Message> {
        if !self.is_preview_window(window) {
            return Task::none();
        }
        if double_click {
            return self.toggle_window_maximized(window);
        }
        self.engine.cancel_preview_window_initial_chrome_hide();
        self.engine.preview_window_drag_active = true;
        self.engine.preview_window_chrome.start_reveal();
        window::drag(window)
    }

    /// resize 边缘按下 → 原生 drag_resize（边缘仅在非最大化时存在）。
    pub(crate) fn handle_resize_edge_pressed(
        &mut self,
        window: window::Id,
        direction: window::Direction,
    ) -> Task<Message> {
        if !self.is_preview_window(window) {
            return Task::none();
        }
        window::drag_resize(window, direction)
    }

    fn toggle_window_maximized(&mut self, window: window::Id) -> Task<Message> {
        window::toggle_maximize(window).chain(
            window::is_maximized(window)
                .map(move |maximized| Message::PreviewWindowMaximizeObserved { window, maximized }),
        )
    }

    /// 最大化观测回信（主软件 accept_window_maximized_observation 的
    /// 预览分支：只登记 chrome 按钮形态，无其它窗口簿记）。
    pub(crate) fn accept_maximize_observed(&mut self, window: window::Id, maximized: bool) {
        if self.is_preview_window(window) {
            self.preview_window_maximized = maximized;
        }
    }

    /// 预览窗失焦自动关闭的门禁（引擎判定：预览窗且未钉住）。
    pub(crate) fn handle_window_unfocused(&mut self, window: window::Id) -> Task<Message> {
        if self.engine.should_close_unfocused_window(window) {
            return self.close_focused_preview();
        }
        Task::none()
    }

    /// 预览窗尺寸变化（主软件 handle_auxiliary_window_resized 的预览
    /// 分支）：pending resize 匹配语义（未到目标重发 resize）+ 底部控件
    /// 刷新 + 文档重排 + 缩略图档位刷新；档位刷新需会话缩略图体系，
    /// 以自由函数收 daemon（同 handle_space_pressed 模式）。
    pub(crate) fn handle_window_resized_engine(
        &mut self,
        window: window::Id,
        width: f32,
        height: f32,
    ) -> Option<Task<Message>> {
        if !self.is_preview_window(window) {
            return Some(Task::none());
        }
        let resized_size =
            preview_content_size_from_window(self.engine.preview_window_profile, width, height);
        if let Some(pending_size) = self.engine.pending_preview_resize {
            if preview_size_matches(resized_size, pending_size) {
                self.engine.pending_preview_resize = None;
                self.engine.preview_size = resized_size;
            } else {
                tracing::debug!(
                    target: "portal::preview",
                    window = ?window,
                    actual_width = resized_size.width,
                    actual_height = resized_size.height,
                    pending_width = pending_size.width,
                    pending_height = pending_size.height,
                    "preview resize still pending"
                );
                return Some(window::resize(
                    window,
                    preview_window_size_for_content(pending_size),
                ));
            }
        } else {
            self.engine.preview_size = resized_size;
        }
        self.engine.refresh_preview_window_bottom_controls();
        None
    }

    /// 预览窗指针移动：媒体类驱动 chrome/底部控件淡入淡出；标准窗口
    /// chrome（文本/SQLite）只推进表格列宽拖拽（主软件 update_pointer_
    /// motion 的预览分支）。
    pub(crate) fn handle_pointer_moved(
        &mut self,
        window: window::Id,
        position: Point,
    ) -> Task<Message> {
        if !self.is_preview_window(window) {
            return Task::none();
        }
        if self.engine.preview_window_uses_window_chrome() {
            self.engine.update_sqlite_tables_resize_drag(position);
            self.preview_window_pointer = Some(position);
            return Task::none();
        }
        self.engine.cancel_preview_window_initial_chrome_hide();
        self.engine.preview_window_pointer_y = Some(position.y);
        self.preview_window_pointer = Some(position);
        self.engine.refresh_preview_window_bottom_controls();
        if !self.engine.preview_window_drag_active {
            self.engine
                .preview_window_chrome
                .update_for_cursor_y(position.y);
        }
        Task::none()
    }

    /// 指针离开预览窗：隐藏 chrome/底部控件；按着左键离开时就地结束
    /// 图片平移（窗口外收不到 release，主软件 CursorLeft 同款）。
    pub(crate) fn handle_pointer_left(&mut self, window: window::Id) -> Task<Message> {
        if !self.is_preview_window(window) {
            return Task::none();
        }
        self.engine.cancel_preview_window_initial_chrome_hide();
        self.engine.preview_window_pointer_y = None;
        self.preview_window_pointer = None;
        self.engine.preview_image_viewport.panning = false;
        self.engine.preview_window_bottom_controls.start_hide();
        if !self.engine.preview_window_drag_active
            && !self.engine.preview_window_uses_window_chrome()
        {
            self.engine.preview_window_chrome.start_hide();
        }
        Task::none()
    }

    /// 预览窗内左键释放 / SQLite 列宽拖拽收尾（面板闭包与全局释放共用，
    /// 主软件 finish_pointer_drag_interactions 的预览子集）。
    pub(crate) fn finish_window_drags(&mut self) -> Task<Message> {
        if self.engine.preview_window_drag_active {
            self.engine.preview_window_drag_active = false;
            if let Some(pointer_y) = self.engine.preview_window_pointer_y {
                self.engine
                    .preview_window_chrome
                    .update_for_cursor_y(pointer_y);
                self.engine.refresh_preview_window_bottom_controls();
            } else {
                self.engine.preview_window_chrome.start_hide();
                self.engine.preview_window_bottom_controls.start_hide();
            }
        }
        self.engine.preview_image_viewport.panning = false;
        self.engine
            .finish_sqlite_tables_resize_drag()
            .map(Message::Preview)
    }

    /// 帧时钟是否需要挂载（chrome 淡入淡出 ∪ 底部控件 ∪ 预览树动画 ∪
    /// 滚动管线；与选择窗 is_animating 联合后驱动 AnimationTick）。
    pub(crate) fn is_animating(&self) -> bool {
        self.engine.preview_window_chrome.is_animating()
            || self.engine.preview_window_bottom_controls.is_animating()
            || self.engine.preview_tree_animation_is_active()
            || self.scroll.is_animating()
    }

    /// 帧推进：chrome/底部控件透明度、预览树展开动画、惯性滚动与滚动条
    /// 透明度（app-ui advance_window_animation_frame + WindowChrome-
    /// AnimationTick/PreviewTreeAnimationTick 两条订阅的合并驱动）。
    pub(crate) fn advance_frame(&mut self) -> Task<Message> {
        self.engine.preview_window_chrome.advance();
        self.engine.preview_window_bottom_controls.advance();
        let tree_task = self.engine.advance_preview_tree_animation();
        let scroll_task = self.scroll.advance_frame();
        Task::batch([tree_task.map(Message::Preview), scroll_task])
    }
}

#[cfg(test)]
mod tests;
