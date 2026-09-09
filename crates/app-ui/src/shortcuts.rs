use iced::keyboard::{self, key, Key};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileSelectionDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutAction {
    OpenSelected,
    RenameSelected,
    FocusPathInput,
    NavigateBack,
    NavigateForward,
    NavigateUp,
    MoveSelection(FileSelectionDirection),
    FileProperties,
    Refresh,
    Escape,
    Preview,
    ToggleRightPreviewPanel,
    ToggleTerminal,
    TerminalNewTab,
    TerminalCloseTab,
    SelectAll,
    Copy,
    DuplicateSelected,
    NewFolderFromSelection,
    Paste,
    Cut,
    Delete,
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileBrowserShortcutOwnership {
    CapturedDeleteEvent,
    FocusedTextInputProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutRoutingContext {
    Application,
    FileBrowserContent(FileBrowserShortcutOwnership),
}

impl ShortcutAction {
    pub(crate) fn routing_context(self, key: &Key) -> ShortcutRoutingContext {
        match self {
            Self::Refresh
            | Self::Escape
            | Self::FocusPathInput
            | Self::ToggleRightPreviewPanel
            | Self::ToggleTerminal
            | Self::TerminalNewTab
            | Self::TerminalCloseTab
            | Self::NavigateBack
            | Self::NavigateForward
            | Self::NavigateUp => ShortcutRoutingContext::Application,
            Self::Delete if matches!(key.as_ref(), Key::Named(key::Named::Delete)) => {
                ShortcutRoutingContext::FileBrowserContent(
                    FileBrowserShortcutOwnership::CapturedDeleteEvent,
                )
            }
            Self::Delete
            | Self::OpenSelected
            | Self::RenameSelected
            | Self::MoveSelection(_)
            | Self::FileProperties
            | Self::Preview
            | Self::SelectAll
            | Self::Copy
            | Self::DuplicateSelected
            | Self::NewFolderFromSelection
            | Self::Paste
            | Self::Cut
            | Self::Undo
            | Self::Redo => ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::FocusedTextInputProbe,
            ),
        }
    }

    pub(crate) fn is_preview_toggle(self) -> bool {
        matches!(self, Self::Preview)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutBindingId {
    OpenSelected,
    RenameSelected,
    FocusPathInput,
    NavigateBack,
    NavigateForward,
    NavigateUp,
    MoveSelectionUp,
    MoveSelectionDown,
    MoveSelectionLeft,
    MoveSelectionRight,
    FileProperties,
    Refresh,
    Escape,
    Preview,
    ToggleRightPreviewPanel,
    ToggleTerminal,
    TerminalNewTab,
    TerminalCloseTab,
    SelectAll,
    Copy,
    DuplicateSelected,
    NewFolderFromSelection,
    CopyNamed,
    Paste,
    PasteNamed,
    Cut,
    CutNamed,
    Delete,
    Undo,
    Redo,
}

const ALL_SHORTCUT_BINDING_IDS: [ShortcutBindingId; 30] = [
    ShortcutBindingId::OpenSelected,
    ShortcutBindingId::RenameSelected,
    ShortcutBindingId::FocusPathInput,
    ShortcutBindingId::NavigateBack,
    ShortcutBindingId::NavigateForward,
    ShortcutBindingId::NavigateUp,
    ShortcutBindingId::MoveSelectionUp,
    ShortcutBindingId::MoveSelectionDown,
    ShortcutBindingId::MoveSelectionLeft,
    ShortcutBindingId::MoveSelectionRight,
    ShortcutBindingId::FileProperties,
    ShortcutBindingId::Refresh,
    ShortcutBindingId::Escape,
    ShortcutBindingId::Preview,
    ShortcutBindingId::ToggleRightPreviewPanel,
    ShortcutBindingId::ToggleTerminal,
    ShortcutBindingId::TerminalNewTab,
    ShortcutBindingId::TerminalCloseTab,
    ShortcutBindingId::SelectAll,
    ShortcutBindingId::Copy,
    ShortcutBindingId::DuplicateSelected,
    ShortcutBindingId::NewFolderFromSelection,
    ShortcutBindingId::CopyNamed,
    ShortcutBindingId::Paste,
    ShortcutBindingId::PasteNamed,
    ShortcutBindingId::Cut,
    ShortcutBindingId::CutNamed,
    ShortcutBindingId::Delete,
    ShortcutBindingId::Undo,
    ShortcutBindingId::Redo,
];

impl ShortcutBindingId {
    pub(crate) fn all() -> &'static [Self] {
        &ALL_SHORTCUT_BINDING_IDS
    }

    pub(crate) fn action(self) -> ShortcutAction {
        match self {
            Self::OpenSelected => ShortcutAction::OpenSelected,
            Self::RenameSelected => ShortcutAction::RenameSelected,
            Self::FocusPathInput => ShortcutAction::FocusPathInput,
            Self::NavigateBack => ShortcutAction::NavigateBack,
            Self::NavigateForward => ShortcutAction::NavigateForward,
            Self::NavigateUp => ShortcutAction::NavigateUp,
            Self::MoveSelectionUp => ShortcutAction::MoveSelection(FileSelectionDirection::Up),
            Self::MoveSelectionDown => ShortcutAction::MoveSelection(FileSelectionDirection::Down),
            Self::MoveSelectionLeft => ShortcutAction::MoveSelection(FileSelectionDirection::Left),
            Self::MoveSelectionRight => {
                ShortcutAction::MoveSelection(FileSelectionDirection::Right)
            }
            Self::FileProperties => ShortcutAction::FileProperties,
            Self::Refresh => ShortcutAction::Refresh,
            Self::Escape => ShortcutAction::Escape,
            Self::Preview => ShortcutAction::Preview,
            Self::ToggleRightPreviewPanel => ShortcutAction::ToggleRightPreviewPanel,
            Self::ToggleTerminal => ShortcutAction::ToggleTerminal,
            Self::TerminalNewTab => ShortcutAction::TerminalNewTab,
            Self::TerminalCloseTab => ShortcutAction::TerminalCloseTab,
            Self::SelectAll => ShortcutAction::SelectAll,
            Self::Copy | Self::CopyNamed => ShortcutAction::Copy,
            Self::DuplicateSelected => ShortcutAction::DuplicateSelected,
            Self::NewFolderFromSelection => ShortcutAction::NewFolderFromSelection,
            Self::Paste | Self::PasteNamed => ShortcutAction::Paste,
            Self::Cut | Self::CutNamed => ShortcutAction::Cut,
            Self::Delete => ShortcutAction::Delete,
            Self::Undo => ShortcutAction::Undo,
            Self::Redo => ShortcutAction::Redo,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OpenSelected => "Open",
            Self::RenameSelected => "Rename",
            Self::FocusPathInput => "Focus Path",
            Self::NavigateBack => "Back",
            Self::NavigateForward => "Forward",
            Self::NavigateUp => "Up Folder",
            Self::MoveSelectionUp => "Select Up",
            Self::MoveSelectionDown => "Select Down",
            Self::MoveSelectionLeft => "Select Parent Column",
            Self::MoveSelectionRight => "Select Child Column",
            Self::FileProperties => "Properties",
            Self::Refresh => "Refresh",
            Self::Escape => "Dismiss",
            Self::Preview => "Preview",
            Self::ToggleRightPreviewPanel => "Toggle Preview Panel",
            Self::ToggleTerminal => "Toggle Terminal",
            Self::TerminalNewTab => "New Terminal Tab",
            Self::TerminalCloseTab => "Close Terminal Tab",
            Self::SelectAll => "Select All",
            Self::Copy => "Copy",
            Self::DuplicateSelected => "Duplicate",
            Self::NewFolderFromSelection => "New Folder with Selection",
            Self::CopyNamed => "Copy Named Key",
            Self::Paste => "Paste",
            Self::PasteNamed => "Paste Named Key",
            Self::Cut => "Cut",
            Self::CutNamed => "Cut Named Key",
            Self::Delete => "Delete",
            Self::Undo => "Undo File Operation",
            Self::Redo => "Redo File Operation",
        }
    }

    fn config_key(self) -> &'static str {
        match self {
            Self::OpenSelected => "open_selected",
            Self::RenameSelected => "rename_selected",
            Self::FocusPathInput => "focus_path_input",
            Self::NavigateBack => "navigate_back",
            Self::NavigateForward => "navigate_forward",
            Self::NavigateUp => "navigate_up",
            Self::MoveSelectionUp => "move_selection_up",
            Self::MoveSelectionDown => "move_selection_down",
            Self::MoveSelectionLeft => "move_selection_left",
            Self::MoveSelectionRight => "move_selection_right",
            Self::FileProperties => "file_properties",
            Self::Refresh => "refresh",
            Self::Escape => "escape",
            Self::Preview => "preview",
            Self::ToggleRightPreviewPanel => "toggle_right_preview_panel",
            Self::ToggleTerminal => "toggle_terminal",
            Self::TerminalNewTab => "terminal_new_tab",
            Self::TerminalCloseTab => "terminal_close_tab",
            Self::SelectAll => "select_all",
            Self::Copy => "copy",
            Self::DuplicateSelected => "duplicate_selected",
            Self::NewFolderFromSelection => "new_folder_from_selection",
            Self::CopyNamed => "copy_named",
            Self::Paste => "paste",
            Self::PasteNamed => "paste_named",
            Self::Cut => "cut",
            Self::CutNamed => "cut_named",
            Self::Delete => "delete",
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }

    fn from_config_key(key: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|binding_id| binding_id.config_key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutConfig {
    bindings: Vec<ShortcutBinding>,
}

impl ShortcutConfig {
    pub(crate) fn defaults() -> Self {
        let bindings = ShortcutBindingId::all()
            .iter()
            .copied()
            .map(|id| ShortcutBinding {
                id,
                binding: default_binding(id),
            })
            .collect();
        Self { bindings }
    }

    pub(crate) fn bindings(&self) -> &[ShortcutBinding] {
        &self.bindings
    }

    #[cfg(test)]
    pub(crate) fn binding(&self, id: ShortcutBindingId) -> &KeyBinding {
        &self.bindings[self.binding_index(id)].binding
    }

    pub(crate) fn set_binding(&mut self, id: ShortcutBindingId, binding: KeyBinding) {
        let index = self.binding_index(id);
        self.bindings[index].binding = binding;
    }

    pub(crate) fn reset_binding(&mut self, id: ShortcutBindingId) {
        self.set_binding(id, default_binding(id));
    }

    pub(crate) fn default_binding(id: ShortcutBindingId) -> KeyBinding {
        default_binding(id)
    }

    pub(crate) fn matching_action(
        &self,
        key: &Key,
        modifiers: keyboard::Modifiers,
    ) -> Option<ShortcutAction> {
        self.bindings
            .iter()
            .find(|binding| binding.binding.matches_iced_key(key, modifiers))
            .map(|binding| binding.id.action())
    }

    pub(crate) fn conflicting_binding(
        &self,
        id: ShortcutBindingId,
        candidate: &KeyBinding,
    ) -> Option<ShortcutBindingId> {
        self.bindings
            .iter()
            .find(|binding| binding.id != id && binding.binding.conflicts_with(candidate))
            .map(|binding| binding.id)
    }

    pub(crate) fn apply_toml_table(&mut self, table: &toml::Table) {
        for (key, value) in table {
            let (Some(id), Some(value)) = (
                ShortcutBindingId::from_config_key(key),
                value.as_str().filter(|value| !value.is_empty()),
            ) else {
                continue;
            };
            if let Some(binding) = KeyBinding::from_config_value(value) {
                self.set_binding(id, binding);
            }
        }
    }

    pub(crate) fn toml_table(&self) -> toml::Table {
        self.bindings
            .iter()
            .map(|binding| {
                (
                    binding.id.config_key().to_owned(),
                    toml::Value::String(binding.binding.config_value()),
                )
            })
            .collect()
    }

    fn binding_index(&self, id: ShortcutBindingId) -> usize {
        self.bindings
            .iter()
            .position(|binding| binding.id == id)
            .expect("shortcut config must contain every known binding id")
    }
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutBinding {
    pub(crate) id: ShortcutBindingId,
    pub(crate) binding: KeyBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    modifiers: ShortcutModifiers,
    key: ShortcutKey,
}

impl KeyBinding {
    pub(crate) fn from_iced_key(key: &Key, modifiers: keyboard::Modifiers) -> Option<Self> {
        let key = ShortcutKey::from_iced_key(key)?;
        Some(Self {
            modifiers: ShortcutModifiers::from_iced(modifiers),
            key,
        })
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        let mut modifiers = ShortcutModifiers::default();
        let mut parts = value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty());
        let key_part = parts.next_back()?;
        for part in parts {
            if !modifiers.apply_config_part(part) {
                return None;
            }
        }
        Some(Self {
            modifiers,
            key: ShortcutKey::from_config_part(key_part)?,
        })
    }

    pub(crate) fn config_value(&self) -> String {
        let mut parts = Vec::new();
        self.modifiers.push_config_parts(&mut parts);
        parts.push(self.key.config_value());
        parts.join("+")
    }

    pub(crate) fn matches_iced_key(&self, key: &Key, modifiers: keyboard::Modifiers) -> bool {
        self.key.matches_iced_key(key) && self.modifiers.matches_iced(modifiers)
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        self.key == other.key && self.modifiers.conflicts_with(&other.modifiers)
    }
}

fn default_binding(id: ShortcutBindingId) -> KeyBinding {
    match id {
        ShortcutBindingId::OpenSelected => KeyBinding::named(ShortcutNamedKey::Enter),
        ShortcutBindingId::RenameSelected => KeyBinding::named(ShortcutNamedKey::Function(2)),
        ShortcutBindingId::FocusPathInput => KeyBinding::control_character('L'),
        ShortcutBindingId::NavigateBack => KeyBinding::alt_named(ShortcutNamedKey::ArrowLeft),
        ShortcutBindingId::NavigateForward => KeyBinding::alt_named(ShortcutNamedKey::ArrowRight),
        ShortcutBindingId::NavigateUp => KeyBinding::alt_named(ShortcutNamedKey::ArrowUp),
        ShortcutBindingId::MoveSelectionUp => KeyBinding::named(ShortcutNamedKey::ArrowUp),
        ShortcutBindingId::MoveSelectionDown => KeyBinding::named(ShortcutNamedKey::ArrowDown),
        ShortcutBindingId::MoveSelectionLeft => KeyBinding::named(ShortcutNamedKey::ArrowLeft),
        ShortcutBindingId::MoveSelectionRight => KeyBinding::named(ShortcutNamedKey::ArrowRight),
        ShortcutBindingId::FileProperties => KeyBinding::alt_named(ShortcutNamedKey::Enter),
        ShortcutBindingId::Refresh => KeyBinding::named(ShortcutNamedKey::Function(5)),
        ShortcutBindingId::Escape => KeyBinding::named(ShortcutNamedKey::Escape),
        ShortcutBindingId::Preview => KeyBinding::named(ShortcutNamedKey::Space),
        ShortcutBindingId::ToggleRightPreviewPanel => KeyBinding::alt_character('P'),
        ShortcutBindingId::ToggleTerminal => KeyBinding::control_character('`'),
        ShortcutBindingId::TerminalNewTab => KeyBinding::control_shift_character('T'),
        ShortcutBindingId::TerminalCloseTab => KeyBinding::control_shift_character('W'),
        ShortcutBindingId::SelectAll => KeyBinding::primary_character('A'),
        ShortcutBindingId::Copy => KeyBinding::primary_character('C'),
        ShortcutBindingId::DuplicateSelected => KeyBinding::primary_character('D'),
        ShortcutBindingId::NewFolderFromSelection => KeyBinding::control_shift_character('N'),
        ShortcutBindingId::CopyNamed => KeyBinding::named(ShortcutNamedKey::Copy),
        ShortcutBindingId::Paste => KeyBinding::primary_character('V'),
        ShortcutBindingId::PasteNamed => KeyBinding::named(ShortcutNamedKey::Paste),
        ShortcutBindingId::Cut => KeyBinding::primary_character('X'),
        ShortcutBindingId::CutNamed => KeyBinding::named(ShortcutNamedKey::Cut),
        ShortcutBindingId::Delete => KeyBinding::named(ShortcutNamedKey::Delete),
        ShortcutBindingId::Undo => KeyBinding::primary_character('Z'),
        ShortcutBindingId::Redo => KeyBinding::primary_character('Y'),
    }
}

impl KeyBinding {
    fn named(key: ShortcutNamedKey) -> Self {
        Self {
            modifiers: ShortcutModifiers::default(),
            key: ShortcutKey::Named(key),
        }
    }

    fn alt_named(key: ShortcutNamedKey) -> Self {
        Self {
            modifiers: ShortcutModifiers {
                alt: true,
                ..ShortcutModifiers::default()
            },
            key: ShortcutKey::Named(key),
        }
    }

    fn alt_character(character: char) -> Self {
        Self {
            modifiers: ShortcutModifiers {
                alt: true,
                ..ShortcutModifiers::default()
            },
            key: ShortcutKey::Character(character),
        }
    }

    fn control_character(character: char) -> Self {
        Self {
            modifiers: ShortcutModifiers {
                control: true,
                ..ShortcutModifiers::default()
            },
            key: ShortcutKey::Character(character),
        }
    }

    fn control_shift_character(character: char) -> Self {
        Self {
            modifiers: ShortcutModifiers {
                control: true,
                shift: true,
                ..ShortcutModifiers::default()
            },
            key: ShortcutKey::Character(character),
        }
    }

    fn primary_character(character: char) -> Self {
        Self {
            modifiers: ShortcutModifiers {
                primary: true,
                ..ShortcutModifiers::default()
            },
            key: ShortcutKey::Character(character),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ShortcutModifiers {
    primary: bool,
    control: bool,
    alt: bool,
    shift: bool,
}

impl ShortcutModifiers {
    fn from_iced(modifiers: keyboard::Modifiers) -> Self {
        Self {
            primary: false,
            control: modifiers.control(),
            alt: modifiers.alt(),
            shift: modifiers.shift(),
        }
    }

    fn apply_config_part(&mut self, part: &str) -> bool {
        if part.eq_ignore_ascii_case("primary") {
            self.primary = true;
        } else if part.eq_ignore_ascii_case("ctrl") || part.eq_ignore_ascii_case("control") {
            self.control = true;
        } else if part.eq_ignore_ascii_case("alt") {
            self.alt = true;
        } else if part.eq_ignore_ascii_case("shift") {
            self.shift = true;
        } else {
            return false;
        }
        !(self.primary && self.control)
    }

    fn push_config_parts(&self, parts: &mut Vec<String>) {
        if self.primary {
            parts.push("Primary".to_owned());
        }
        if self.control {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
    }

    fn matches_iced(&self, modifiers: keyboard::Modifiers) -> bool {
        // iced 在非 macOS 上 command() == control(),非 primary 分支若用
        // command() 排除平台主键,会把字面 Ctrl 绑定(如 Ctrl+L、Ctrl+`)
        // 一并排除导致永远失配;这里必须用 logo() 只排除真正的平台主键。
        let primary_matches = if self.primary {
            modifiers.control() || modifiers.command()
        } else {
            modifiers.control() == self.control && !modifiers.logo()
        };
        primary_matches && modifiers.alt() == self.alt && modifiers.shift() == self.shift
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        let (self_accepts_control, self_accepts_command) = self.accepts_control_or_command();
        let (other_accepts_control, other_accepts_command) = other.accepts_control_or_command();
        self.alt == other.alt
            && self.shift == other.shift
            && ((self_accepts_control && other_accepts_control)
                || (self_accepts_command && other_accepts_command))
    }

    fn accepts_control_or_command(&self) -> (bool, bool) {
        if self.primary {
            (true, true)
        } else {
            (self.control, false)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortcutKey {
    Character(char),
    Named(ShortcutNamedKey),
}

impl ShortcutKey {
    fn from_iced_key(key: &Key) -> Option<Self> {
        match key.as_ref() {
            Key::Character(value) => normalized_character(value).map(Self::Character),
            Key::Named(named) => ShortcutNamedKey::from_iced_named(named).map(Self::Named),
            _ => None,
        }
    }

    fn from_config_part(part: &str) -> Option<Self> {
        normalized_character(part)
            .map(Self::Character)
            .or_else(|| ShortcutNamedKey::from_config_part(part).map(Self::Named))
    }

    fn config_value(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Named(named) => named.config_value(),
        }
    }

    fn matches_iced_key(self, key: &Key) -> bool {
        Self::from_iced_key(key) == Some(self)
    }
}

fn normalized_character(value: &str) -> Option<char> {
    let mut characters = value.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    // '+' 是配置里的修饰键分隔符,不能当按键;其余 ASCII 标点允许,
    // 否则 Ctrl+` 这类绑定在注册和匹配两端都会被拒。
    let is_bindable = character.is_ascii_alphanumeric()
        || (character.is_ascii_punctuation() && character != '+');
    if !is_bindable {
        return None;
    }
    Some(character.to_ascii_uppercase())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortcutNamedKey {
    Enter,
    Escape,
    Space,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Copy,
    Paste,
    Cut,
    Function(u8),
}

impl ShortcutNamedKey {
    fn from_iced_named(named: key::Named) -> Option<Self> {
        match named {
            key::Named::Enter => Some(Self::Enter),
            key::Named::Escape => Some(Self::Escape),
            key::Named::Space => Some(Self::Space),
            key::Named::Delete => Some(Self::Delete),
            key::Named::ArrowLeft => Some(Self::ArrowLeft),
            key::Named::ArrowRight => Some(Self::ArrowRight),
            key::Named::ArrowUp => Some(Self::ArrowUp),
            key::Named::ArrowDown => Some(Self::ArrowDown),
            key::Named::Copy => Some(Self::Copy),
            key::Named::Paste => Some(Self::Paste),
            key::Named::Cut => Some(Self::Cut),
            key::Named::F1 => Some(Self::Function(1)),
            key::Named::F2 => Some(Self::Function(2)),
            key::Named::F3 => Some(Self::Function(3)),
            key::Named::F4 => Some(Self::Function(4)),
            key::Named::F5 => Some(Self::Function(5)),
            key::Named::F6 => Some(Self::Function(6)),
            key::Named::F7 => Some(Self::Function(7)),
            key::Named::F8 => Some(Self::Function(8)),
            key::Named::F9 => Some(Self::Function(9)),
            key::Named::F10 => Some(Self::Function(10)),
            key::Named::F11 => Some(Self::Function(11)),
            key::Named::F12 => Some(Self::Function(12)),
            _ => None,
        }
    }

    fn from_config_part(part: &str) -> Option<Self> {
        if part.eq_ignore_ascii_case("enter") || part.eq_ignore_ascii_case("return") {
            return Some(Self::Enter);
        }
        if part.eq_ignore_ascii_case("escape") || part.eq_ignore_ascii_case("esc") {
            return Some(Self::Escape);
        }
        if part.eq_ignore_ascii_case("space") {
            return Some(Self::Space);
        }
        if part.eq_ignore_ascii_case("delete") || part.eq_ignore_ascii_case("del") {
            return Some(Self::Delete);
        }
        if part.eq_ignore_ascii_case("left") || part.eq_ignore_ascii_case("arrowleft") {
            return Some(Self::ArrowLeft);
        }
        if part.eq_ignore_ascii_case("right") || part.eq_ignore_ascii_case("arrowright") {
            return Some(Self::ArrowRight);
        }
        if part.eq_ignore_ascii_case("up") || part.eq_ignore_ascii_case("arrowup") {
            return Some(Self::ArrowUp);
        }
        if part.eq_ignore_ascii_case("down") || part.eq_ignore_ascii_case("arrowdown") {
            return Some(Self::ArrowDown);
        }
        if part.eq_ignore_ascii_case("copy") {
            return Some(Self::Copy);
        }
        if part.eq_ignore_ascii_case("paste") {
            return Some(Self::Paste);
        }
        if part.eq_ignore_ascii_case("cut") {
            return Some(Self::Cut);
        }
        let function_key = part.strip_prefix('F').or_else(|| part.strip_prefix('f'))?;
        let number = function_key.parse::<u8>().ok()?;
        (1..=12).contains(&number).then_some(Self::Function(number))
    }

    fn config_value(self) -> String {
        match self {
            Self::Enter => "Enter".to_owned(),
            Self::Escape => "Escape".to_owned(),
            Self::Space => "Space".to_owned(),
            Self::Delete => "Delete".to_owned(),
            Self::ArrowLeft => "Left".to_owned(),
            Self::ArrowRight => "Right".to_owned(),
            Self::ArrowUp => "Up".to_owned(),
            Self::ArrowDown => "Down".to_owned(),
            Self::Copy => "Copy".to_owned(),
            Self::Paste => "Paste".to_owned(),
            Self::Cut => "Cut".to_owned(),
            Self::Function(number) => format!("F{number}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutCaptureState {
    pub(crate) binding_id: ShortcutBindingId,
    pub(crate) rejected_binding: Option<KeyBinding>,
    pub(crate) conflict_binding_id: Option<ShortcutBindingId>,
    pub(crate) unsupported_key: bool,
}

impl ShortcutCaptureState {
    pub(crate) fn new(binding_id: ShortcutBindingId) -> Self {
        Self {
            binding_id,
            rejected_binding: None,
            conflict_binding_id: None,
            unsupported_key: false,
        }
    }

    pub(crate) fn conflict(
        binding_id: ShortcutBindingId,
        rejected_binding: KeyBinding,
        conflict_binding_id: ShortcutBindingId,
    ) -> Self {
        Self {
            binding_id,
            rejected_binding: Some(rejected_binding),
            conflict_binding_id: Some(conflict_binding_id),
            unsupported_key: false,
        }
    }

    pub(crate) fn unsupported(binding_id: ShortcutBindingId) -> Self {
        Self {
            binding_id,
            rejected_binding: None,
            conflict_binding_id: None,
            unsupported_key: true,
        }
    }
}

#[cfg(test)]
#[path = "shortcuts_routing_context_tests.rs"]
mod routing_context_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[test]
    fn ctrl_backtick_matches_default_toggle_terminal() {
        // 回归:字符归一化曾拒绝标点,导致 Ctrl+` 注册后永远匹配不上。
        let action = ShortcutConfig::defaults().matching_action(
            &Key::Character("`".into()),
            keyboard::Modifiers::CTRL,
        );
        assert_eq!(action, Some(ShortcutAction::ToggleTerminal));
    }

    #[test]
    fn ctrl_shift_t_w_are_default_terminal_tab_shortcuts() {
        // 大小写两种字形都要命中:物理按键送来的字符跟随 shift 状态。
        let config = ShortcutConfig::defaults();
        for character in ["T", "t"] {
            assert_eq!(
                config.matching_action(
                    &Key::Character(character.into()),
                    keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
                ),
                Some(ShortcutAction::TerminalNewTab)
            );
        }
        assert_eq!(
            config.matching_action(
                &Key::Character("W".into()),
                keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
            ),
            Some(ShortcutAction::TerminalCloseTab)
        );
    }

    #[test]
    fn parses_and_formats_shortcut_binding() {
        let binding = KeyBinding::from_config_value("Ctrl+Alt+L").expect("binding");

        assert_eq!(binding.config_value(), "Ctrl+Alt+L");
    }

    #[test]
    fn primary_binding_matches_control_or_command() {
        let binding = KeyBinding::from_config_value("Primary+F").expect("binding");
        let key = Key::Character("f".into());

        assert!(binding.matches_iced_key(&key, keyboard::Modifiers::CTRL));
        #[cfg(target_os = "macos")]
        assert!(binding.matches_iced_key(&key, keyboard::Modifiers::LOGO));
        #[cfg(not(target_os = "macos"))]
        assert!(!binding.matches_iced_key(&key, keyboard::Modifiers::LOGO));
        assert!(!binding.matches_iced_key(&key, keyboard::Modifiers::ALT));
    }

    #[test]
    fn primary_binding_conflicts_with_control_binding() {
        let primary = KeyBinding::from_config_value("Primary+F").expect("binding");
        let control = KeyBinding::from_config_value("Ctrl+F").expect("binding");

        assert!(primary.conflicts_with(&control));
    }

    #[test]
    fn toml_table_round_trips_known_shortcuts() {
        let mut shortcuts = ShortcutConfig::defaults();
        let mut table = toml::Table::new();
        table.insert(
            "focus_path_input".to_owned(),
            toml::Value::String("Ctrl+Alt+L".to_owned()),
        );
        shortcuts.apply_toml_table(&table);

        assert_eq!(
            shortcuts
                .binding(ShortcutBindingId::FocusPathInput)
                .config_value(),
            "Ctrl+Alt+L"
        );
        assert_eq!(
            shortcuts
                .toml_table()
                .get("focus_path_input")
                .and_then(toml::Value::as_str),
            Some("Ctrl+Alt+L")
        );
    }

    #[test]
    fn properties_default_shortcut_is_alt_enter() {
        let shortcuts = ShortcutConfig::defaults();
        let key = Key::Named(key::Named::Enter);

        assert_eq!(
            shortcuts.matching_action(&key, keyboard::Modifiers::ALT),
            Some(ShortcutAction::FileProperties)
        );
    }

    #[test]
    fn alt_p_is_default_toggle_right_preview_panel() {
        // 大小写两种字形都要命中:物理按键送来的字符跟随 shift 状态。
        let shortcuts = ShortcutConfig::defaults();
        for character in ["P", "p"] {
            assert_eq!(
                shortcuts
                    .matching_action(&Key::Character(character.into()), keyboard::Modifiers::ALT),
                Some(ShortcutAction::ToggleRightPreviewPanel)
            );
        }
        // 无 Alt 修饰的 P 不得误触面板开关。
        assert_eq!(
            shortcuts.matching_action(&Key::Character("P".into()), keyboard::Modifiers::default()),
            None
        );
    }
}
