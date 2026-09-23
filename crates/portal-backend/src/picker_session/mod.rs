//! 每个选择窗口的核心状态机：目录导航（含历史栈）、条目选择、目录原地
//! 展开（访达列表语义）、SaveFile 命名与覆盖确认、地址栏编辑。不触碰
//! iced 类型，效果以 `SessionEffect` 输出由上层翻译执行，保证逻辑可
//! 脱离窗口环境单测。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bennu_theme::address_bar::{
    AddressBarTransition, AddressEditingSession, AddressSuggestionRequest,
};
use tokio::sync::oneshot;

use file_core::entry::{DirectoryEntry, FileKind};
use thumbnails::{CachedThumbnail, ThumbnailLoadFailed, ThumbnailRequest};

use crate::dbus_file_chooser::PickerResolution;
use crate::filter::PickerFilter;
use crate::picker_request::{FilterRule, PickerChoice, PickerKind, PickerRequestSpec};

mod address_editing;
mod choices;
mod confirm;
mod expansion;
mod keyboard_nav;
pub(crate) mod scan;
pub(crate) mod scrollbar;
mod selection;
mod sidebar;
pub(crate) mod suggestions;
pub(crate) mod thumbnails;

/// 行几何唯一真值再导出：view 层行高/间距与各子模块同源（模块本身
/// 保持私有，只放行这两个常量）。
pub(crate) use expansion::{LIST_ROW_HEIGHT, LIST_ROW_SPACING};
pub(crate) use scan::{scan_listing, scan_trash_listing, DirectoryListing, DirectoryScanResult};
pub(crate) use scrollbar::SessionScrollRegion;
use scrollbar::{ScrollbarViewport, SessionScrollbarState, SmoothScrollState};
pub(crate) use sidebar::{
    load_sidebar_data, PickerSidebarData, PickerSidebarState, SidebarEntryId, TRASH_DIRECTORY,
};

pub(crate) use address_editing::PathSuggestionDirection;
use expansion::ExpansionState;
use keyboard_nav::KeyboardNavState;

/// 状态机对外请求的效果。
pub(crate) enum SessionEffect {
    None,
    /// 导航类扫描（进入/上级/面包屑/初始 begin）：main 层除扫描外还
    /// 要把面包屑滚到当前段。与列表展开的 `ScanDirectory` 拆开，是
    /// 为了让「滚尾」只挂在导航上——展开行不该强滚面包屑。
    NavigateDirectory(PathBuf),
    /// 列表展开行扫描：结果按路径回填到展开节点，不触发面包屑滚尾。
    ScanDirectory(PathBuf),
    /// 用户确认，携带选中路径。
    Confirmed(Vec<PathBuf>),
    /// 用户取消（Esc / 取消按钮）。
    Dismissed,
    /// 窗口打开时一次性读取侧边栏数据（位置/收藏/设备/网络连接），
    /// 与每窗重解析主题同模式；回信 `SidebarDataLoaded`。
    LoadSidebarData,
    /// 侧边栏未挂载设备挂载：回信 `SidebarDeviceMountFinished`。
    MountDevice(desktop_linux::StorageDeviceId),
    /// 侧边栏网络连接挂载（先查主程序已存凭证）：回信
    /// `SidebarConnectionMountFinished`。
    MountConnection(desktop_linux::NetworkConnectionId),
    /// 列表/面包屑内容骤变后核实滚动条溢出：布局探针回信自愈视口
    /// 缓存，塞得下则淡入被拦截。main 层翻译为探针 Task。
    VerifyScrollbarLayout,
    /// 地址输入停笔 120ms 防抖：main 层 sleep 后回信
    /// `AddressSuggestionInputStabilized`，会话校验凭据再发起读取。
    StabilizeAddressInput {
        request: AddressSuggestionRequest,
    },
    /// 读取目录补全建议：main 层 `Task::perform(suggestions::load)` 后
    /// 回信 `AddressSuggestionsLoaded`。
    LoadPathSuggestions {
        request: AddressSuggestionRequest,
    },
    /// 键盘导航滚动跟随：把列表滚动到绝对 Y 偏移（贴边值）。main 层
    /// 翻译为 scroll_to + List 布局探针（联动滚动条显隐与视口缓存）。
    ScrollListTo {
        offset_y: f32,
    },
}

