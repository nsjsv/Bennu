//! 一个动作文件夹读出来的全部内容：认得的动作，加上用不了的文件和原因。在后台线程整份读出来，
//! 读完一次替换，菜单不会看到读到一半的结果。

use std::fmt;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::mime::MimeDatabase;

use super::action_file::parse_action_file;
use super::definition::{ActionDefinition, ActionId};
use super::type_filter::{ItemType, TargetTypes};

/// 动作文件夹：`$XDG_CONFIG_HOME/bennu/actions`。
pub(crate) fn action_folder() -> PathBuf {
    file_core::app_dirs::app_config_dir().join("actions")
}

/// 动作文件不会这么大；更大的多半不是动作文件，不读。
const LARGEST_ACTION_FILE: u64 = 256 * 1024;

/// 用不了的文件（或者整个文件夹）和原因，在设置的"动作"页里列出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionFileProblem {
    pub(crate) file: PathBuf,
    pub(crate) line: Option<usize>,
    pub(crate) reason: String,
}

#[derive(Clone)]
pub(crate) struct ActionLibrary {
    /// 按名字排序。
    definitions: Vec<Arc<ActionDefinition>>,
    problems: Vec<ActionFileProblem>,
    /// 读取时拿到的类型库，菜单匹配用它，界面线程上不再碰系统库的文件。
    types: Arc<MimeDatabase>,
}

impl fmt::Debug for ActionLibrary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionLibrary")
            .field("definitions", &self.definitions)
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

/// 动作要作用的那一批的类型：选中的项，或者在空白处右键时的当前文件夹。
pub(crate) struct ActionTarget(TargetTypes);

impl ActionLibrary {
    /// 第一次读完之前：没有动作。
    pub(crate) fn empty() -> Self {
        Self {
            definitions: Vec::new(),
            problems: Vec::new(),
            types: Arc::new(MimeDatabase::load(Vec::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_definitions(definitions: Vec<ActionDefinition>) -> Self {
        Self {
            definitions: definitions.into_iter().map(Arc::new).collect(),
            problems: Vec::new(),
            types: MimeDatabase::system(),
        }
    }

    /// 会读盘，只在后台线程调用。
    pub(crate) fn load(folder: &Path) -> Self {
        let mut library = Self {
            definitions: Vec::new(),
            problems: Vec::new(),
            types: MimeDatabase::system(),
        };
        let folder_problem = |reason: String| ActionFileProblem {
            file: folder.to_path_buf(),
            line: None,
            reason,
        };
        let metadata = match std::fs::metadata(folder) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return library,
            Err(error) => {
                library.problems.push(folder_problem(error.to_string()));
                return library;
            }
        };
        if let Some(reason) = untrusted(&metadata) {
            library.problems.push(folder_problem(reason));
            return library;
        }
        let mut files = match std::fs::read_dir(folder) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "toml")
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                library.problems.push(folder_problem(error.to_string()));
                return library;
            }
        };
        files.sort();
        for file in files {
            match library.read_action(&file) {
                Ok(definition) => library.definitions.push(Arc::new(definition)),
                Err(problem) => library.problems.push(problem),
            }
        }
        library
            .definitions
            .sort_by(|left, right| left.name.cmp(&right.name));
        library
    }

    fn read_action(&self, file: &Path) -> Result<ActionDefinition, ActionFileProblem> {
        let problem = |line: Option<usize>, reason: String| ActionFileProblem {
            file: file.to_path_buf(),
            line,
            reason,
        };
        let stem = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .ok_or_else(|| problem(None, tr!("The file name has to be text")))?;
        let text = read_trusted_text(file).map_err(|reason| problem(None, reason))?;
        parse_action_file(ActionId::from_file_stem(stem), file, &text, &self.types)
            .map_err(|mistake| problem(mistake.line, mistake.reason))
    }

    pub(crate) fn definitions(&self) -> &[Arc<ActionDefinition>] {
        &self.definitions
    }

    pub(crate) fn problems(&self) -> &[ActionFileProblem] {
        &self.problems
    }

    pub(crate) fn find(&self, id: &ActionId) -> Option<&Arc<ActionDefinition>> {
        self.definitions
            .iter()
            .find(|definition| definition.id == *id)
    }

    /// `items` 是（路径, 是不是文件夹）。
    pub(crate) fn target<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a Path, bool)>,
    ) -> ActionTarget {
        ActionTarget(TargetTypes::new(
            items
                .into_iter()
                .map(|(path, is_directory)| ItemType::of(path, is_directory, &self.types)),
            &self.types,
        ))
    }

    /// 对这一批适用的动作，按名字排序。一项都没有时没有动作。
    pub(crate) fn matching<'a>(
        &'a self,
        target: &'a ActionTarget,
    ) -> impl Iterator<Item = &'a Arc<ActionDefinition>> + 'a {
        self.definitions.iter().filter(move |definition| {
            !target.0.is_empty() && definition.types.matches_all(&target.0)
        })
    }
}

/// 打开文件后对同一个文件描述符检查属主和权限再读，检查和读取之间文件换不掉。不阻塞地打开，
/// 叫这个名字的管道也不会卡住后台线程。
fn read_trusted_text(file: &Path) -> Result<String, String> {
    let mut handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)
        .map_err(|error| error.to_string())?;
    let metadata = handle.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err(tr!("It is not a regular file"));
    }
    if let Some(reason) = untrusted(&metadata) {
        return Err(reason);
    }
    if metadata.len() > LARGEST_ACTION_FILE {
        return Err(tr!("It is too large to be an action file"));
    }
    read_text(&mut handle)
}

fn read_text(handle: &mut File) -> Result<String, String> {
    let mut bytes = Vec::new();
    handle
        .take(LARGEST_ACTION_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|_| tr!("It is not UTF-8 text"))
}

fn untrusted(metadata: &Metadata) -> Option<String> {
    trust_problem(
        metadata.uid(),
        metadata.mode(),
        rustix::process::getuid().as_raw(),
    )
}

/// 动作能执行命令，所以只用当前用户自己（或 root）的、别人改不了的文件和文件夹，
/// 和 sshd 对 `authorized_keys` 的要求一样。
fn trust_problem(owner: u32, mode: u32, user: u32) -> Option<String> {
    if owner != user && owner != 0 {
        return Some(tr!("It belongs to another user, so it is not used"));
    }
    if mode & 0o002 != 0 {
        return Some(tr!("Other users can change it, so it is not used"));
    }
    None
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
