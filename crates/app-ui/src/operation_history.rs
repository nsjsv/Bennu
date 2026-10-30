use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::{
    BatchRenameItem, CompletedBatchRename, FileOperationVerification, TransferConflictStrategy,
    TrashRestoreEntry,
};
use file_operations::{CompletedTransfer, FileOperationHistoryEligibility, FileOperationOutcome};

use file_operations::{QueuedFileOperation, QueuedTransfer};

#[derive(Debug, Clone)]
pub(crate) struct PendingHistoryOperation {
    direction: HistoryDirection,
    item: FileOperationHistoryItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryDirection {
    Undo,
    Redo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileOperationHistoryItem {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    BatchRename {
        renames: Vec<CompletedBatchRename>,
    },
    CreateDirectory {
        path: PathBuf,
    },
    /// 高级新建文件夹成果。没移入条目时撤销 = 顶层新目录整批进回收站（与单建
    /// CreateDirectory 同语义）；移入过条目时撤销 = 条目挪回原位、删掉新建的空目录
    /// （与撤销收纳一致），重做不再可用（源路径已变）。
    CreateDirectories {
        parent: PathBuf,
        leaves: Vec<String>,
        leaf_paths: Vec<PathBuf>,
        created: Vec<PathBuf>,
        moved: Vec<CompletedTransfer>,
    },
    CreateEmptyFile {
        path: PathBuf,
    },
    Trash {
        original_paths: Vec<PathBuf>,
        restore_entries: Vec<TrashRestoreEntry>,
    },
    Restore {
        restore_entries: Vec<TrashRestoreEntry>,
        restored_paths: Vec<PathBuf>,
    },
    Copy {
        transfers: Vec<CompletedTransfer>,
    },
    Move {
        transfers: Vec<CompletedTransfer>,
    },
    GatheredIntoNewFolder {
        directory: PathBuf,
        moved: Vec<CompletedTransfer>,
    },
}

#[derive(Debug, Default)]
pub(crate) struct FileOperationHistory {
    undo_stack: Vec<FileOperationHistoryItem>,
    redo_stack: Vec<FileOperationHistoryItem>,
    pending_tasks: HashMap<u64, PendingHistoryOperation>,
}

impl FileOperationHistory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn take_undo_operation(
        &mut self,
    ) -> Option<(QueuedFileOperation, PendingHistoryOperation)> {
        let item = self.undo_stack.pop()?;
        let Some(operation) = item.undo_operation() else {
            self.undo_stack.push(item);
            return None;
        };
        Some((operation, PendingHistoryOperation::undo(item)))
    }

    pub(crate) fn take_redo_operation(
        &mut self,
    ) -> Option<(QueuedFileOperation, PendingHistoryOperation)> {
        let item = self.redo_stack.pop()?;
        let Some(operation) = item.redo_operation() else {
            self.redo_stack.push(item);
            return None;
        };
        Some((operation, PendingHistoryOperation::redo(item)))
    }

    pub(crate) fn reject_pending(&mut self, pending: PendingHistoryOperation) {
        self.restore_item(pending.direction, pending.item);
    }

    pub(crate) fn track_pending(&mut self, task_id: u64, operation: PendingHistoryOperation) {
        self.pending_tasks.insert(task_id, operation);
    }

    pub(crate) fn remap_pending_task(&mut self, local_task_id: u64, stored_task_id: u64) {
        if let Some(operation) = self.pending_tasks.remove(&local_task_id) {
            self.pending_tasks.insert(stored_task_id, operation);
        }
    }

    pub(crate) fn is_replaying(&self, task_id: u64) -> bool {
        self.pending_tasks.contains_key(&task_id)
    }

    pub(crate) fn accept_completed(&mut self, task_id: u64, outcome: &FileOperationOutcome) {
        if let Some(pending) = self.pending_tasks.remove(&task_id) {
            self.accept_completed_replay(pending, outcome);
            return;
        }

        if let Some(item) = FileOperationHistoryItem::from_outcome(outcome) {
            self.undo_stack.push(item);
        }
        self.redo_stack.clear();
    }

    /// 失败或取消：停下前已生效的部分和成功一样进撤销历史。
    pub(crate) fn accept_failed(&mut self, task_id: u64, partial_outcome: &FileOperationOutcome) {
        if self.pending_tasks.contains_key(&task_id) {
            self.accept_interrupted(task_id, partial_outcome);
        } else if let Some(item) = FileOperationHistoryItem::from_outcome(partial_outcome) {
            self.undo_stack.push(item);
            self.redo_stack.clear();
        }
    }

    /// 恢复中断的任务之后还会接着跑完，这时只处理撤销/重做重放，不记新历史。
    pub(crate) fn accept_interrupted(
        &mut self,
        task_id: u64,
        partial_outcome: &FileOperationOutcome,
    ) {
        let Some(pending) = self.pending_tasks.remove(&task_id) else {
            return;
        };
        let completed_move_transfers = match partial_outcome {
            FileOperationOutcome::Move { transfers, .. } => transfers.as_slice(),
            _ => &[],
        };

        let PendingHistoryOperation { direction, item } = pending;
        let FileOperationHistoryItem::Move { transfers } = item else {
            self.restore_item(direction, item);
            return;
        };
        if completed_move_transfers.is_empty() {
            self.restore_item(direction, FileOperationHistoryItem::Move { transfers });
            return;
        }

        let completed_original_transfers = match direction {
            HistoryDirection::Undo => completed_move_transfers
                .iter()
                .rev()
                .map(|transfer| CompletedTransfer {
                    source: transfer.target.clone(),
                    target: transfer.source.clone(),
                })
                .collect::<Vec<_>>(),
            HistoryDirection::Redo => completed_move_transfers.to_vec(),
        };
        let mut unmatched_completed_transfers = completed_original_transfers.clone();
        let remaining_original_transfers = transfers
            .into_iter()
            .filter(|transfer| {
                let Some(completed_index) = unmatched_completed_transfers
                    .iter()
                    .position(|completed_transfer| completed_transfer == transfer)
                else {
                    return true;
                };
                unmatched_completed_transfers.remove(completed_index);
                false
            })
            .collect::<Vec<_>>();
        debug_assert!(
            unmatched_completed_transfers.is_empty(),
            "completed history replay transfers must belong to the pending move"
        );

        match direction {
            HistoryDirection::Undo => {
                self.push_move_item(HistoryDirection::Undo, remaining_original_transfers);
                self.push_move_item(HistoryDirection::Redo, completed_original_transfers);
            }
            HistoryDirection::Redo => {
                self.push_move_item(HistoryDirection::Redo, remaining_original_transfers);
                self.push_move_item(HistoryDirection::Undo, completed_original_transfers);
            }
        }
    }

    fn restore_item(&mut self, direction: HistoryDirection, item: FileOperationHistoryItem) {
        match direction {
            HistoryDirection::Undo => self.undo_stack.push(item),
            HistoryDirection::Redo => self.redo_stack.push(item),
        }
    }

    fn push_move_item(&mut self, direction: HistoryDirection, transfers: Vec<CompletedTransfer>) {
        if !transfers.is_empty() {
            self.restore_item(direction, FileOperationHistoryItem::Move { transfers });
        }
    }

    fn accept_completed_replay(
        &mut self,
        pending: PendingHistoryOperation,
        outcome: &FileOperationOutcome,
    ) {
        match pending.direction {
            HistoryDirection::Undo => self.redo_stack.push(pending.item.after_undo(outcome)),
            HistoryDirection::Redo => {
                if let Some(item) = FileOperationHistoryItem::from_outcome(outcome) {
                    self.undo_stack.push(item);
                }
            }
        }
    }
}

impl PendingHistoryOperation {
    fn undo(item: FileOperationHistoryItem) -> Self {
        Self {
            direction: HistoryDirection::Undo,
            item,
        }
    }

