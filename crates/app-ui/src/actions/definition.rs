use std::path::PathBuf;
use std::sync::Arc;

use file_core::{ArchiveExtractionPlacement, ArchiveFormat};

use super::convert_format::ConvertFormat;
use super::notice::ActionNotice;
use super::template::{CommandTemplate, PathTemplate};
use super::type_filter::TypeFilter;

/// 动作的身份。快捷键按它记，所以改了文件名要重新绑键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ActionId {
    /// Bennu 的动作文件：文件名去掉 `.toml`。
    File(Arc<str>),
    ServiceMenu(ServiceMenuKey),
}

/// Dolphin 服务菜单里的一项：`.desktop` 文件名（同名文件只用一个，所以它就够分辨文件），
/// 加上 `[Desktop Action <键>]` 的键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ServiceMenuKey {
    pub(crate) file: Arc<str>,
    pub(crate) action: Arc<str>,
}

impl ActionId {
    /// 存储里写成 `action:<文件名>` 和 `servicemenu:<文件名>/<键>`，和内置动作的键放在同一个列表里
    /// 也不会撞名。文件名里不会有 `/`，按第一个 `/` 分开。
    const FILE_PREFIX: &'static str = "action:";
    const SERVICE_MENU_PREFIX: &'static str = "servicemenu:";

    pub(crate) fn from_file_stem(stem: &str) -> Self {
        Self::File(Arc::from(stem))
    }

    pub(crate) fn service_menu(file: &str, action: &str) -> Self {
        Self::ServiceMenu(ServiceMenuKey {
            file: Arc::from(file),
            action: Arc::from(action),
        })
    }

    pub(crate) fn from_config_key(key: &str) -> Option<Self> {
        if let Some(stem) = key.strip_prefix(Self::FILE_PREFIX) {
            return (!stem.is_empty()).then(|| Self::from_file_stem(stem));
        }
        let (file, action) = key
            .strip_prefix(Self::SERVICE_MENU_PREFIX)?
            .split_once('/')?;
        (!file.is_empty() && !action.is_empty()).then(|| Self::service_menu(file, action))
    }

    pub(crate) fn config_key(&self) -> String {
        match self {
            Self::File(stem) => format!("{}{stem}", Self::FILE_PREFIX),
            Self::ServiceMenu(key) => {
                format!("{}{}/{}", Self::SERVICE_MENU_PREFIX, key.file, key.action)
            }
        }
    }

    /// 文件不在了、读不出名字时显示的：文件名（服务菜单再加上它的键）。
    pub(crate) fn fallback_label(&self) -> String {
        match self {
            Self::File(stem) => stem.to_string(),
            Self::ServiceMenu(key) => format!("{}/{}", key.file, key.action),
        }
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
    /// 写了 `[watch]` 的动作也是一条规则：自己跑。
    pub(crate) watch: Option<ActionWatch>,
}

/// 规则：监视一个文件夹（不进子文件夹），新出现或改过、写完了的文件对得上 `query` 就对它自动跑
/// 这个动作。`query` 是超级输入框 `f:` 的写法，用的时候再读（时间筛选按那时算）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionWatch {
    pub(crate) query: String,
    pub(crate) folder: PathBuf,
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
