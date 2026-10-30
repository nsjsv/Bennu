//! 测试用的单目录内存文件系统，供单条重命名与批量重命名共用。

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use super::entry_moves::{EntryFilesystem, ObjectIdentity, TEMPORARY_NAME_PREFIX};
use super::recoverable_transfer::NoReplaceRenameError;

/// 判定某一步改名（from → to）是否注入失败。
pub(super) type InjectedRenameFailure = Box<dyn Fn(&Path, &Path) -> bool>;

/// 条目名 → 对象身份（inode 即对象编号）。`case_insensitive` 模拟 FAT/exFAT 的
/// 名字比较；`unsupported` 模拟不支持 `RENAME_NOREPLACE`；`fail_when` 让指定的那一步
/// 改名失败。真实环境里这些行为都无法在测试机上稳定制造。
pub(super) struct FakeDirectory {
    entries: Vec<(OsString, ObjectIdentity)>,
    case_insensitive: bool,
    pub(super) unsupported: bool,
    pub(super) fail_when: InjectedRenameFailure,
}

impl FakeDirectory {
    pub(super) fn new(entries: &[(&str, u64)]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(|(name, inode)| (OsString::from(name), fake_identity(*inode)))
                .collect(),
            case_insensitive: false,
            unsupported: false,
            fail_when: Box::new(|_, _| false),
        }
    }

    pub(super) fn case_insensitive(mut self) -> Self {
        self.case_insensitive = true;
        self
    }

    fn key(&self, name: &OsStr) -> String {
        let name = name.to_string_lossy();
        if self.case_insensitive {
            name.to_lowercase()
        } else {
            name.into_owned()
        }
    }

    pub(super) fn position(&self, path: &Path) -> Option<usize> {
        let key = self.key(path.file_name()?);
        self.entries
            .iter()
            .position(|(name, _)| self.key(name) == key)
    }

    /// 某个名字下的对象编号；名字不存在时为 `None`。
    pub(super) fn object_at(&self, name: &str) -> Option<u64> {
        self.position(&fake_path(name))
            .map(|index| self.entries[index].1.inode)
    }

    pub(super) fn names(&self) -> Vec<String> {
        let mut names = self
            .entries
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

impl EntryFilesystem for FakeDirectory {
    fn rename_noreplace(&mut self, from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
        if self.unsupported {
            return Err(NoReplaceRenameError::Unsupported(io::Error::from(
                io::ErrorKind::InvalidInput,
            )));
        }
        if (self.fail_when)(from, to) {
            return Err(NoReplaceRenameError::Failed(io::Error::from(
                io::ErrorKind::PermissionDenied,
            )));
        }
        let Some(source) = self.position(from) else {
            return Err(NoReplaceRenameError::Failed(io::Error::from(
                io::ErrorKind::NotFound,
            )));
        };
        if self.position(to).is_some() {
            return Err(NoReplaceRenameError::TargetExists);
        }
        self.entries[source].0 = to.file_name().unwrap().to_os_string();
        Ok(())
    }

    fn object_identity(&mut self, path: &Path) -> io::Result<ObjectIdentity> {
        self.position(path)
            .map(|index| self.entries[index].1)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

fn fake_identity(inode: u64) -> ObjectIdentity {
    ObjectIdentity { device: 1, inode }
}

pub(super) fn fake_path(name: &str) -> PathBuf {
    Path::new("/fake").join(name)
}

pub(super) fn is_temporary_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(TEMPORARY_NAME_PREFIX))
}