    fn redo(item: FileOperationHistoryItem) -> Self {
        Self {
            direction: HistoryDirection::Redo,
            item,
        }
    }
}

impl FileOperationHistoryItem {
    fn from_outcome(outcome: &FileOperationOutcome) -> Option<Self> {
        match outcome {
            // 撤销收纳只在撤销时发生，它本身不再进历史。
            FileOperationOutcome::NoHistory | FileOperationOutcome::NewFolderUngathered { .. } => {
                None
            }
            // 转换生成新文件,不可通过删除新文件安全撤销(可能有同名列/外部引用),不写历史。
            FileOperationOutcome::Convert { .. } => None,
            FileOperationOutcome::Rename { from, to } => Some(Self::Rename {
                from: from.clone(),
                to: to.clone(),
            }),
            FileOperationOutcome::BatchRename { renames } if !renames.is_empty() => {
                Some(Self::BatchRename {
                    renames: renames.clone(),
                })
            }
            FileOperationOutcome::BatchRename { .. } => None,
            FileOperationOutcome::CreateDirectory { path } => {
                Some(Self::CreateDirectory { path: path.clone() })
            }
            FileOperationOutcome::CreateDirectories {
                parent,
                leaves,
                leaf_paths,
                created,
                moved,
            } if !created.is_empty() => Some(Self::CreateDirectories {
                parent: parent.clone(),
                leaves: leaves.clone(),
                leaf_paths: leaf_paths.clone(),
                created: created.clone(),
                moved: moved.clone(),
            }),
            FileOperationOutcome::CreateDirectories { .. } => None,
            FileOperationOutcome::CreateEmptyFile { path } => {
                Some(Self::CreateEmptyFile { path: path.clone() })
            }
            FileOperationOutcome::Trash { entries, .. } if !entries.is_empty() => {
                Some(Self::Trash {
                    original_paths: entries
                        .iter()
                        .map(|entry| entry.original_path.clone())
                        .collect(),
                    restore_entries: entries.clone(),
                })
            }
            FileOperationOutcome::Trash { .. } => None,
            FileOperationOutcome::Restore {
                entries,
                restored_paths,
            } if !restored_paths.is_empty() => Some(Self::Restore {
                restore_entries: entries.clone(),
                restored_paths: restored_paths.clone(),
            }),
            FileOperationOutcome::Restore { .. } => None,
            FileOperationOutcome::Copy {
                transfers,
                history_eligibility: FileOperationHistoryEligibility::Replayable,
                ..
            } if !transfers.is_empty() => Some(Self::Copy {
                transfers: transfers.clone(),
            }),
            FileOperationOutcome::Copy { .. } => None,
            FileOperationOutcome::Move {
                transfers,
                history_eligibility: FileOperationHistoryEligibility::Replayable,
                ..
            } if !transfers.is_empty() => Some(Self::Move {
                transfers: transfers.clone(),
            }),
            FileOperationOutcome::Move { .. } => None,
            FileOperationOutcome::GatheredIntoNewFolder { directory, moved }
                if !moved.is_empty() =>
            {
                Some(Self::GatheredIntoNewFolder {
                    directory: directory.clone(),
                    moved: moved.clone(),
                })
            }
            FileOperationOutcome::GatheredIntoNewFolder { .. } => None,
        }
    }

