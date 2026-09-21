use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::catalog::revalidate_trash_location;
use super::model::{
    TrashCommitOutcome, TrashEntryIdentity, TrashLocationGuard, TrashObjectIdentity,
    TrashRestoreEntry,
};
use super::operations::{
    commit_path_with_shared_tracking, prepare_restore_target, remove_trash_info,
    remove_trash_payload, verify_entry_identity_with_locations, verify_trash_info_identity,
    RestoreTarget, TrashTrackingShared,
};
use crate::ops::rename_noreplace;
use crate::{FileError, TransferConflictStrategy};

/// 一个批次内已通过全套目录验证的 Trash location,以 trash_root 身份为缓存键。
/// 命中即跳过整套目录重验;整个回收站被外力替换时键失配会重新全套验证,
/// 而 files/info 子树被换的场景由逐条 payload/info 的 inode 比对兜底。
#[derive(Default)]
pub(super) struct VerifiedTrashLocations {
    verified: HashMap<PathBuf, TrashObjectIdentity>,
}

impl VerifiedTrashLocations {
    pub(super) fn revalidate_once(
        &mut self,
        location: &TrashLocationGuard,
    ) -> Result<(), FileError> {
        let root = &location.trash_root;
        if self
            .verified
            .get(&root.path)
            .is_some_and(|verified| *verified == root.identity)
        {
            return Ok(());
        }
        revalidate_trash_location(location)?;
        self.verified
            .insert(root.path.clone(), root.identity.clone());
        Ok(())
    }
}

/// 批量回收站条目操作的共享验证上下文:同一批条目共享的位置全套验证整批只做
/// 一次,逐条的身份快照比对(payload/info 的 inode 级确认)逐条保留。调用侧在
/// 一个循环里通过同一实例处理整批条目,实例丢弃即批次结束。
#[derive(Default, Clone)]
pub struct TrashVerificationBatch {
    locations: Arc<Mutex<VerifiedTrashLocations>>,
}

impl TrashVerificationBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// 单条入口复用同一实现,空缓存退化为逐条全套位置验证。
    pub(super) fn single_entry() -> Self {
        Self::new()
    }

    pub async fn delete_entry(&self, entry: TrashRestoreEntry) -> Result<(), FileError> {
        let locations = self.locations.clone();
        tokio::task::spawn_blocking(move || delete_entry_on_batch(&entry, &locations))
            .await
            .map_err(|join_error| FileError::Trash {
                path: PathBuf::from("Trash"),
                message: format!("Trash batch delete worker failed: {join_error}"),
            })?
    }

    pub async fn restore_entry(
        &self,
        entry: TrashRestoreEntry,
        conflict_strategy: TransferConflictStrategy,
    ) -> Result<PathBuf, FileError> {
        let identity = self.verify_identity(&entry).await?;
        let restore_target = prepare_restore_target(&entry, conflict_strategy).await?;
        let target = match &restore_target {
            RestoreTarget::Skip => return Ok(entry.original_path),
            RestoreTarget::MoveNoReplace(target) | RestoreTarget::MergeDirectory(target) => {
                target.clone()
            }
        };
        if let Some(parent) = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| FileError::Move {
                    from: entry.trash_path.clone(),
                    to: target.clone(),
                    source,
                })?;
        }
        match restore_target {
            RestoreTarget::MoveNoReplace(_) => {
                rename_noreplace(&entry.trash_path, &target).map_err(|error| FileError::Move {
                    from: entry.trash_path.clone(),
                    to: target.clone(),
                    source: error.into_io_error(),
                })?;
            }
            RestoreTarget::MergeDirectory(_) => {
                tokio::fs::rename(&entry.trash_path, &target)
                    .await
                    .map_err(|source| FileError::Move {
                        from: entry.trash_path.clone(),
                        to: target.clone(),
                        source,
                    })?;
            }
            RestoreTarget::Skip => unreachable!("skip returns before restore commit"),
        }
        // payload 移走后凭据若被换过必然 inode 失配,逐条信息验证足够;
        // 位置级全套验证已由批次缓存覆盖。
        let verify_entry = entry.clone();
        tokio::task::spawn_blocking(move || verify_trash_info_identity(&verify_entry, &identity))
            .await
            .map_err(|join_error| FileError::Trash {
                path: PathBuf::from("Trash"),
                message: format!("Trash batch verify worker failed: {join_error}"),
            })??;
        tokio::fs::remove_file(&entry.info_path)
            .await
            .map_err(|source| FileError::Delete {
                path: entry.info_path.clone(),
                source,
            })?;
        Ok(target)
    }

    async fn verify_identity(
        &self,
        entry: &TrashRestoreEntry,
    ) -> Result<TrashEntryIdentity, FileError> {
        let entry = entry.clone();
        let locations = self.locations.clone();
        tokio::task::spawn_blocking(move || {
            let mut locations = locations.lock().expect("trash batch locations mutex");
            verify_entry_identity_with_locations(&entry, &mut locations)
        })
        .await
        .map_err(|join_error| FileError::Trash {
            path: PathBuf::from("Trash"),
            message: format!("Trash batch verify worker failed: {join_error}"),
        })?
    }
}

fn delete_entry_on_batch(
    entry: &TrashRestoreEntry,
    locations: &Mutex<VerifiedTrashLocations>,
) -> Result<(), FileError> {
    let identity = {
        let mut locations = locations.lock().expect("trash batch locations mutex");
        verify_entry_identity_with_locations(entry, &mut locations)?
    };
    remove_trash_payload(entry, identity.payload.kind)?;
    verify_trash_info_identity(entry, &identity)?;
    remove_trash_info(entry)
}

/// 批量「移入回收站」:mountinfo 快照与各卷移入前凭据快照整批共享一次,
/// 逐条仍由 trash crate 落盘并精确找回撤销条目。调用侧同一循环复用同一实例。
#[derive(Default)]
pub struct TrashCommitBatch {
    shared: Option<Arc<Mutex<TrashTrackingShared>>>,
}

impl TrashCommitBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn commit(
        &mut self,
        path: impl AsRef<Path>,
        cancellation: CancellationToken,
    ) -> Result<TrashCommitOutcome, FileError> {
        let path = path.as_ref().to_path_buf();
        let shared = match &self.shared {
            Some(shared) => shared.clone(),
            None => {
                let shared = tokio::task::spawn_blocking({
                    let cancellation = cancellation.clone();
                    move || TrashTrackingShared::prepare(cancellation)
                })
                .await
                .map_err(|join_error| FileError::Trash {
                    path: PathBuf::from("/proc/self/mountinfo"),
                    message: format!("Trash tracking worker failed: {join_error}"),
                })??;
                let shared = Arc::new(Mutex::new(shared));
                self.shared = Some(shared.clone());
                shared
            }
        };
        tokio::task::spawn_blocking(move || {
            let mut shared = shared.lock().expect("trash commit batch mutex");
            commit_path_with_shared_tracking(path, cancellation, &mut shared)
        })
        .await
        .map_err(|join_error| FileError::Trash {
            path: PathBuf::from("/proc/self/mountinfo"),
            message: format!("Trash operation worker failed: {join_error}"),
        })?
    }
}