/// 视图事件。
#[derive(Debug, Clone)]
pub(crate) enum SessionMessage {
    ScanReady(Box<DirectoryScanResult>),
    EntryClicked {
        index: usize,
        ctrl: bool,
        shift: bool,
    },
    EntryDoubleClicked {
        index: usize,
    },
    /// 目录行的展开/收起开关（访达列表语义）。
    EntryExpandToggled {
        index: usize,
    },
    NavigateUp,
    NavigateBack,
    NavigateForward,
    BreadcrumbActivated {
        target: PathBuf,
    },
    FilterSelected {
        rule: usize,
    },
    NameInputChanged(String),
    ConfirmPressed,
    DismissPressed,
    OverwriteDeclined,
    /// 下拉选择值不在已知规则中（视图与状态不一致的兜底，正常不可达）。
    FilterSelectionIgnored,
    /// 指针悬停变化；None = 离开所有行。索引以当前扁平化行列表为准。
    EntryHovered {
        index: Option<usize>,
    },
    /// Ctrl+A 全选可选中条目（仅 OpenFile 多选模式有意义）。
    SelectAllPressed,
    /// 键盘列表光标移动（↑/↓）；语义实现见 keyboard_nav 子模块。
    ListCursorMoved {
        delta: isize,
    },
    /// 键盘跳转列表首/尾（Home/End）。
    ListCursorJumped {
        to_end: bool,
    },
    /// 键盘按视口行数翻页（PageUp/PageDown）。
    ListPageMoved {
        pages: isize,
    },
    /// type-ahead 字符输入（无 ctrl/alt/command 修饰的字符键）。
    TypeAheadChar {
        ch: char,
    },
    /// 清空 type-ahead 缓冲（Esc 优先级：缓冲非空时先于关窗）。
    TypeAheadReset,
    /// 点击地址栏空白处进入路径编辑（草稿预填当前目录）。
    AddressEditingStarted,
    AddressEditChanged(String),
    AddressEditingSubmitted,
    AddressEditingCancelled,
    /// 防抖停笔回信：携带发请求时的凭据，陈旧（改稿/换目录/退出编辑
    /// 后）回信按凭据拒收。
    AddressSuggestionInputStabilized {
        request: AddressSuggestionRequest,
    },
    /// 补全建议读取结果回信：同样按凭据拒收陈旧结果。
    AddressSuggestionsLoaded {
        request: AddressSuggestionRequest,
        suggestions: Vec<PathBuf>,
    },
    /// 补全面板行点击：仅当路径仍在当前建议列表中才提交。
    AddressSuggestionSelected {
        path: PathBuf,
    },
    /// 键盘循环选择建议（↓/↑/Tab/Shift+Tab）。
    MoveSuggestionSelection {
        direction: PathSuggestionDirection,
    },
    /// 键盘补全：选中建议写入草稿并请求下一级建议。
    CompleteSuggestion {
        direction: PathSuggestionDirection,
    },
    /// choices 控件变更（下拉选中/复选开关）：按 id 定位覆写选中值，
    /// 见 choices 子模块；id 不存在静默忽略。
    ChoiceSelected { id: String, value: String },
    /// 滚轮输入（视图包装层捕获后发布）；增量换算在会话滚动子模块。
    WheelScrolled {
        region: SessionScrollRegion,
        delta: iced::mouse::ScrollDelta,
    },
    /// 布局探针回信：当帧滚动区布局（含溢出裁决依据）。
    ScrollbarLayoutVerified {
        region: SessionScrollRegion,
        viewport: ScrollbarViewport,
    },
    /// 滚动条视口回传（on_scroll 持续刷缓存），内层事件继续按滚动消息路由。
    ScrollbarViewportChanged {
        region: SessionScrollRegion,
        viewport: ScrollbarViewport,
        event: Box<SessionMessage>,
    },
    /// 滚动条被直接交互（拖动/轨道点击）触发的视口回传内层事件。
    ScrollbarEngaged {
        region: SessionScrollRegion,
    },
    /// 650ms 无输入的 auto-hide 回信；代数不匹配（期间又有滚动）则忽略。
    ScrollbarAutoHideElapsed {
        generation: u64,
    },
    /// 缩略图加载回信（main 层 drain 出队后 Task::perform 的结果）；
    /// key 已不在 in_flight 的迟到回信（换目录后）由会话直接丢弃。
    ThumbnailReady {
        request: ThumbnailRequest,
        outcome: Result<CachedThumbnail, ThumbnailLoadFailed>,
    },
    /// 侧边栏数据回填（窗口打开时一次性读取）。
    SidebarDataLoaded(Box<PickerSidebarData>),
    /// 点击位置/收藏行：导航到对应目录。
    SidebarLocationPressed {
        path: PathBuf,
    },
    /// 点击垃圾桶行：进入 trash:/// 虚拟视图。
    SidebarTrashPressed,
    SidebarDevicePressed {
        id: desktop_linux::StorageDeviceId,
    },
    SidebarConnectionPressed {
        id: desktop_linux::NetworkConnectionId,
    },
    SidebarDeviceMountFinished {
        id: desktop_linux::StorageDeviceId,
        mount_path: Result<PathBuf, String>,
    },
    SidebarConnectionMountFinished {
        id: desktop_linux::NetworkConnectionId,
        mount_path: Result<PathBuf, String>,
    },
    /// 侧边栏行悬停变化；None = 离开所有行。
    SidebarHoverChanged {
        entry: Option<sidebar::SidebarEntryId>,
    },
    /// 拖宽手柄按下：main 层拦截处理（拖宽的指针增量归 main 的指针
    /// 簿记，iced::Point 不进会话层），本臂仅为穷尽兑底。
    SidebarResizeStarted,
}

