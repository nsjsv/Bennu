use super::key_press::{KeyPress, NamedKey};

/// 默认键表：常量函数的调用结果不会自动提升成 `'static`，放进常量项。
macro_rules! keys {
    ($($key:expr),* $(,)?) => {{
        const KEYS: &[KeyPress] = &[$($key),*];
        KEYS
    }};
}

/// 能在设置里改键的命令。移动键、Esc、Alt+数字是固定键（`fixed_keys`），字母层另算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ShortcutAction {
    NavigateBack,
    NavigateForward,
    NavigateUp,
    FocusPathInput,
    Refresh,
    OpenSelected,
    RenameSelected,
    Copy,
    Cut,
    Paste,
    DuplicateSelected,
    NewFolderFromSelection,
    Delete,
    Undo,
    Redo,
    FileProperties,
    SelectAll,
    Preview,
    ToggleRightPreviewPanel,
    ToggleTerminal,
    TerminalNewTab,
    TerminalCloseTab,
}

/// 设置页和快捷键一览里的分组，按这个顺序排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ShortcutGroup {
    Navigation,
    Files,
    Selection,
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

/// 输入框被别的控件捕获时还执行不执行：`Application` 照样执行（导航、面板），
/// `FileBrowserContent` 让给控件，并且要先确认没有文本框焦点。编辑键另有规则，见 `routing`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredContext {
    Application,
    FileBrowserContent,
}

const ALL_ACTIONS: [ShortcutAction; 22] = [
    ShortcutAction::NavigateBack,
    ShortcutAction::NavigateForward,
    ShortcutAction::NavigateUp,
    ShortcutAction::FocusPathInput,
    ShortcutAction::Refresh,
    ShortcutAction::OpenSelected,
    ShortcutAction::RenameSelected,
    ShortcutAction::Copy,
    ShortcutAction::Cut,
    ShortcutAction::Paste,
    ShortcutAction::DuplicateSelected,
    ShortcutAction::NewFolderFromSelection,
    ShortcutAction::Delete,
    ShortcutAction::Undo,
    ShortcutAction::Redo,
    ShortcutAction::FileProperties,
    ShortcutAction::SelectAll,
    ShortcutAction::Preview,
    ShortcutAction::ToggleRightPreviewPanel,
    ShortcutAction::ToggleTerminal,
    ShortcutAction::TerminalNewTab,
    ShortcutAction::TerminalCloseTab,
];

