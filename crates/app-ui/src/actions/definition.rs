use std::path::PathBuf;
use std::sync::Arc;

use file_core::{ArchiveExtractionPlacement, ArchiveFormat};

use super::convert_format::ConvertFormat;
use super::notice::ActionNotice;
use super::template::{CommandTemplate, PathTemplate};
use super::type_filter::TypeFilter;

/// 动作的身份：文件名去掉 `.toml`。快捷键按它记，所以改了文件名要重新绑键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ActionId(Arc<str>);

impl ActionId {
    /// 存储里写成 `action:<文件名>`，和内置动作的键放在同一个列表里也不会撞名。
    const CONFIG_PREFIX: &'static str = "action:";

    pub(crate) fn from_file_stem(stem: &str) -> Self {
        Self(Arc::from(stem))
    }

    pub(crate) fn from_config_key(key: &str) -> Option<Self> {
        key.strip_prefix(Self::CONFIG_PREFIX)
            .filter(|stem| !stem.is_empty())
            .map(Self::from_file_stem)
    }

    pub(crate) fn config_key(&self) -> String {
        format!("{}{}", Self::CONFIG_PREFIX, self.0)
    }

    pub(crate) fn file_stem(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionDefinition {
    pub(crate) id: ActionId,
    pub(crate) name: String,
    pub(crate) file: PathBuf,
    pub(crate) types: TypeFilter,
    /// 至少一步；`Trash` 只会是最后一步。
    pub(crate) steps: Vec<ActionStep>,
    pub(crate) notice: ActionNotice,
}

/// 一步拿到"当前这批文件"，把结果交给下一步。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActionStep {
    Convert(ConvertFormat),
    Compress(ArchiveFormat),
    Extract(ArchiveExtractionPlacement),
    CopyTo(PathTemplate),
    MoveTo(PathTemplate),
    Trash(TrashScope),
    Run(CommandTemplate),
    CopyPaths,
}

impl ActionStep {
    /// 一步的简短说明：队列面板的任务标题、设置页的步骤摘要。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Convert(format) => tr!("Convert to %1", format.name()),
            Self::Compress(format) => tr!("Compress to %1", format.extension()),
            Self::Extract(ArchiveExtractionPlacement::Smart) => tr!("Smart Extract Here"),
            Self::Extract(ArchiveExtractionPlacement::ArchiveFolder) => {
                tr!("Extract to Archive Folder")
            }
            Self::CopyTo(folder) => tr!("Copy to %1", folder.text()),
            Self::MoveTo(folder) => tr!("Move to %1", folder.text()),
            Self::Trash(TrashScope::Current) => tr!("Move to Trash"),
            Self::Trash(TrashScope::Selected) => tr!("Move the selected items to the trash"),
            Self::Run(command) => tr!("Run %1", command.text()),
            Self::CopyPaths => tr!("Copy Path"),
        }
    }
}

/// 移到回收站的是哪一批。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrashScope {
    /// 上一步交过来的（第一步时就是选中的项）。
    Current,
    /// 动作开始时选中的项，比如解压完把压缩包移到回收站。
    Selected,
}
