//! 右键菜单项的词表:每类菜单的项枚举、全集顺序、展示文案与存储字符串转换。

use crate::icons::IconSymbol;
use crate::model::SearchEntryTypePreset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAreaMenuItem {
    Open,
    SmartExtractHere,
    ExtractToArchiveFolder,
    OpenWith,
    Copy,
    Duplicate,
    Move,
    Tools,
    CreateArchive,
    ConvertFormat,
    FileChecksum,
    Paste,
    Rename,
    BatchRename,
    NewEntry,
    NewFolderFromSelection,
    OpenTerminalHere,
    CopyPath,
    CreateSymlink,
    Delete,
    Properties,
}

pub(crate) const FILE_ENTRY_MENU_ITEMS: [FileAreaMenuItem; 21] = [
    FileAreaMenuItem::Open,
    FileAreaMenuItem::SmartExtractHere,
    FileAreaMenuItem::ExtractToArchiveFolder,
    FileAreaMenuItem::OpenWith,
    FileAreaMenuItem::Copy,
    FileAreaMenuItem::Duplicate,
    FileAreaMenuItem::Move,
    FileAreaMenuItem::Tools,
    FileAreaMenuItem::CreateArchive,
    FileAreaMenuItem::ConvertFormat,
    FileAreaMenuItem::FileChecksum,
    FileAreaMenuItem::Paste,
    FileAreaMenuItem::Rename,
    FileAreaMenuItem::BatchRename,
    FileAreaMenuItem::NewEntry,
    FileAreaMenuItem::NewFolderFromSelection,
    FileAreaMenuItem::OpenTerminalHere,
    FileAreaMenuItem::CopyPath,
    FileAreaMenuItem::CreateSymlink,
    FileAreaMenuItem::Delete,
    FileAreaMenuItem::Properties,
];

/// 空白处右键菜单的项全集(条目菜单的子集)。
pub(crate) const FILE_BLANK_MENU_ITEMS: [FileAreaMenuItem; 3] = [
    FileAreaMenuItem::Paste,
    FileAreaMenuItem::NewEntry,
    FileAreaMenuItem::OpenTerminalHere,
];

/// 文件条目菜单的悬停子菜单分组。锚点行单点执行自身动作(纯触发器除外),
/// 成员渲染进子菜单且保留独立可见性;组是结构常量,不参与存储与排序配置。
pub(crate) struct FileEntryMenuGroupSpec {
    pub(crate) anchor: FileAreaMenuItem,
    pub(crate) members: &'static [FileAreaMenuItem],
}

pub(crate) const FILE_ENTRY_MENU_GROUPS: [FileEntryMenuGroupSpec; 4] = [
    FileEntryMenuGroupSpec {
        anchor: FileAreaMenuItem::Copy,
        members: &[FileAreaMenuItem::Duplicate, FileAreaMenuItem::CopyPath],
    },
    FileEntryMenuGroupSpec {
        anchor: FileAreaMenuItem::Tools,
        members: &[
            FileAreaMenuItem::CreateArchive,
            FileAreaMenuItem::ConvertFormat,
            FileAreaMenuItem::FileChecksum,
            FileAreaMenuItem::CreateSymlink,
        ],
    },
    FileEntryMenuGroupSpec {
        anchor: FileAreaMenuItem::Rename,
        members: &[FileAreaMenuItem::BatchRename],
    },
    FileEntryMenuGroupSpec {
        anchor: FileAreaMenuItem::NewEntry,
        members: &[FileAreaMenuItem::NewFolderFromSelection],
    },
];

/// 成员所属组的锚点;非成员返回 None。
pub(crate) fn file_entry_group_anchor_of(item: FileAreaMenuItem) -> Option<FileAreaMenuItem> {
    FILE_ENTRY_MENU_GROUPS
        .iter()
        .find(|group| group.members.contains(&item))
        .map(|group| group.anchor)
}

/// 锚点对应的组;非锚点返回 None。
pub(crate) fn file_entry_group_of(
    anchor: FileAreaMenuItem,
) -> Option<&'static FileEntryMenuGroupSpec> {
    FILE_ENTRY_MENU_GROUPS
        .iter()
        .find(|group| group.anchor == anchor)
}

