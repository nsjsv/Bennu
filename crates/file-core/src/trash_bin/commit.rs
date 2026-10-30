use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::catalog::{
    effective_user_id, inspect_directory, inspect_trash_object, probe_home_location,
    probe_volume_location, trash_data_home, DirectoryExpectation,
};
use super::model::{
    OriginalPathBase, TrashEntryIdentity, TrashLocationGuard, TrashLocationKind, TrashRestoreEntry,
    VerifiedTrashDirectory,
};
use super::mountinfo::{mounts_candidate_for_trash_probing, parse_mountinfo, MOUNTINFO_PATH};
use super::operations::trash_info_name;
use super::trash_info::{create_trash_info, remove_unused_trash_info};
use crate::mount_table::MountTableEntry;
use crate::ops::{rename_noreplace, NoReplaceRenameError};
use crate::{FileError, ScanWarning};

/// 放入回收站并返回确切的条目，照 GLib `g_local_file_trash`：选好位置，先独占
/// `.trashinfo` 名字，再把文件移进 `files/`。位置用的是扫描同一套检查，所以放进去
/// 的东西回收站页面一定看得到。
pub async fn trash_path(
    path: impl AsRef<Path>,
    cancellation: CancellationToken,
) -> Result<TrashRestoreEntry, FileError> {
    let path = path.as_ref().to_path_buf();
    let worker_path = path.clone();
    tokio::task::spawn_blocking(move || {
        TrashCommitContext::current()?.trash_path(&worker_path, &cancellation)
    })
    .await
    .map_err(|join_error| FileError::Trash {
        path,
        message: format!("Trash worker failed: {join_error}"),
    })?
}

/// 每次放入各读一次 mountinfo，同 GLib 每个文件各自查挂载点。
struct TrashCommitContext {
    data_home: PathBuf,
    uid: u32,
    mounts: Vec<MountTableEntry>,
}

impl TrashCommitContext {
    fn current() -> Result<Self, FileError> {
        let mountinfo = fs::read(MOUNTINFO_PATH).map_err(|source| FileError::ReadDirectory {
            path: PathBuf::from(MOUNTINFO_PATH),
            source,
        })?;
        Ok(Self {
            data_home: trash_data_home()?,
            uid: effective_user_id(),
            mounts: parse_mountinfo(&mountinfo).mounts,
        })
    }

    fn trash_path(
        &self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<TrashRestoreEntry, FileError> {
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let path = canonical_trash_source_path(path)?;
        let location = self.prepare_location(&path)?;
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        move_into_location(&path, location)
    }

    /// 与 Home Trash 同一挂载用 Home Trash；否则只用源挂载顶层的卷回收站，不退回
    /// Home Trash 跨盘复制（同 GLib）。按挂载点而不是 `st_dev` 判断，同设备的 bind
    /// mount 之间 rename 也会失败。
    fn prepare_location(&self, path: &Path) -> Result<TrashLocationGuard, FileError> {
        let unavailable = |reason: String| FileError::TrashUnavailable {
            path: path.to_path_buf(),
            reason,
        };
        let source = inspect_trash_object(path).map_err(|source| FileError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
        let source_mount = mount_containing(&self.mounts, path)
            .ok_or_else(|| unavailable("its mount point is unknown".to_owned()))?;
        let home_root = self.data_home.join("Trash");
        let canonical_home_root =
            canonicalize_path_or_existing_parent(&home_root).map_err(|source| {
                FileError::Metadata {
                    path: home_root.clone(),
                    source,
                }
            })?;
        if mount_containing(&self.mounts, &canonical_home_root) == Some(source_mount) {
            return self.prepare_home_location(&home_root).map_err(unavailable);
        }

        let top_directory = &source_mount.mount_point;
        if !mounts_candidate_for_trash_probing(source_mount) {
            return Err(unavailable(format!(
                "Trash is not supported on system internal mounts ({} on {})",
                source_mount.fs_type.to_string_lossy(),
                top_directory.display()
            )));
        }
        let top = inspect_directory(top_directory, DirectoryExpectation::MountTop)
            .map_err(|problem| unavailable(describe_warnings(&[problem])))?;
        // 挂载里另一个设备（如未单独挂载的 btrfs 子卷）没有自己的挂载点，扫描看不到。
        if top.identity.device != source.device {
            return Err(unavailable(format!(
                "it is on another device than {}",
                top_directory.display()
            )));
        }
        self.prepare_volume_location(top_directory, &top)
            .map_err(unavailable)
    }

    fn prepare_home_location(&self, root: &Path) -> Result<TrashLocationGuard, String> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.data_home)
            .map_err(|error| format!("cannot create {}: {error}", self.data_home.display()))?;
        let mut warnings = Vec::new();
        if let Err(problem) = create_trash_directories(root) {
            warnings.push(problem);
        } else if let Some(location) =
            probe_home_location(&self.data_home, root, self.uid, &mut warnings)
        {
            return Ok(location);
        }
        Err(describe_warnings(&warnings))
    }

