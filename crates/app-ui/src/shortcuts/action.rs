use super::key_press::KeyPress;

#[path = "action_table.rs"]
mod table;

use table::ACTIONS;

/// 能在设置里改键的命令。移动键、Esc、Alt+数字是固定键（`fixed_keys`），字母层另算。
/// 变体顺序就是 `ACTIONS` 的行序（设置页和一览按它排），两者靠测试对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ShortcutAction {
    NavigateBack,
    NavigateForward,
    NavigateUp,
    NavigateHome,
    FocusPathInput,
    Search,
    Refresh,
    ToggleHiddenFiles,
    OpenSelected,
    OpenInNewTab,
    OpenInNewWindow,
    RenameSelected,
    NewFolder,
    NewFile,
    NewFolderFromSelection,
    Copy,
    Cut,
    Paste,
    DuplicateSelected,
    CopyPath,
    CreateSymlink,
    Delete,
    DeletePermanently,
    Undo,
    Redo,
    FileProperties,
    AddToFavorites,
    OpenContextMenu,
    SelectAll,
    InvertSelection,
    NewTab,
    CloseTab,
    RestoreClosedTab,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
    NewWindow,
    CloseWindow,
    ToggleSplit,
    FocusOtherPane,
    CopyToOtherPane,
    MoveToOtherPane,
    ColumnsView,
    ListView,
    IconsView,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Preview,
    ToggleRightPreviewPanel,
    ToggleTerminal,
    OpenExternalTerminal,
    ToggleSidebar,
    OpenSettings,
    ShowShortcuts,
    OpenSuperBox,
    TerminalNewTab,
    TerminalCloseTab,
}

/// 设置页和快捷键一览里的分组，按这个顺序排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ShortcutGroup {
    Navigation,
    Files,
    Selection,
    TabsAndWindows,
    SplitView,
    View,
    Panels,
    Terminal,
}

/// 按键在哪里算数。终端有焦点时键盘归终端，只有 `Terminal` 和 `Everywhere` 的键还归 Bennu；
/// 其余时候是 `FileBrowser` 和 `Everywhere`。不同作用域可以用同一个键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutScope {
    Everywhere,
    FileBrowser,
    Terminal,
}

impl ShortcutScope {
    pub(crate) fn overlaps(self, other: Self) -> bool {
        self == Self::Everywhere || other == Self::Everywhere || self == other
    }
}

/// 输入框被别的控件捕获时还执行不执行：`Application` 照样执行（导航、标签页、视图、面板），
/// `FileBrowserContent` 让给控件，并且要先确认没有文本框焦点。编辑键另有规则，见 `routing_context`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredContext {
    Application,
    FileBrowserContent,
}

struct ActionSpec {
    action: ShortcutAction,
    /// 存储里的名字；改名要在 `overrides` 的旧数据迁移里留旧名。
    config_key: &'static str,
    label: fn() -> String,
    group: ShortcutGroup,
    scope: ShortcutScope,
    context: DeclaredContext,
    keys: &'static [KeyPress],
}

const ALL_ACTIONS: [ShortcutAction; ACTIONS.len()] = {
    let mut actions = [ShortcutAction::NavigateBack; ACTIONS.len()];
    let mut index = 0;
    while index < ACTIONS.len() {
        actions[index] = ACTIONS[index].action;
        index += 1;
    }
    actions
};

impl ShortcutAction {
    pub(crate) fn all() -> &'static [Self] {
        &ALL_ACTIONS
    }

    /// 在 `all()` 里的位置；变体顺序与 `ACTIONS` 行序一致（`variants_follow_the_table_order`）。
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    fn spec(self) -> &'static ActionSpec {
        &ACTIONS[self.index()]
    }

    pub(crate) fn default_keys(self) -> &'static [KeyPress] {
        self.spec().keys
    }

    pub(crate) fn label(self) -> String {
        (self.spec().label)()
    }

    pub(crate) fn group(self) -> ShortcutGroup {
        self.spec().group
    }

    pub(crate) fn scope(self) -> ShortcutScope {
        self.spec().scope
    }

    pub(crate) fn declared_context(self) -> DeclaredContext {
        self.spec().context
    }

    pub(crate) fn config_key(self) -> &'static str {
        self.spec().config_key
    }

    pub(crate) fn from_config_key(key: &str) -> Option<Self> {
        ACTIONS
            .iter()
            .find(|spec| spec.config_key == key)
            .map(|spec| spec.action)
    }
}

impl ShortcutGroup {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Navigation => tr!("Navigation"),
            Self::Files => tr!("Files"),
            Self::Selection => tr!("Selection"),
            Self::TabsAndWindows => tr!("Tabs and Windows"),
            Self::SplitView => tr!("Split View"),
            Self::View => tr!("View"),
            Self::Panels => tr!("Panels"),
            Self::Terminal => tr!("Terminal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_follow_the_table_order() {
        for (index, spec) in ACTIONS.iter().enumerate() {
            assert_eq!(spec.action as usize, index, "{:?}", spec.action);
        }
    }

    #[test]
    fn config_keys_are_unique() {
        for (index, spec) in ACTIONS.iter().enumerate() {
            assert!(
                ACTIONS[index + 1..]
                    .iter()
                    .all(|other| other.config_key != spec.config_key),
                "{}",
                spec.config_key
            );
        }
    }

    #[test]
    fn groups_are_contiguous_in_table_order() {
        let groups = ACTIONS.iter().map(|spec| spec.group).collect::<Vec<_>>();
        assert!(groups.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
