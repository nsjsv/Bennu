//! 视图事件（会话消息）集合。独立成文件控制 mod.rs 的行数；语义实现
//! 分布在各子模块，本文件只定义消息形状。

use bennu_theme::address_bar::AddressSuggestionRequest;

use super::scrollbar::ScrollbarViewport;
use super::sidebar::SidebarEntryId;
use super::thumbnails::{CachedThumbnail, ThumbnailLoadFailed, ThumbnailRequest};
use super::{DirectoryScanResult, PathSuggestionDirection, PickerViewMode, SessionScrollRegion};

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
        target: std::path::PathBuf,
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
    /// 视图模式切换（导航栏分段按钮）；语义见 view_mode 子模块。
    ViewModeSelected {
        mode: PickerViewMode,
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
        suggestions: Vec<std::path::PathBuf>,
    },
    /// 补全面板行点击：仅当路径仍在当前建议列表中才提交。
    AddressSuggestionSelected {
        path: std::path::PathBuf,
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
    ChoiceSelected {
        id: String,
        value: String,
    },
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
    SidebarDataLoaded(Box<super::sidebar::PickerSidebarData>),
    /// 点击位置/收藏行：导航到对应目录。
    SidebarLocationPressed {
        path: std::path::PathBuf,
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
        mount_path: Result<std::path::PathBuf, String>,
    },
    SidebarConnectionMountFinished {
        id: desktop_linux::NetworkConnectionId,
        mount_path: Result<std::path::PathBuf, String>,
    },
    /// 侧边栏行悬停变化；None = 离开所有行。
    SidebarHoverChanged {
        entry: Option<SidebarEntryId>,
    },
    /// 拖宽手柄按下：main 层拦截处理（拖宽的指针增量归 main 的指针
    /// 簿记，iced::Point 不进会话层），本臂仅为穷尽兑底。
    SidebarResizeStarted,
    /// 多栏条目点击（视图报裸点击，ctrl/shift 由 main 层合成）。
    ColumnEntryClicked {
        lane: usize,
        index: usize,
        ctrl: bool,
        shift: bool,
    },
    /// 多栏条目双击/Enter 激活。
    ColumnEntryDoubleClicked {
        lane: usize,
        index: usize,
    },
    /// 多栏栏内悬停变化；None = 离开该栏所有行。
    ColumnEntryHovered {
        lane: usize,
        index: Option<usize>,
    },
    /// 多栏 ←：焦点退回父栏（光标落在打开原焦点栏的那条目录上）。
    ColumnsBackwardRequested,
    /// 多栏 →：进入焦点栏光标所在目录（光标落新栏首项）。
    ColumnsForwardRequested,
}