    fn undo_operation(&self) -> Option<QueuedFileOperation> {
        match self {
            Self::Rename { from, to } => rename_operation(to.clone(), from),
            Self::BatchRename { renames } => Some(QueuedFileOperation::BatchRename {
                items: reverse_batch_rename_items(renames),
            }),
            Self::CreateDirectory { path } | Self::CreateEmptyFile { path } => {
                Some(QueuedFileOperation::Trash {
                    paths: vec![path.clone()],
                })
            }
            // 移入过条目：与撤销收纳一致，条目挪回原位、新建的空目录逆序删除。
            Self::CreateDirectories {
                leaf_paths,
                created,
                moved,
                ..
            } if !moved.is_empty() => {
                leaf_paths
                    .first()
                    .map(|directory| QueuedFileOperation::UngatherNewFolder {
                        directory: directory.clone(),
                        restore_targets: transfer_sources(moved),
                        created_directories: created.clone(),
                    })
            }
            // 没移入条目：与单建同语义，本次新建的顶层目录整批进回收站
            //（嵌套链只回收顶层，复用的已有目录不受影响）。
            Self::CreateDirectories { created, .. } => Some(QueuedFileOperation::Trash {
                paths: top_level_directories(created),
            }),
            Self::Trash {
                restore_entries, ..
            } if !restore_entries.is_empty() => Some(QueuedFileOperation::Restore {
                entries: restore_entries.clone(),
            }),
            Self::Trash { .. } => None,
            Self::Restore { restored_paths, .. } => Some(QueuedFileOperation::Trash {
                paths: restored_paths.clone(),
            }),
            Self::Copy { transfers } => Some(QueuedFileOperation::Trash {
                paths: transfer_targets(transfers),
            }),
            Self::Move { transfers } => Some(QueuedFileOperation::Move {
                transfers: reverse_transfers(transfers),
                verification: FileOperationVerification::default(),
            }),
            Self::GatheredIntoNewFolder { directory, moved } => {
                Some(QueuedFileOperation::UngatherNewFolder {
                    directory: directory.clone(),
                    restore_targets: transfer_sources(moved),
                    created_directories: vec![directory.clone()],
                })
            }
        }
    }