impl FileAreaMenuItem {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::SmartExtractHere => "Smart Extract Here",
            Self::ExtractToArchiveFolder => "Extract to Archive Folder",
            Self::OpenWith => "Open with",
            Self::Copy => "Copy",
            Self::Duplicate => "Duplicate",
            Self::Move => "Move",
            Self::Tools => "Tools",
            Self::CreateArchive => "Create Archive...",
            Self::ConvertFormat => "Convert Format...",
            Self::FileChecksum => "File Checksum...",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::BatchRename => "Batch Rename...",
            Self::NewEntry => "New...",
            Self::NewFolderFromSelection => "New Folder with Selection",
            Self::OpenTerminalHere => "Open Terminal Here",
            Self::CopyPath => "Copy Path",
            Self::CreateSymlink => "Create Symbolic Link",
            Self::Delete => "Delete",
            Self::Properties => "Properties",
        }
    }

    pub(super) fn config_value(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::SmartExtractHere => "smart_extract_here",
            Self::ExtractToArchiveFolder => "extract_to_archive_folder",
            Self::OpenWith => "open_with",
            Self::Copy => "copy",
            Self::Duplicate => "duplicate",
            Self::Move => "move",
            Self::Tools => "tools",
            Self::CreateArchive => "create_archive",
            Self::ConvertFormat => "convert_format",
            Self::FileChecksum => "file_checksum",
            Self::Paste => "paste",
            Self::Rename => "rename",
            Self::BatchRename => "batch_rename",
            Self::NewEntry => "new_entry",
            Self::NewFolderFromSelection => "new_folder_from_selection",
            Self::OpenTerminalHere => "open_terminal_here",
            Self::CopyPath => "copy_path",
            Self::CreateSymlink => "create_symlink",
            Self::Delete => "delete",
            Self::Properties => "properties",
        }
    }

    pub(super) fn from_config_value(value: &str) -> Option<Self> {
        Some(match value {
            "open" => Self::Open,
            "smart_extract_here" => Self::SmartExtractHere,
            "extract_to_archive_folder" => Self::ExtractToArchiveFolder,
            "open_with" => Self::OpenWith,
            "copy" => Self::Copy,
            "duplicate" => Self::Duplicate,
            "move" => Self::Move,
            "tools" => Self::Tools,
            "create_archive" => Self::CreateArchive,
            "convert_format" => Self::ConvertFormat,
            "file_checksum" => Self::FileChecksum,
            "paste" => Self::Paste,
            "rename" => Self::Rename,
            "batch_rename" => Self::BatchRename,
            "new_entry" => Self::NewEntry,
            "new_folder_from_selection" => Self::NewFolderFromSelection,
            "open_terminal_here" => Self::OpenTerminalHere,
            "copy_path" => Self::CopyPath,
            "create_symlink" => Self::CreateSymlink,
            "delete" => Self::Delete,
            "properties" => Self::Properties,
            _ => return None,
        })
    }

    /// 纯触发器锚点:自身无动作,单点仅展开子菜单(成员空时整行省略)。
    pub(crate) fn is_pure_trigger(self) -> bool {
        matches!(self, Self::Tools)
    }

    /// 与 floating_panels 菜单渲染使用的图标保持一致。
    pub(crate) fn icon(self) -> IconSymbol {        match self {
            Self::Open => IconSymbol::Folder,
            Self::SmartExtractHere | Self::ExtractToArchiveFolder => IconSymbol::FileArchive,
            Self::OpenWith => IconSymbol::Monitor,
            Self::Copy | Self::Duplicate | Self::Paste => IconSymbol::Copy,
            Self::Move => IconSymbol::ArrowRight,
            Self::Tools => IconSymbol::Settings,
            Self::CreateArchive => IconSymbol::FileArchive,
            Self::ConvertFormat => IconSymbol::FileImage,
            Self::FileChecksum => IconSymbol::Hash,
            Self::Rename | Self::BatchRename => IconSymbol::Pencil,
            Self::NewEntry => IconSymbol::File,
            Self::NewFolderFromSelection => IconSymbol::Folder,
            Self::OpenTerminalHere => IconSymbol::Terminal,
            Self::CopyPath => IconSymbol::List,
            Self::CreateSymlink => IconSymbol::Link,
            Self::Delete => IconSymbol::Trash,
            Self::Properties => IconSymbol::FileText,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrashMenuItem {
    Restore,
    DeletePermanently,
    Properties,
    EmptyTrash,
}

pub(crate) const TRASH_MENU_ITEMS: [TrashMenuItem; 4] = [
    TrashMenuItem::Restore,
    TrashMenuItem::DeletePermanently,
    TrashMenuItem::Properties,
    TrashMenuItem::EmptyTrash,
];

impl TrashMenuItem {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Restore => "Restore",
            Self::DeletePermanently => "Delete Permanently",
            Self::Properties => "Properties",
            Self::EmptyTrash => "Empty Trash",
        }
    }

    pub(super) fn config_value(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::DeletePermanently => "delete_permanently",
            Self::Properties => "properties",
            Self::EmptyTrash => "empty_trash",
        }
    }

    pub(super) fn from_config_value(value: &str) -> Option<Self> {
        Some(match value {
            "restore" => Self::Restore,
            "delete_permanently" => Self::DeletePermanently,
            "properties" => Self::Properties,
            "empty_trash" => Self::EmptyTrash,
            _ => return None,
        })
    }

    pub(crate) fn icon(self) -> IconSymbol {
        match self {
            Self::Restore => IconSymbol::ArrowLeft,
            Self::DeletePermanently | Self::EmptyTrash => IconSymbol::Trash,
            Self::Properties => IconSymbol::FileText,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchResultMenuItem {
    OpenContainingFolder,
    Copy,
    Cut,
    MoveToTrash,
    DeletePermanently,
}

pub(crate) const SEARCH_RESULT_MENU_ITEMS: [SearchResultMenuItem; 5] = [
    SearchResultMenuItem::OpenContainingFolder,
    SearchResultMenuItem::Copy,
    SearchResultMenuItem::Cut,
    SearchResultMenuItem::MoveToTrash,
    SearchResultMenuItem::DeletePermanently,
];

impl SearchResultMenuItem {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OpenContainingFolder => "Open Containing Folder",
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::MoveToTrash => "Move to Trash",
            Self::DeletePermanently => "Delete Permanently",
        }
    }

    pub(super) fn config_value(self) -> &'static str {
        match self {
            Self::OpenContainingFolder => "open_containing_folder",
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::MoveToTrash => "move_to_trash",
            Self::DeletePermanently => "delete_permanently",
        }
    }

    pub(super) fn from_config_value(value: &str) -> Option<Self> {
        Some(match value {
            "open_containing_folder" => Self::OpenContainingFolder,
            "copy" => Self::Copy,
            "cut" => Self::Cut,
            "move_to_trash" => Self::MoveToTrash,
            "delete_permanently" => Self::DeletePermanently,
            _ => return None,
        })
    }

    pub(crate) fn icon(self) -> IconSymbol {
        match self {
            Self::OpenContainingFolder => IconSymbol::FolderOpen,
            Self::Copy => IconSymbol::Copy,
            Self::Cut => IconSymbol::ArrowRight,
            Self::MoveToTrash | Self::DeletePermanently => IconSymbol::Trash,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BookmarkMenuItem {
    RemoveFromFavorites,
}

pub(crate) const BOOKMARK_MENU_ITEMS: [BookmarkMenuItem; 1] =
    [BookmarkMenuItem::RemoveFromFavorites];

impl BookmarkMenuItem {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::RemoveFromFavorites => "Remove from Favorites",
        }
    }

    pub(super) fn config_value(self) -> &'static str {
        match self {
            Self::RemoveFromFavorites => "remove_from_favorites",
        }
    }

    pub(super) fn from_config_value(value: &str) -> Option<Self> {
        Some(match value {
            "remove_from_favorites" => Self::RemoveFromFavorites,
            _ => return None,
        })
    }

    pub(crate) fn icon(self) -> IconSymbol {
        IconSymbol::Trash
    }
}