pub(crate) use expansion::PickerRow;

pub(crate) struct PickerSession {
    request_path: String,
    kind: PickerKind,
    accept_label: Option<String>,
    /// 调用方指定的窗口标题；仅作显示，空缺时回落模式默认标题。
    title: Option<String>,
    filters: Vec<FilterRule>,
    active_filter: PickerFilter,
    directory: PathBuf,
    listing: DirectoryListing,
    /// 已展开目录：路径 → 子内容。导航切换目录时整体清空。
    expansions: HashMap<PathBuf, ExpansionState>,
    /// 扁平化可见行（根 + 展开子级），`refresh_rows` 是唯一重建点。
    rows: Vec<PickerRow>,
    selection: Vec<usize>,
    selection_anchor: Option<usize>,
    /// 指针悬停的行索引；导航/刷新后必须重新验证或清空。
    hovered_index: Option<usize>,
    /// 键盘列表导航状态（光标行 + type-ahead 缓冲），见子模块。
    keyboard_nav: KeyboardNavState,
    name_input: String,
    /// 调用方 choices 选项（选中值由 ChoiceSelected 簿记，回信时按
    /// 顺序回传）；三种模式都可携带。
    choices: Vec<PickerChoice>,
    /// SaveFile/SaveFiles 二次确认目标集：非空时确认按钮变为"覆盖"。
    /// SaveFile 场景即长度 1；SaveFiles 只装冲突子集（确认后仍返回
    /// 全部目标）。
    overwrite_targets: Vec<PathBuf>,
    /// 地址栏编辑会话（共享层模型）；None = 面包屑态。
    address_editing: Option<AddressEditingSession>,
    /// 面包屑 ↔ 编辑渐变过渡；退出方向携带草稿快照供渐出帧渲染。
    address_bar_transition: Option<AddressBarTransition>,
    /// 面包屑 Home 段折叠基准：共享 breadcrumb_segments 用它把家目录
    /// 收成 House 图标。
    home_dir: PathBuf,
    /// 编辑会话 id 发号器：防陈旧凭据的会话身份部分，跨进入/退出
    /// 单调递增，旧会话的迟到回信永远对不上新会话。
    next_address_editing_session_id: u64,
    /// 目录访问历史；`history_position` 指向当前目录（侧键后退/前进）。
    history: Vec<PathBuf>,
    history_position: usize,
    /// 滚轮 Mos 惯性状态机与滚动条显隐状态：逻辑见子模块 scrollbar。
    smooth_scroll: SmoothScrollState,
    scrollbar: SessionScrollbarState,
    /// 侧边栏子状态（数据/宽度/拖宽/提示），见 sidebar 子模块。
    sidebar: PickerSidebarState,
    /// 滚动轴向换算用的 shift 修饰键（main 层同步；视图与处理端同源）。
    shift_pressed: bool,
    /// 行内缩略图调度状态（就绪 LRU/在途/排队/失败 backoff），见子模块。
    thumbnails: thumbnails::SessionThumbnailState,
    reply: Option<oneshot::Sender<PickerResolution>>,
}