    /// 规范顺序：先 `$top/.Trash/$uid`（`.Trash` 须有 sticky bit），不行再
    /// `$top/.Trash-$uid`；用不了的不报给用户，只在两个都不行时合起来说明原因。
    fn prepare_volume_location(
        &self,
        top_directory: &Path,
        top: &VerifiedTrashDirectory,
    ) -> Result<TrashLocationGuard, String> {
        let mut warnings = Vec::new();
        let shared_root_path = top_directory.join(".Trash");
        if let Ok(shared_root) = inspect_directory(
            &shared_root_path,
            DirectoryExpectation::SharedRoot {
                device: top.identity.device,
            },
        ) {
            let user_root = shared_root_path.join(self.uid.to_string());
            if let Err(problem) = create_trash_directories(&user_root) {
                warnings.push(problem);
            } else if let Some(location) = probe_volume_location(
                TrashLocationKind::SharedVolume,
                top_directory,
                top,
                Some(shared_root),
                &user_root,
                self.uid,
                &mut warnings,
            ) {
                return Ok(location);
            }
        }

        let private_root = top_directory.join(format!(".Trash-{}", self.uid));
        if let Err(problem) = create_trash_directories(&private_root) {
            warnings.push(problem);
        } else if let Some(location) = probe_volume_location(
            TrashLocationKind::PrivateVolume,
            top_directory,
            top,
            None,
            &private_root,
            self.uid,
            &mut warnings,
        ) {
            return Ok(location);
        }
        Err(describe_warnings(&warnings))
    }
}

fn create_trash_directories(root: &Path) -> Result<(), ScanWarning> {
    for directory in [root.to_path_buf(), root.join("files"), root.join("info")] {
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ScanWarning {
                    path: directory,
                    message: format!("cannot create the Trash directory: {error}"),
                })
            }
        }
    }
    Ok(())
}