impl ShortcutAction {
    pub(crate) fn all() -> &'static [Self] {
        &ALL_ACTIONS
    }

    pub(crate) fn default_keys(self) -> &'static [KeyPress] {
        match self {
            Self::NavigateBack => keys![KeyPress::alt_named(NamedKey::ArrowLeft)],
            Self::NavigateForward => keys![KeyPress::alt_named(NamedKey::ArrowRight)],
            Self::NavigateUp => keys![
                KeyPress::alt_named(NamedKey::ArrowUp),
                KeyPress::named(NamedKey::Backspace),
            ],
            Self::FocusPathInput => keys![KeyPress::ctrl('L')],
            Self::Refresh => keys![KeyPress::named(NamedKey::Function(5)), KeyPress::ctrl('R')],
            Self::OpenSelected => keys![KeyPress::named(NamedKey::Enter)],
            Self::RenameSelected => keys![KeyPress::named(NamedKey::Function(2))],
            Self::Copy => keys![KeyPress::ctrl('C'), KeyPress::named(NamedKey::Copy)],
            Self::Cut => keys![KeyPress::ctrl('X'), KeyPress::named(NamedKey::Cut)],
            Self::Paste => keys![KeyPress::ctrl('V'), KeyPress::named(NamedKey::Paste)],
            Self::DuplicateSelected => keys![KeyPress::ctrl('D')],
            Self::NewFolderFromSelection => keys![KeyPress::ctrl('G')],
            Self::Delete => keys![KeyPress::named(NamedKey::Delete)],
            Self::Undo => keys![KeyPress::ctrl('Z')],
            Self::Redo => keys![KeyPress::ctrl_shift('Z'), KeyPress::ctrl('Y')],
            Self::FileProperties => {
                keys![KeyPress::alt_named(NamedKey::Enter), KeyPress::ctrl('I')]
            }
            Self::SelectAll => keys![KeyPress::ctrl('A')],
            Self::Preview => keys![KeyPress::named(NamedKey::Space)],
            Self::ToggleRightPreviewPanel => keys![KeyPress::alt('P')],
            Self::ToggleTerminal => {
                keys![KeyPress::ctrl('`'), KeyPress::named(NamedKey::Function(4))]
            }
            Self::TerminalNewTab => keys![KeyPress::ctrl_shift('T')],
            Self::TerminalCloseTab => keys![KeyPress::ctrl_shift('W')],
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::NavigateBack => tr!("Back"),
            Self::NavigateForward => tr!("Forward"),
            Self::NavigateUp => tr!("Up Folder"),
            Self::FocusPathInput => tr!("Focus Path"),
            Self::Refresh => tr!("Refresh"),
            Self::OpenSelected => tr!("Open"),
            Self::RenameSelected => tr!("Rename"),
            Self::Copy => tr!("Copy"),
            Self::Cut => tr!("Cut"),
            Self::Paste => tr!("Paste"),
            Self::DuplicateSelected => tr!("Duplicate"),
            Self::NewFolderFromSelection => tr!("New Folder with Selection"),
            Self::Delete => tr!("Move to Trash"),
            Self::Undo => tr!("Undo File Operation"),
            Self::Redo => tr!("Redo File Operation"),
            Self::FileProperties => tr!("Properties"),
            Self::SelectAll => tr!("Select All"),
            Self::Preview => tr!("Preview"),
            Self::ToggleRightPreviewPanel => tr!("Toggle Preview Panel"),
            Self::ToggleTerminal => tr!("Toggle Terminal"),
            Self::TerminalNewTab => tr!("New Terminal Tab"),
            Self::TerminalCloseTab => tr!("Close Terminal Tab"),
        }
    }

    pub(crate) fn group(self) -> ShortcutGroup {
        match self {
            Self::NavigateBack
            | Self::NavigateForward
            | Self::NavigateUp
            | Self::FocusPathInput
            | Self::Refresh => ShortcutGroup::Navigation,
            Self::OpenSelected
            | Self::RenameSelected
            | Self::Copy
            | Self::Cut
            | Self::Paste
            | Self::DuplicateSelected
            | Self::NewFolderFromSelection
            | Self::Delete
            | Self::Undo
            | Self::Redo
            | Self::FileProperties => ShortcutGroup::Files,
            Self::SelectAll => ShortcutGroup::Selection,
            Self::Preview | Self::ToggleRightPreviewPanel | Self::ToggleTerminal => {
                ShortcutGroup::Panels
            }
            Self::TerminalNewTab | Self::TerminalCloseTab => ShortcutGroup::Terminal,
        }
    }

    pub(crate) fn scope(self) -> ShortcutScope {
        match self {
            Self::ToggleTerminal => ShortcutScope::Everywhere,
            Self::TerminalNewTab | Self::TerminalCloseTab => ShortcutScope::Terminal,
            _ => ShortcutScope::FileBrowser,
        }
    }

    pub(crate) fn declared_context(self) -> DeclaredContext {
        match self {
            Self::NavigateBack
            | Self::NavigateForward
            | Self::NavigateUp
            | Self::FocusPathInput
            | Self::Refresh
            | Self::ToggleRightPreviewPanel
            | Self::ToggleTerminal
            | Self::TerminalNewTab
            | Self::TerminalCloseTab => DeclaredContext::Application,
            _ => DeclaredContext::FileBrowserContent,
        }
    }

    /// 存储里的名字；改名要同时在 `overrides::LEGACY_DEFAULTS` 里留旧名。
    pub(crate) fn config_key(self) -> &'static str {
        match self {
            Self::NavigateBack => "navigate_back",
            Self::NavigateForward => "navigate_forward",
            Self::NavigateUp => "navigate_up",
            Self::FocusPathInput => "focus_path_input",
            Self::Refresh => "refresh",
            Self::OpenSelected => "open_selected",
            Self::RenameSelected => "rename_selected",
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::Paste => "paste",
            Self::DuplicateSelected => "duplicate_selected",
            Self::NewFolderFromSelection => "new_folder_from_selection",
            Self::Delete => "delete",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::FileProperties => "file_properties",
            Self::SelectAll => "select_all",
            Self::Preview => "preview",
            Self::ToggleRightPreviewPanel => "toggle_right_preview_panel",
            Self::ToggleTerminal => "toggle_terminal",
            Self::TerminalNewTab => "terminal_new_tab",
            Self::TerminalCloseTab => "terminal_close_tab",
        }
    }

    pub(crate) fn from_config_key(key: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|action| action.config_key() == key)
    }
}

impl ShortcutGroup {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Navigation => tr!("Navigation"),
            Self::Files => tr!("Files"),
            Self::Selection => tr!("Selection"),
            Self::Panels => tr!("Panels"),
            Self::Terminal => tr!("Terminal"),
        }
    }
}
