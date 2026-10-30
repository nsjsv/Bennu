//! 条目不覆盖地改名或移动：多步操作失败时回滚到操作前，回不去的条目逐个报告。
//! 落名只用 `RENAME_NOREPLACE`，"目标已被占用"由这一次原子操作判定，不先查再改。

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::recoverable_transfer::{rename_noreplace, NoReplaceRenameError};
use crate::entry_naming::validate_entry_name;
use crate::FileError;

/// 临时名撞名的重试次数。64 位随机名连续撞上只可能是目录被刻意构造，
/// 超过即放弃，不覆盖任何东西。
const TEMPORARY_NAME_ATTEMPTS: usize = 16;

/// 回滚没能把条目放回原处：原位置、条目此刻实际所在位置、放不回去的原因。
/// 用户要靠这三项自己找回文件，所以回滚的每一步失败都必须落到这里。
#[derive(Debug)]
pub struct UnrestoredEntry {
    pub original: PathBuf,
    pub current: PathBuf,
    pub error: FileError,
}

/// 本次新建、回滚或撤销时却没能删掉的目录或链接：目录多半是里面有了别的内容，
/// 链接多半是已被换成了别的东西。
#[derive(Debug)]
pub struct LeftoverEntry {
    pub path: PathBuf,
    pub error: io::Error,
}

/// 改名要用到的文件系统操作。正式路径走真实系统调用；测试注入大小写不敏感、
/// 不支持 no-replace、某一步失败等真实环境难以制造的行为，判定与回退逻辑共用一份。
pub(crate) trait EntryFilesystem {
    fn rename_noreplace(&mut self, from: &Path, to: &Path) -> Result<(), NoReplaceRenameError>;
    fn object_identity(&mut self, path: &Path) -> io::Result<ObjectIdentity>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ObjectIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

/// 建目录、删空目录：收纳与高级新建在改名之外还要用到的操作，同样可注入。
pub(crate) trait EntryTreeFilesystem: EntryFilesystem {
    /// 已存在时返回 `AlreadyExists`，这就是"名字被占用"的判定。
    fn create_dir(&mut self, path: &Path) -> io::Result<()>;
    /// 只删空目录；目录里有内容时失败，不会连带删掉别人的东西。
    fn remove_dir(&mut self, path: &Path) -> io::Result<()>;
    /// 跟随符号链接，与 `mkdir -p` 复用已有中间目录的判定一致。
    fn is_directory(&mut self, path: &Path) -> io::Result<bool>;
}

pub(crate) struct SystemEntryFilesystem;

impl EntryFilesystem for SystemEntryFilesystem {
    fn rename_noreplace(&mut self, from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
        rename_noreplace(from, to)
    }