    fn redo_operation(&self) -> Option<QueuedFileOperation> {
        match self {
            Self::Rename { from, to } => rename_operation(from.clone(), to),
            Self::BatchRename { renames } => Some(QueuedFileOperation::BatchRename {
                items: forward_batch_rename_items(renames),
            }),
            Self::CreateDirectory { path } => create_directory_operation(path),
            Self::CreateEmptyFile { path } => create_empty_file_operation(path),
            // 移入过条目后源路径已变，重做不可用；纯创建时按原叶子链重建。
            Self::CreateDirectories {
                parent,
                leaves,
                moved,
                ..
            } if moved.is_empty() => Some(QueuedFileOperation::CreateDirectories {
                parent: parent.clone(),
                names: leaves.clone(),
                gather_sources: Vec::new(),
            }),
            Self::CreateDirectories { .. } => None,
            Self::Trash { original_paths, .. } => Some(QueuedFileOperation::Trash {
                paths: original_paths.clone(),
            }),
            Self::Restore {
                restore_entries, ..
            } if !restore_entries.is_empty() => Some(QueuedFileOperation::Restore {
                entries: restore_entries.clone(),
            }),
            Self::Restore { .. } => None,
            Self::Copy { transfers } => Some(QueuedFileOperation::Copy {
                transfers: forward_transfers(transfers),
                verification: FileOperationVerification::default(),
            }),
            Self::Move { transfers } => Some(QueuedFileOperation::Move {
                transfers: forward_transfers(transfers),
                verification: FileOperationVerification::default(),
            }),
            // 重做沿用原目录名;若名字在撤销期间被占,执行器会按同一规则重选。
            Self::GatheredIntoNewFolder { directory, moved } => {
                Some(QueuedFileOperation::GatherSelectionIntoNewFolder {
                    directory: directory.clone(),
                    sources: transfer_sources(moved),
                })
            }
        }
    }

    fn after_undo(self, outcome: &FileOperationOutcome) -> Self {
        match (self, outcome) {
            (Self::Trash { .. }, FileOperationOutcome::Restore { restored_paths, .. }) => {
                Self::Trash {
                    original_paths: restored_paths.clone(),
                    restore_entries: Vec::new(),
                }
            }
            (Self::Restore { restored_paths, .. }, FileOperationOutcome::Trash { entries, .. }) => {
                Self::Restore {
                    restore_entries: entries.clone(),
                    restored_paths,
                }
            }
            (item, _) => item,
        }
    }
}

fn rename_operation(path: PathBuf, target: &Path) -> Option<QueuedFileOperation> {
    target.file_name().map(|name| QueuedFileOperation::Rename {
        path,
        new_name: name.to_os_string(),
    })
}

fn create_directory_operation(path: &Path) -> Option<QueuedFileOperation> {
    path.parent()
        .map(Path::to_path_buf)
        .map(|parent| QueuedFileOperation::CreateDirectory { parent })
}

/// 从成果目录列表反推批量重建操作：名字取各目录末段，父目录取首项父目录。
// 原反推函数已删:嵌套链无法从目录列表反推,redo 直接重放原叶子路径
// (见 redo_operation 的 CreateDirectories 分支)。

fn create_empty_file_operation(path: &Path) -> Option<QueuedFileOperation> {
    path.parent()
        .map(Path::to_path_buf)
        .map(|parent| QueuedFileOperation::CreateEmptyFile { parent })
}

fn forward_transfers(transfers: &[CompletedTransfer]) -> Vec<QueuedTransfer> {
    transfers
        .iter()
        .map(|transfer| QueuedTransfer {
            source: transfer.source.clone(),
            target: transfer.target.clone(),
            conflict_strategy: TransferConflictStrategy::Fail,
        })
        .collect()
}

fn reverse_transfers(transfers: &[CompletedTransfer]) -> Vec<QueuedTransfer> {
    transfers
        .iter()
        .rev()
        .map(|transfer| QueuedTransfer {
            source: transfer.target.clone(),
            target: transfer.source.clone(),
            conflict_strategy: TransferConflictStrategy::Fail,
        })
        .collect()
}

fn forward_batch_rename_items(renames: &[CompletedBatchRename]) -> Vec<BatchRenameItem> {
    renames
        .iter()
        .map(|rename| BatchRenameItem {
            from: rename.from.clone(),
            to: rename.to.clone(),
        })
        .collect()
}

fn reverse_batch_rename_items(renames: &[CompletedBatchRename]) -> Vec<BatchRenameItem> {
    renames
        .iter()
        .rev()
        .map(|rename| BatchRenameItem {
            from: rename.to.clone(),
            to: rename.from.clone(),
        })
        .collect()
}

fn transfer_targets(transfers: &[CompletedTransfer]) -> Vec<PathBuf> {
    transfers
        .iter()
        .rev()
        .map(|transfer| transfer.target.clone())
        .collect()
}

fn transfer_sources(transfers: &[CompletedTransfer]) -> Vec<PathBuf> {
    transfers
        .iter()
        .map(|transfer| transfer.source.clone())
        .collect()
}

/// 新建目录里去掉是另一个新建目录后代的项：顶层目录整棵进回收站，不留下空壳中间层。
fn top_level_directories(created: &[PathBuf]) -> Vec<PathBuf> {
    created
        .iter()
        .filter(|path| {
            !created
                .iter()
                .any(|other| *other != **path && path.strip_prefix(other).is_ok())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "operation_history_tests.rs"]
mod tests;