/// 侧栏设备菜单与网络连接菜单的项就是现有动作枚举,这里只提供存储字符串转换。
pub(crate) mod device_action_config_values {
    use crate::sidebar_devices::SidebarDeviceAction;

    pub(crate) fn device_config_value(action: SidebarDeviceAction) -> &'static str {
        match action {
            SidebarDeviceAction::Mount => "mount",
            SidebarDeviceAction::Unmount => "unmount",
            SidebarDeviceAction::Eject => "eject",
        }
    }

    pub(crate) fn device_from_config_value(value: &str) -> Option<SidebarDeviceAction> {
        Some(match value {
            "mount" => SidebarDeviceAction::Mount,
            "unmount" => SidebarDeviceAction::Unmount,
            "eject" => SidebarDeviceAction::Eject,
            _ => return None,
        })
    }

    pub(crate) const DEVICE_MENU_ITEMS: [SidebarDeviceAction; 3] = [
        SidebarDeviceAction::Mount,
        SidebarDeviceAction::Unmount,
        SidebarDeviceAction::Eject,
    ];
}

pub(crate) mod network_action_config_values {
    use crate::icons::IconSymbol;
    use crate::network_connections::SidebarNetworkConnectionAction;

    pub(crate) fn network_config_value(action: SidebarNetworkConnectionAction) -> &'static str {
        match action {
            SidebarNetworkConnectionAction::Connect => "connect",
            SidebarNetworkConnectionAction::Disconnect => "disconnect",
            SidebarNetworkConnectionAction::Edit => "edit",
            SidebarNetworkConnectionAction::Remove => "remove",
        }
    }

    pub(crate) fn network_from_config_value(value: &str) -> Option<SidebarNetworkConnectionAction> {
        Some(match value {
            "connect" => SidebarNetworkConnectionAction::Connect,
            "disconnect" => SidebarNetworkConnectionAction::Disconnect,
            "edit" => SidebarNetworkConnectionAction::Edit,
            "remove" => SidebarNetworkConnectionAction::Remove,
            _ => return None,
        })
    }

    pub(crate) fn network_icon(action: SidebarNetworkConnectionAction) -> IconSymbol {
        match action {
            SidebarNetworkConnectionAction::Connect => IconSymbol::Link,
            SidebarNetworkConnectionAction::Disconnect => IconSymbol::Close,
            SidebarNetworkConnectionAction::Edit => IconSymbol::Pencil,
            SidebarNetworkConnectionAction::Remove => IconSymbol::Trash,
        }
    }

    pub(crate) const NETWORK_MENU_ITEMS: [SidebarNetworkConnectionAction; 4] = [
        SidebarNetworkConnectionAction::Connect,
        SidebarNetworkConnectionAction::Disconnect,
        SidebarNetworkConnectionAction::Edit,
        SidebarNetworkConnectionAction::Remove,
    ];
}

