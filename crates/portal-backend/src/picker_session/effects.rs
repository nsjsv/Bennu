//! 状态机对外请求的效果枚举：从 mod.rs 拆出以守住其 800 行上限。
//! 会话层不触碰 iced 类型，效果由 main 层翻译成 Task / IO，保证逻辑
//! 可脱离窗口环境单测。

use std::path::PathBuf;

use bennu_theme::address_bar::AddressSuggestionRequest;

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
    /// 多栏打开新栏需要扫描：main 层翻译为扫描 + 横向栏容器滚到最右
    /// （新栏自动可见）+ 布局探针。
    ColumnScanAndReveal(PathBuf),
    /// 多栏栏链延伸（无需扫描，内容已有缓存）：横向栏容器滚到最右。
    ScrollColumnsRailToEnd,
    /// 多栏键盘揭示：把第 lane 栏滚到绝对 Y 偏移。main 层翻译为
    /// scroll_to + 该栏布局探针。
    ScrollColumnLaneTo {
        lane: usize,
        offset_y: f32,
    },
}