    fn object_identity(&mut self, path: &Path) -> io::Result<ObjectIdentity> {
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(ObjectIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl EntryTreeFilesystem for SystemEntryFilesystem {
    fn create_dir(&mut self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }

    fn is_directory(&mut self, path: &Path) -> io::Result<bool> {
        Ok(std::fs::metadata(path)?.is_dir())
    }
}

/// 在同一目录内把条目改名为 `new_name`，绝不覆盖已有条目。
pub async fn rename_entry(path: PathBuf, new_name: OsString) -> Result<PathBuf, FileError> {
    // 两次改名与身份比对都是同步系统调用，整体放进一个阻塞任务。
    let execution = tokio::task::spawn_blocking(move || {
        rename_entry_with(&mut SystemEntryFilesystem, &path, &new_name)
    })
    .await;
    match execution {
        Ok(result) => result,
        Err(join_error) if join_error.is_panic() => {
            std::panic::resume_unwind(join_error.into_panic())
        }
        // 阻塞任务只会在运行时关闭时被取消。
        Err(_) => Err(FileError::ApplicationStopping),
    }
}

pub(crate) fn rename_entry_with(
    filesystem: &mut impl EntryFilesystem,
    path: &Path,
    new_name: &OsStr,
) -> Result<PathBuf, FileError> {
    validate_entry_name(new_name).map_err(|reason| FileError::InvalidEntryName {
        name: new_name.to_os_string(),
        reason,
    })?;
    let (Some(parent), Some(current_name)) = (path.parent(), path.file_name()) else {
        return Err(FileError::InvalidInput {
            path: path.to_path_buf(),
            message: "path has no parent directory".to_owned(),
        });
    };
    if current_name == new_name {
        return Ok(path.to_path_buf());
    }
    let target = parent.join(new_name);
    match filesystem.rename_noreplace(path, &target) {
        Ok(()) => Ok(target),
        Err(NoReplaceRenameError::TargetExists) => {
            rename_onto_same_object(filesystem, path, parent, &target)
        }
        Err(error) => Err(no_replace_rename_error(error, path, &target)),
    }
}

/// no-replace 判定目标已存在时，占着目标名的可能就是源本身：大小写不敏感的
/// 文件系统（FAT/exFAT）上只改大小写，或同一对象的另一个硬链接名。前者经临时名
/// 两步完成；后者第二步仍撞上另一个名字，放回原名后按"已占用"报告。
fn rename_onto_same_object(
    filesystem: &mut impl EntryFilesystem,
    path: &Path,
    parent: &Path,
    target: &Path,
) -> Result<PathBuf, FileError> {
    let source = filesystem
        .object_identity(path)
        .map_err(|source| FileError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
    let occupant = match filesystem.object_identity(target) {
        Ok(identity) => identity,
        // 判定"已存在"之后目标又消失：不追着重试，按已占用报告，由用户重新发起。
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(FileError::EntryNameTaken {
                path: target.to_path_buf(),
            })
        }
        Err(source) => {
            return Err(FileError::Metadata {
                path: target.to_path_buf(),
                source,
            })
        }
    };
    if occupant != source {
        return Err(FileError::EntryNameTaken {
            path: target.to_path_buf(),
        });
    }

    let temporary = park_at_temporary_name(filesystem, path, parent)?;
    let Err(error) = filesystem.rename_noreplace(&temporary, target) else {
        return Ok(target.to_path_buf());
    };
    let cause = no_replace_rename_error(error, &temporary, target);
    match filesystem.rename_noreplace(&temporary, path) {
        Ok(()) => Err(cause),
        Err(restore_error) => Err(FileError::EntryMovesIncomplete {
            cause: Box::new(cause),
            unrestored: vec![UnrestoredEntry {
                original: path.to_path_buf(),
                current: temporary.clone(),
                error: no_replace_rename_error(restore_error, &temporary, path),
            }],
            leftover_entries: Vec::new(),
        }),
    }
}

/// 把条目挪到同目录下一个随机的隐藏临时名上。临时名由 no-replace 改名本身占位，
/// 撞名就换一个，不先查再改。
pub(super) fn park_at_temporary_name(
    filesystem: &mut impl EntryFilesystem,
    path: &Path,
    parent: &Path,
) -> Result<PathBuf, FileError> {
    for _ in 0..TEMPORARY_NAME_ATTEMPTS {
        let temporary = parent.join(temporary_entry_name(path, parent)?);
        match filesystem.rename_noreplace(path, &temporary) {
            Ok(()) => return Ok(temporary),
            Err(NoReplaceRenameError::TargetExists) => {}
            Err(error) => return Err(no_replace_rename_error(error, path, &temporary)),
        }
    }
    Err(FileError::EntryNameCandidatesExhausted {
        directory: parent.to_path_buf(),
        base: OsString::from(TEMPORARY_NAME_PREFIX),
    })
}

pub(super) const TEMPORARY_NAME_PREFIX: &str = ".bennu-rename-";

pub(super) fn move_without_replacing(
    filesystem: &mut impl EntryFilesystem,
    from: &Path,
    to: &Path,
) -> Result<(), FileError> {
    filesystem
        .rename_noreplace(from, to)
        .map_err(|error| no_replace_rename_error(error, from, to))
}

fn temporary_entry_name(path: &Path, parent: &Path) -> Result<OsString, FileError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| FileError::Rename {
        from: path.to_path_buf(),
        to: parent.to_path_buf(),
        source: io::Error::other(error.to_string()),
    })?;
    let mut name = String::from(TEMPORARY_NAME_PREFIX);
    for byte in bytes {
        name.push_str(&format!("{byte:02x}"));
    }
    Ok(OsString::from(name))
}

/// `RENAME_NOREPLACE` 的结果统一映射：目标被占用与文件系统不支持是两种独立的
/// 失败，界面据此决定是让用户换名字还是告诉用户这里不能安全改名。
pub(crate) fn no_replace_rename_error(
    error: NoReplaceRenameError,
    from: &Path,
    to: &Path,
) -> FileError {
    match error {
        NoReplaceRenameError::TargetExists => FileError::EntryNameTaken {
            path: to.to_path_buf(),
        },
        NoReplaceRenameError::Unsupported(_) => FileError::SafeRenameUnsupported {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        },
        other => FileError::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source: other.into_io_error(),
        },
    }
}

#[cfg(test)]
#[path = "entry_moves_tests.rs"]
mod tests;
