use super::key_press::{KeyPress, NamedKey};

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
    SelectAll,
    InvertSelection,
    NewTab,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
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
    OpenSettings,
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

use DeclaredContext::{Application as App, FileBrowserContent as Content};
use ShortcutGroup as G;
use ShortcutScope::FileBrowser as Browser;

const fn ctrl_named(key: NamedKey) -> KeyPress {
    KeyPress {
        control: true,
        ..KeyPress::named(key)
    }
}

const fn ctrl_shift_named(key: NamedKey) -> KeyPress {
    KeyPress {
        control: true,
        shift: true,
        ..KeyPress::named(key)
    }
}

const fn shift_named(key: NamedKey) -> KeyPress {
    KeyPress {
        shift: true,
        ..KeyPress::named(key)
    }
}

const fn ctrl_alt(character: char) -> KeyPress {
    KeyPress {
        alt: true,
        ..KeyPress::ctrl(character)
    }
}

const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        action: ShortcutAction::NavigateBack,
        config_key: "navigate_back",
        label: || tr!("Back"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::alt_named(NamedKey::ArrowLeft)],
    },
    ActionSpec {
        action: ShortcutAction::NavigateForward,
        config_key: "navigate_forward",
        label: || tr!("Forward"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::alt_named(NamedKey::ArrowRight)],
    },
    ActionSpec {
        action: ShortcutAction::NavigateUp,
        config_key: "navigate_up",
        label: || tr!("Up Folder"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[
            KeyPress::alt_named(NamedKey::ArrowUp),
            KeyPress::named(NamedKey::Backspace),
        ],
    },
    ActionSpec {
        action: ShortcutAction::NavigateHome,
        config_key: "navigate_home",
        label: || tr!("Home Folder"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::alt_named(NamedKey::Home)],
    },
    ActionSpec {
        action: ShortcutAction::FocusPathInput,
        config_key: "focus_path_input",
        label: || tr!("Focus Path"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('L')],
    },
    ActionSpec {
        action: ShortcutAction::Search,
        config_key: "search",
        label: || tr!("Search"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('F')],
    },
    ActionSpec {
        action: ShortcutAction::Refresh,
        config_key: "refresh",
        label: || tr!("Refresh"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::named(NamedKey::Function(5)), KeyPress::ctrl('R')],
    },
    ActionSpec {
        action: ShortcutAction::ToggleHiddenFiles,
        config_key: "toggle_hidden_files",
        label: || tr!("Show or Hide Hidden Files"),
        group: G::Navigation,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('H')],
    },
    ActionSpec {
        action: ShortcutAction::OpenSelected,
        config_key: "open_selected",
        label: || tr!("Open"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::named(NamedKey::Enter)],
    },
    ActionSpec {
        action: ShortcutAction::OpenInNewTab,
        config_key: "open_in_new_tab",
        label: || tr!("Open in New Tab"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[ctrl_named(NamedKey::Enter)],
    },
    ActionSpec {
        action: ShortcutAction::RenameSelected,
        config_key: "rename_selected",
        label: || tr!("Rename"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::named(NamedKey::Function(2))],
    },
    ActionSpec {
        action: ShortcutAction::NewFolder,
        config_key: "new_folder",
        label: || tr!("New Folder"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl_shift('N')],
    },
    ActionSpec {
        action: ShortcutAction::NewFile,
        config_key: "new_file",
        label: || tr!("New File"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[ctrl_alt('N')],
    },
    ActionSpec {
        action: ShortcutAction::NewFolderFromSelection,
        config_key: "new_folder_from_selection",
        label: || tr!("New Folder with Selection"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('G')],
    },
    ActionSpec {
        action: ShortcutAction::Copy,
        config_key: "copy",
        label: || tr!("Copy"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('C'), KeyPress::named(NamedKey::Copy)],
    },
    ActionSpec {
        action: ShortcutAction::Cut,
        config_key: "cut",
        label: || tr!("Cut"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('X'), KeyPress::named(NamedKey::Cut)],
    },
    ActionSpec {
        action: ShortcutAction::Paste,
        config_key: "paste",
        label: || tr!("Paste"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('V'), KeyPress::named(NamedKey::Paste)],
    },
    ActionSpec {
        action: ShortcutAction::DuplicateSelected,
        config_key: "duplicate_selected",
        label: || tr!("Duplicate"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('D')],
    },
    ActionSpec {
        action: ShortcutAction::CopyPath,
        config_key: "copy_path",
        label: || tr!("Copy Path"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[ctrl_alt('C')],
    },
    ActionSpec {
        action: ShortcutAction::CreateSymlink,
        config_key: "create_symlink",
        label: || tr!("Create Symbolic Link"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('M')],
    },
    ActionSpec {
        action: ShortcutAction::Delete,
        config_key: "delete",
        label: || tr!("Move to Trash"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::named(NamedKey::Delete)],
    },
    ActionSpec {
        action: ShortcutAction::DeletePermanently,
        config_key: "delete_permanently",
        label: || tr!("Delete Permanently"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[shift_named(NamedKey::Delete)],
    },
    ActionSpec {
        action: ShortcutAction::Undo,
        config_key: "undo",
        label: || tr!("Undo File Operation"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('Z')],
    },
    ActionSpec {
        action: ShortcutAction::Redo,
        config_key: "redo",
        label: || tr!("Redo File Operation"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl_shift('Z'), KeyPress::ctrl('Y')],
    },
    ActionSpec {
        action: ShortcutAction::FileProperties,
        config_key: "file_properties",
        label: || tr!("Properties"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::alt_named(NamedKey::Enter), KeyPress::ctrl('I')],
    },
    ActionSpec {
        action: ShortcutAction::AddToFavorites,
        config_key: "add_to_favorites",
        label: || tr!("Add to Favorites"),
        group: G::Files,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('B')],
    },
    ActionSpec {
        action: ShortcutAction::SelectAll,
        config_key: "select_all",
        label: || tr!("Select All"),
        group: G::Selection,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl('A')],
    },
    ActionSpec {
        action: ShortcutAction::InvertSelection,
        config_key: "invert_selection",
        label: || tr!("Invert Selection"),
        group: G::Selection,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::ctrl_shift('A')],
    },
    ActionSpec {
        action: ShortcutAction::NewTab,
        config_key: "new_tab",
        label: || tr!("New Tab"),
        group: G::TabsAndWindows,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('T')],
    },
    ActionSpec {
        action: ShortcutAction::NextTab,
        config_key: "next_tab",
        label: || tr!("Next Tab"),
        group: G::TabsAndWindows,
        scope: Browser,
        context: App,
        keys: &[ctrl_named(NamedKey::Tab), ctrl_named(NamedKey::PageDown)],
    },
    ActionSpec {
        action: ShortcutAction::PreviousTab,
        config_key: "previous_tab",
        label: || tr!("Previous Tab"),
        group: G::TabsAndWindows,
        scope: Browser,
        context: App,
        keys: &[
            ctrl_shift_named(NamedKey::Tab),
            ctrl_named(NamedKey::PageUp),
        ],
    },
    ActionSpec {
        action: ShortcutAction::MoveTabLeft,
        config_key: "move_tab_left",
        label: || tr!("Move Tab Left"),
        group: G::TabsAndWindows,
        scope: Browser,
        context: App,
        keys: &[ctrl_shift_named(NamedKey::PageUp)],
    },
    ActionSpec {
        action: ShortcutAction::MoveTabRight,
        config_key: "move_tab_right",
        label: || tr!("Move Tab Right"),
        group: G::TabsAndWindows,
        scope: Browser,
        context: App,
        keys: &[ctrl_shift_named(NamedKey::PageDown)],
    },
    ActionSpec {
        action: ShortcutAction::ColumnsView,
        config_key: "columns_view",
        label: || tr!("Columns View"),
        group: G::View,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('1')],
    },
    ActionSpec {
        action: ShortcutAction::ListView,
        config_key: "list_view",
        label: || tr!("List View"),
        group: G::View,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('2')],
    },
    ActionSpec {
        action: ShortcutAction::IconsView,
        config_key: "icons_view",
        label: || tr!("Icons View"),
        group: G::View,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('3')],
    },
    ActionSpec {
        action: ShortcutAction::ZoomIn,
        config_key: "zoom_in",
        label: || tr!("Zoom In"),
        group: G::View,
        scope: Browser,
        context: App,
        // 主键盘上的 + 是 Shift+=（iced 送来的是不带修饰的 =），小键盘的 + 直接是 +。
        keys: &[
            KeyPress::ctrl('='),
            KeyPress::ctrl_shift('='),
            KeyPress::ctrl('+'),
        ],
    },
    ActionSpec {
        action: ShortcutAction::ZoomOut,
        config_key: "zoom_out",
        label: || tr!("Zoom Out"),
        group: G::View,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('-')],
    },
    ActionSpec {
        action: ShortcutAction::ZoomReset,
        config_key: "zoom_reset",
        label: || tr!("Default Size"),
        group: G::View,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl('0')],
    },
    ActionSpec {
        action: ShortcutAction::Preview,
        config_key: "preview",
        label: || tr!("Preview"),
        group: G::Panels,
        scope: Browser,
        context: Content,
        keys: &[KeyPress::named(NamedKey::Space)],
    },
    ActionSpec {
        action: ShortcutAction::ToggleRightPreviewPanel,
        config_key: "toggle_right_preview_panel",
        label: || tr!("Toggle Preview Panel"),
        group: G::Panels,
        scope: Browser,
        context: App,
        keys: &[KeyPress::alt('P')],
    },
    ActionSpec {
        action: ShortcutAction::ToggleTerminal,
        config_key: "toggle_terminal",
        label: || tr!("Toggle Terminal"),
        group: G::Panels,
        scope: ShortcutScope::Everywhere,
        context: App,
        keys: &[KeyPress::ctrl('`'), KeyPress::named(NamedKey::Function(4))],
    },
    ActionSpec {
        action: ShortcutAction::OpenExternalTerminal,
        config_key: "open_external_terminal",
        label: || tr!("Open Terminal Here"),
        group: G::Panels,
        scope: Browser,
        context: App,
        keys: &[shift_named(NamedKey::Function(4))],
    },
    ActionSpec {
        action: ShortcutAction::OpenSettings,
        config_key: "open_settings",
        label: || tr!("Settings"),
        group: G::Panels,
        scope: Browser,
        context: App,
        keys: &[KeyPress::ctrl(',')],
    },
    ActionSpec {
        action: ShortcutAction::TerminalNewTab,
        config_key: "terminal_new_tab",
        label: || tr!("New Terminal Tab"),
        group: G::Terminal,
        scope: ShortcutScope::Terminal,
        context: App,
        keys: &[KeyPress::ctrl_shift('T')],
    },
    ActionSpec {
        action: ShortcutAction::TerminalCloseTab,
        config_key: "terminal_close_tab",
        label: || tr!("Close Terminal Tab"),
        group: G::Terminal,
        scope: ShortcutScope::Terminal,
        context: App,
        keys: &[KeyPress::ctrl_shift('W')],
    },
];

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