impl PickerSession {
    pub(crate) fn new(
        invocation_spec: &PickerRequestSpec,
        request_path: String,
        start_directory: PathBuf,
        reply: oneshot::Sender<PickerResolution>,
    ) -> Self {
        let PickerRequestSpec {
            kind,
            accept_label,
            title,
            filters,
            active_filter,
            choices,
            ..
        } = invocation_spec;
        let active_filter = active_filter
            .and_then(|index| filters.get(index))
            .map(|rule| PickerFilter::from_rules(vec![rule.clone()]))
            .unwrap_or_else(|| {
                if filters.is_empty() {
                    PickerFilter::unconstrained()
                } else {
                    PickerFilter::from_rules(filters[..1].to_vec())
                }
            });
        let name_input = match kind {
            PickerKind::SaveFile { default_name } => default_name.clone().unwrap_or_default(),
            _ => String::new(),
        };
        PickerSession {
            request_path,
            kind: kind.clone(),
            accept_label: accept_label.clone(),
            title: title.clone(),
            filters: filters.clone(),
            active_filter,
            directory: start_directory.clone(),
            listing: DirectoryListing::Pending,
            expansions: HashMap::new(),
            rows: Vec::new(),
            selection: Vec::new(),
            selection_anchor: None,
            hovered_index: None,
            keyboard_nav: KeyboardNavState::default(),
            name_input,
            choices: choices.clone(),
            overwrite_targets: Vec::new(),
            address_editing: None,
            address_bar_transition: None,
            home_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            next_address_editing_session_id: 0,
            history: vec![start_directory],
            history_position: 0,
            smooth_scroll: SmoothScrollState::default(),
            scrollbar: SessionScrollbarState::default(),
            sidebar: PickerSidebarState::default(),
            shift_pressed: false,
            thumbnails: thumbnails::SessionThumbnailState::new(
                thumbnails::default_thumbnail_cache_dir(),
            ),
            reply: Some(reply),
        }
    }