fn describe_warnings(warnings: &[ScanWarning]) -> String {
    warnings
        .iter()
        .map(|warning| format!("{}: {}", warning.path.display(), warning.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn move_into_location(
    path: &Path,
    location: TrashLocationGuard,
) -> Result<TrashRestoreEntry, FileError> {
    let trash_error = |message: String| FileError::Trash {
        path: path.to_path_buf(),
        message,
    };
    let name = path
        .file_name()
        .expect("canonical Trash source paths end in a file name");
    let path_value = match location.original_path_base {
        OriginalPathBase::Absolute => path,
        OriginalPathBase::RelativeToTopDirectory => path
            .strip_prefix(&location.top_directory)
            .expect("the source lies under the mount top that holds its Trash"),
    };
    for sequence in 1_u64.. {
        let item_name = candidate_item_name(name, sequence);
        let info_path = location.info.path.join(trash_info_name(&item_name));
        let info = match create_trash_info(&info_path, path_value) {
            Ok(info) => info,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(trash_error(format!(
                    "cannot create {}: {error}",
                    info_path.display()
                )))
            }
        };
        let trash_path = location.files.path.join(&item_name);
        if let Err(error) = move_without_replacing(path, &trash_path) {
            remove_unused_trash_info(&info_path);
            match error {
                // `files/` 里有没有 info 的孤儿：不碰它，换下一个名字。
                NoReplaceRenameError::TargetExists => continue,
                NoReplaceRenameError::CrossDevice => {
                    return Err(FileError::TrashUnavailable {
                        path: path.to_path_buf(),
                        reason: format!(
                            "{} is on another file system",
                            location.files.path.display()
                        ),
                    })
                }
                error => return Err(trash_error(error.into_io_error().to_string())),
            }
        }
        // rename 会改 ctime，payload 身份只能在移动后取。
        let payload = inspect_trash_object(&trash_path).map_err(|error| {
            trash_error(format!(
                "moved to Trash, but {} cannot be inspected: {error}",
                trash_path.display()
            ))
        })?;
        return Ok(TrashRestoreEntry {
            trash_path,
            info_path,
            original_path: path.to_path_buf(),
            identity: Some(TrashEntryIdentity {
                location,
                item_name,
                info,
                payload,
            }),
        });
    }
    unreachable!("Trash item name candidates are unbounded")
}

/// 文件系统不支持 `RENAME_NOREPLACE`（如 ntfs-3g）时先确认目标不存在再普通改名。
/// 名字已由 `.trashinfo` 独占（规范里名字归属以它为准），这里只防 `files/` 里的孤儿，
/// 同 GLib/KDE。
fn move_without_replacing(from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
    match rename_noreplace(from, to) {
        Err(NoReplaceRenameError::Unsupported(_)) => rename_if_absent(from, to),
        moved => moved,
    }
}

fn rename_if_absent(from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(NoReplaceRenameError::TargetExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::rename(from, to).map_err(|error| match error.kind() {
                io::ErrorKind::CrossesDevices => NoReplaceRenameError::CrossDevice,
                _ => NoReplaceRenameError::Failed(error),
            })
        }
        Err(error) => Err(NoReplaceRenameError::Failed(error)),
    }
}

/// 同名时照 GLib `get_unique_filename`：编号插在第一个点前（`a.2.tar.gz`、`.2.env`），
/// 没有点就接在末尾。
fn candidate_item_name(name: &OsStr, sequence: u64) -> OsString {
    if sequence == 1 {
        return name.to_os_string();
    }
    let bytes = name.as_bytes();
    let (stem, extension) = bytes.split_at(
        bytes
            .iter()
            .position(|byte| *byte == b'.')
            .unwrap_or(bytes.len()),
    );
    let mut candidate = stem.to_vec();
    candidate.extend_from_slice(format!(".{sequence}").as_bytes());
    candidate.extend_from_slice(extension);
    OsString::from_vec(candidate)
}

/// 只 canonicalize 父目录，不跟随叶子：放入回收站的是链接本身。
fn canonical_trash_source_path(path: &Path) -> Result<PathBuf, FileError> {
    let invalid = |message: &str| FileError::Trash {
        path: path.to_path_buf(),
        message: message.to_owned(),
    };
    if !path.is_absolute() {
        return Err(invalid("moving to Trash requires an absolute source path"));
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(invalid(
            "the path does not name an item that can be moved to Trash",
        ));
    };
    let canonical_parent = fs::canonicalize(parent).map_err(|source| FileError::Metadata {
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(canonical_parent.join(name))
}

/// Home Trash 可能还没建：取最近存在的上级 canonicalize，再接上缺的部分，
/// `XDG_DATA_HOME` 经过符号链接时也能判对挂载点。
fn canonicalize_path_or_existing_parent(path: &Path) -> io::Result<PathBuf> {
    let mut existing_parent = path;
    let mut missing_components = Vec::new();
    loop {
        match fs::canonicalize(existing_parent) {
            Ok(canonical) => {
                return Ok(missing_components
                    .iter()
                    .rev()
                    .fold(canonical, |resolved, component: &OsString| {
                        resolved.join(component)
                    }));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(component), Some(parent)) =
                    (existing_parent.file_name(), existing_parent.parent())
                else {
                    return Err(error);
                };
                missing_components.push(component.to_os_string());
                existing_parent = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

/// 最深的挂载点；同一挂载点叠了几层时取 mountinfo 里最后一层（看得见的那层）。
fn mount_containing<'a>(mounts: &'a [MountTableEntry], path: &Path) -> Option<&'a MountTableEntry> {
    mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.components().count())
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