pub(super) fn search_entry_type_config_value(preset: SearchEntryTypePreset) -> &'static str {
    match preset {
        SearchEntryTypePreset::Spreadsheets => "spreadsheets",
        SearchEntryTypePreset::Video => "video",
        SearchEntryTypePreset::Images => "images",
        SearchEntryTypePreset::Text => "text",
        SearchEntryTypePreset::Documents => "documents",
        SearchEntryTypePreset::Folders => "folders",
        SearchEntryTypePreset::Audio => "audio",
        SearchEntryTypePreset::Pdf => "pdf",
        SearchEntryTypePreset::Files => "files",
        SearchEntryTypePreset::Archives => "archives",
        SearchEntryTypePreset::Links => "links",
    }
}

pub(super) fn search_entry_type_from_config_value(value: &str) -> Option<SearchEntryTypePreset> {
    Some(match value {
        "spreadsheets" => SearchEntryTypePreset::Spreadsheets,
        "video" => SearchEntryTypePreset::Video,
        "images" => SearchEntryTypePreset::Images,
        "text" => SearchEntryTypePreset::Text,
        "documents" => SearchEntryTypePreset::Documents,
        "folders" => SearchEntryTypePreset::Folders,
        "audio" => SearchEntryTypePreset::Audio,
        "pdf" => SearchEntryTypePreset::Pdf,
        "files" => SearchEntryTypePreset::Files,
        "archives" => SearchEntryTypePreset::Archives,
        "links" => SearchEntryTypePreset::Links,
        _ => return None,
    })
}