    pub(crate) fn request_path(&self) -> &str {
        &self.request_path
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn listing(&self) -> &DirectoryListing {
        &self.listing
    }

    pub(crate) fn rows(&self) -> &[PickerRow] {
        &self.rows
    }

    pub(crate) fn selection(&self) -> &[usize] {
        &self.selection
    }

    /// 空格预览的目标行（主软件 `selected` 的对齐物）：最后交互行——
    /// 单击/键盘光标都落锚；悬停不作依据（纯键盘操作没有悬停）。
    pub(crate) fn primary_selected_row(&self) -> Option<usize> {
        self.selection_anchor
            .filter(|&index| index < self.rows.len())
    }

    pub(crate) fn hovered_index(&self) -> Option<usize> {
        self.hovered_index
    }

    pub(crate) fn filters(&self) -> &[FilterRule] {
        &self.filters
    }

    pub(crate) fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub(crate) fn active_filter_label(&self) -> String {
        self.active_filter.label()
    }

    pub(crate) fn kind(&self) -> &PickerKind {
        &self.kind
    }

    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub(crate) fn name_input(&self) -> &str {
        &self.name_input
    }

    pub(crate) fn overwrite_targets(&self) -> &[PathBuf] {
        &self.overwrite_targets
    }

    /// 确认按钮文案：调用方 accept_label 优先，缺省按模式定。
    pub(crate) fn accept_button_label(&self) -> String {
        if let Some(label) = &self.accept_label {
            return label.clone();
        }
        match &self.kind {
            PickerKind::OpenFile { .. } => "打开".to_string(),
            PickerKind::SaveFile { .. } | PickerKind::SaveFiles { .. } => {
                if self.overwrite_targets.is_empty() {
                    "保存".to_string()
                } else {
                    "覆盖".to_string()
                }
            }
        }
    }

    /// 确认按钮是否可点：SaveFile 在回收站视图下禁止确认（无法把
    /// 文件保存进 trash:/// 虚拟路径）。
    pub(crate) fn can_confirm(&self) -> bool {
        match &self.kind {
            PickerKind::OpenFile { .. } => !self.selection.is_empty(),
            PickerKind::SaveFile { .. } => {
                !self.is_trash_view() && !self.name_input.trim().is_empty()
            }
            // 名字列表是调用方资产：非空即可确认，目标目录仅在回收站
            // 视图下禁用（与 SaveFile 同一守卫语义）。
            PickerKind::SaveFiles { default_names } => {
                !self.is_trash_view() && !default_names.is_empty()
            }
        }
    }

    /// 初始进入扫描（含侧边栏数据加载）。
    pub(crate) fn begin(&mut self) -> Vec<SessionEffect> {
        vec![
            SessionEffect::LoadSidebarData,
            SessionEffect::NavigateDirectory(self.directory.clone()),
        ]
    }

    pub(crate) fn update(&mut self, message: SessionMessage) -> SessionEffect {
        match message {
            SessionMessage::ScanReady(result) => {
                self.apply_scan(*result);
                // 扫描回填改变列表/面包屑内容高度：核实滚动条溢出。
                SessionEffect::VerifyScrollbarLayout
            }
            SessionMessage::EntryClicked { index, ctrl, shift } => {
                self.click_entry(index, ctrl, shift);
                SessionEffect::None
            }
            SessionMessage::EntryDoubleClicked { index } => self.activate_entry(index),
            SessionMessage::EntryExpandToggled { index } => self.toggle_expansion(index),
            SessionMessage::NavigateUp => self.navigate_up(),
            SessionMessage::NavigateBack => self.navigate_back(),
            SessionMessage::NavigateForward => self.navigate_forward(),
            SessionMessage::BreadcrumbActivated { target } => {
                self.activate_breadcrumb_target(target)
            }
            SessionMessage::FilterSelected { rule } => {
                if let Some(rule) = self.filters.get(rule) {
                    self.active_filter = PickerFilter::from_rules(vec![rule.clone()]);
                    self.refresh_rows();
                }
                // 过滤切换重建行集：滚动内容高度骤变，需核实滚动条溢出。
                SessionEffect::VerifyScrollbarLayout
            }
            SessionMessage::NameInputChanged(text) => {
                self.name_input = text;
                // 改名后覆盖确认失效，需对新名字重新判定。
                self.overwrite_targets.clear();
                SessionEffect::None
            }
            SessionMessage::ConfirmPressed => self.confirm(),
            SessionMessage::DismissPressed => self.finish_cancelled(),
            SessionMessage::OverwriteDeclined => {
                self.overwrite_targets.clear();
                SessionEffect::None
            }
            SessionMessage::FilterSelectionIgnored => SessionEffect::None,
            SessionMessage::EntryHovered { index } => {
                // 悬停索引必须落在当前行列表内，防止刷新后的越界高亮。
                self.hovered_index = index.filter(|&index| index < self.rows.len());
                SessionEffect::None
            }
            SessionMessage::SelectAllPressed => {
                self.select_all();
                SessionEffect::None
            }
            SessionMessage::ListCursorMoved { delta } => self.move_list_cursor(delta),
            SessionMessage::ListCursorJumped { to_end } => self.jump_list_cursor(to_end),
            SessionMessage::ListPageMoved { pages } => self.move_list_page(pages),
            SessionMessage::TypeAheadChar { ch } => self.push_type_ahead_char(ch),
            SessionMessage::TypeAheadReset => {
                self.reset_type_ahead();
                SessionEffect::None
            }
            SessionMessage::AddressEditingStarted => self.begin_address_editing(),
            SessionMessage::AddressEditChanged(text) => self.update_address_draft(text),
            SessionMessage::AddressSuggestionInputStabilized { request } => {
                self.load_stable_address_suggestions(&request)
            }
            SessionMessage::AddressSuggestionsLoaded {
                request,
                suggestions,
            } => self.accept_address_suggestions(&request, suggestions),
            SessionMessage::AddressSuggestionSelected { path } => {
                self.submit_address_suggestion(path)
            }
            SessionMessage::AddressEditingSubmitted => self.submit_address_editing(),
            SessionMessage::AddressEditingCancelled => self.cancel_address_editing(),
            SessionMessage::MoveSuggestionSelection { direction } => {
                self.move_path_suggestion_selection(direction)
            }
            SessionMessage::CompleteSuggestion { direction } => {
                self.complete_path_suggestion(direction)
            }
            SessionMessage::ChoiceSelected { id, value } => {
                self.choice_selected(&id, value);
                SessionEffect::None
            }
            SessionMessage::ThumbnailReady { request, outcome } => {
                self.accept_thumbnail_ready(request, outcome);
                SessionEffect::None
            }
            SessionMessage::SidebarDataLoaded(data) => self.accept_sidebar_data(*data),
            SessionMessage::SidebarLocationPressed { path } => self.sidebar_location_pressed(path),
            SessionMessage::SidebarTrashPressed => self.sidebar_trash_pressed(),
            SessionMessage::SidebarDevicePressed { id } => self.sidebar_device_pressed(id),
            SessionMessage::SidebarConnectionPressed { id } => self.sidebar_connection_pressed(id),
            SessionMessage::SidebarDeviceMountFinished { id, mount_path } => {
                self.sidebar_device_mount_finished(id, mount_path)
            }
            SessionMessage::SidebarConnectionMountFinished { id, mount_path } => {
                self.sidebar_connection_mount_finished(id, mount_path)
            }
            SessionMessage::SidebarHoverChanged { entry } => {
                self.sidebar_hover_changed(entry);
                SessionEffect::None
            }
            SessionMessage::SidebarResizeStarted => {
                // main 层拦截处理（拖宽状态归指针事件层）。
                SessionEffect::None
            }
            // 滚动类消息由 scrollbar 子模块处理并产出 Task，main 层在进入
            // update 前已拦截路由；此臂仅为匹配穷尽兜底。
            SessionMessage::WheelScrolled { .. }
            | SessionMessage::ScrollbarLayoutVerified { .. }
            | SessionMessage::ScrollbarViewportChanged { .. }
            | SessionMessage::ScrollbarEngaged { .. }
            | SessionMessage::ScrollbarAutoHideElapsed { .. } => SessionEffect::None,
        }
    }

    /// 窗口被外部关闭（用户点 X 或 Request.Close）：未回复则视为取消。
    pub(crate) fn window_closed(mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Cancelled);
        }
    }

    pub(crate) fn apply_scan(&mut self, result: DirectoryScanResult) {
        let DirectoryScanResult { directory, outcome } = result;
        if directory == self.directory {
            match outcome {
                Ok(scan) => {
                    self.listing = DirectoryListing::Ready(scan.entries);
                    self.selection.clear();
                    self.selection_anchor = None;
                }
                Err(details) => {
                    self.listing = DirectoryListing::Failed(details);
                    self.selection.clear();
                    self.selection_anchor = None;
                }
            }
            self.refresh_rows();
            return;
        }
        // 展开节点扫描：结果按路径回填；失败的展开自动收起。
        if let Some(state) = self.expansions.get_mut(&directory) {
            match outcome {
                Ok(scan) => state.listing = DirectoryListing::Ready(scan.entries),
                Err(_) => {
                    self.expansions.remove(&directory);
                }
            }
            self.refresh_rows();
        }
        // 既非根也非展开目标（已收起/已导航）：丢弃迟到结果。
    }

    /// 扁平化行列表的唯一重建点：根条目与展开子级在此统一过过滤规则，
    /// 选中/悬停索引在行数变化后重新验证。
    fn refresh_rows(&mut self) {
        self.rows.clear();
        if let DirectoryListing::Ready(entries) = &self.listing {
            let allowed: Vec<DirectoryEntry> = entries
                .iter()
                .filter(|entry| self.is_allowed(entry))
                .cloned()
                .collect();
            self.append_rows(&allowed, 0);
        }
        self.selection.retain(|&index| index < self.rows.len());
        self.hovered_index = self.hovered_index.filter(|&index| index < self.rows.len());
        // 键盘光标与悬停同规则：行集变化后必须重新验证，越界回退到
        // 最近合法行（空列表清空）。
        self.keyboard_nav.revalidate_cursor(self.rows.len());
        self.sync_row_animation();
        // 行集重建即重算可见区间：扫描回填/展开/过滤都在此触发缩略图
        // 请求的积攒，main 层随后 drain 发起。
        self.schedule_visible_thumbnails();
    }

    fn is_allowed(&self, entry: &DirectoryEntry) -> bool {
        self.active_filter
            .entry_allowed(&entry.name.to_string_lossy(), entry.kind, entry.is_hidden)
    }

    fn append_rows(&mut self, entries: &[DirectoryEntry], depth: usize) {
        for entry in entries {
            let has_expansion = self.expansions.contains_key(&entry.path);
            self.rows.push(PickerRow {
                entry: entry.clone(),
                depth,
                height_progress: 1.0,
                expand_progress: 0.0,
            });
            if !has_expansion {
                continue;
            }
            let children =
                self.expansions
                    .get(&entry.path)
                    .and_then(|state| match &state.listing {
                        DirectoryListing::Ready(children) => Some(
                            children
                                .iter()
                                .filter(|child| self.is_allowed(child))
                                .cloned()
                                .collect::<Vec<_>>(),
                        ),
                        DirectoryListing::Pending | DirectoryListing::Failed(_) => None,
                    });
            if let Some(children) = children {
                self.append_rows(&children, depth + 1);
            }
        }
    }

    /// 目录行的展开开关（访达列表语义）：加载中忽略重复点击；再次
    /// 点击进入收起动画（不立即移除）。展开请求通过
    /// `SessionEffect::ScanDirectory` 发出，结果由 `apply_scan` 按
    /// 路径回填。
    fn toggle_expansion(&mut self, index: usize) -> SessionEffect {
        let Some(row) = self.rows.get(index) else {
            return SessionEffect::None;
        };
        if row.entry.kind != FileKind::Directory {
            return SessionEffect::None;
        }
        let path = row.entry.path.clone();
        if let Some(state) = self.expansions.get_mut(&path) {
            if state.is_collapsing {
                // 收起动画中再点 = 反悔：恢复向展开推进（与主应用
                // open_list_directory 同语义），进度保持当前值。
                state.is_collapsing = false;
                return SessionEffect::None;
            }
            if matches!(state.listing, DirectoryListing::Pending) {
                return SessionEffect::None;
            }
            state.is_collapsing = true;
            return SessionEffect::None;
        }
        self.expansions.insert(
            path.clone(),
            ExpansionState {
                listing: DirectoryListing::Pending,
                is_collapsing: false,
                animation_progress: 0.0,
            },
        );
        self.refresh_rows();
        SessionEffect::ScanDirectory(path)
    }

    /// 是否存在进行中的展开/收起动画（含等待扫描的展开节点：其进度
    /// 仍在推进，箭头需要帧时钟驱动）、地址栏渐变过渡、惯性滚动或
    /// 滚动条淡入淡出。
    pub(crate) fn is_animating(&self) -> bool {
        self.expansions
            .values()
            .any(|state| state.is_collapsing || state.animation_progress < 1.0)
            || self
                .address_bar_transition
                .as_ref()
                .is_some_and(|transition| !transition.is_complete())
            || self.smooth_scroll.is_active()
            || self.scrollbar.is_animating()
    }

    /// 进入目录：派生状态（选中、锚点、悬停、展开、地址编辑）的统一
    /// 清理点。历史栈由调用方负责记录。
    fn enter_directory(&mut self, directory: PathBuf) -> SessionEffect {
        self.directory = directory;
        self.listing = DirectoryListing::Pending;
        self.expansions.clear();
        self.rows.clear();
        // 列表内容整体更换：旧视口缓存对新目录是错误几何，不清会让
        // ScanReady 按旧 offset 对新目录错误区间入队；清掉等探针回信
        // （NavigateDirectory 附带布局探针）重填。
        self.scrollbar
            .invalidate_viewport(SessionScrollRegion::List);
        self.selection.clear();
        self.selection_anchor = None;
        self.hovered_index = None;
        // 键盘光标与 type-ahead 缓冲随目录作废。
        self.keyboard_nav.reset();
        // 在途/排队请求作废：迟到回信按 key 找不到在途记录即丢弃；就绪
        // 表保留（返回原目录即时显示）。
        self.thumbnails.clear_pending();
        // 编辑中导航（双击目录/侧键/↑←→）必须走 cancel 而不是裸清空
        // 会话：裸清空会让进行中的过渡停在 target=1.0 永不被回收，
        // 面包屑层 opacity=0 地址栏永久空白，且该态下 Esc 会直接关窗。
        // cancel 无会话时是 no-op，与主软件 navigate_to 首行的
        // cancel_address_editing 同一收口。
        self.cancel_address_editing();
        SessionEffect::NavigateDirectory(self.directory.clone())
    }

    /// 导航到新目录：记录历史（截断前进分支）并进入。
    fn begin_navigation(&mut self, directory: PathBuf) -> SessionEffect {
        if directory == self.directory {
            return SessionEffect::None;
        }
        self.history.truncate(self.history_position + 1);
        self.history.push(directory.clone());
        self.history_position = self.history.len() - 1;
        self.enter_directory(directory)
    }

    fn navigate_back(&mut self) -> SessionEffect {
        if self.history_position == 0 {
            return SessionEffect::None;
        }
        self.history_position -= 1;
        let target = self.history[self.history_position].clone();
        self.enter_directory(target)
    }

    fn navigate_forward(&mut self) -> SessionEffect {
        if self.history_position + 1 >= self.history.len() {
            return SessionEffect::None;
        }
        self.history_position += 1;
        let target = self.history[self.history_position].clone();
        self.enter_directory(target)
    }

    fn navigate_up(&mut self) -> SessionEffect {
        // 回收站虚拟视图没有父目录（"trash:///" 的 parent 是空路径）。
        if self.is_trash_view() {
            return SessionEffect::None;
        }
        let Some(parent) = self.directory.parent().map(Path::to_path_buf) else {
            return SessionEffect::None;
        };
        if parent == self.directory {
            return SessionEffect::None;
        }
        self.begin_navigation(parent)
    }
}

#[cfg(test)]
mod tests;
