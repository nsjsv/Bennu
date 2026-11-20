//! 撤销 / 重做历史。一条记录可以有多项：一次普通操作是一项，一个动作是它做过的各项。
//! 重放一条记录时一次只往队列里放一项，这一项结束了再放下一项；中途停下时，做完的和没做的
//! 各成一条，再按一次接着做剩下的。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use file_core::{
    BatchRenameItem, CompletedBatchRename, FileOperationVerification, TransferConflictStrategy,
    TrashRestoreEntry,
};
use file_operations::{
    CompletedTransfer, FileOperationEnqueueOutcome, FileOperationHistoryEligibility,
    FileOperationOutcome, QueuedFileOperation, QueuedTransfer,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryDirection {
    Undo,
    Redo,
}

/// 一个队列任务怎么结束的，带着它已经生效的部分。
#[derive(Debug, Clone, Copy)]
pub(crate) enum TaskEnd<'a> {
    Completed(&'a FileOperationOutcome),
    /// 失败、取消，或者还没开始就被拒、被取消。
    Stopped(&'a FileOperationOutcome),
    /// 恢复中断：之后还会接着恢复，现在只交代重放。
    Interrupted(&'a FileOperationOutcome),
}

/// 把一个操作放进任务队列。历史只通过它入队，拿到的任务 id 用来认它的结束。
pub(crate) type EnqueueOperation<'a> =
    dyn FnMut(QueuedFileOperation) -> FileOperationEnqueueOutcome + 'a;

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
    /// 这次操作放出来的条目：从回收站还原的，或者转换、压缩、解压新生成的。撤销 = 移到回收站
    /// （同 Nautilus 撤销压缩、解压），重做 = 从回收站拿回来。`trash_entries` 是撤销时它们在
    /// 回收站里的位置，还没撤销过时为空。
    Appeared {
        paths: Vec<PathBuf>,
        trash_entries: Vec<TrashRestoreEntry>,
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

/// 一条撤销记录，项按执行的顺序排。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryEntry {
    items: Vec<FileOperationHistoryItem>,
}

#[derive(Debug, Default)]
pub(crate) struct FileOperationHistory {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    replay: Option<HistoryReplay>,
    /// 重放进行中又按的撤销 / 重做，前一条结束后按顺序开始。
    requested: VecDeque<HistoryDirection>,
}

/// 正在重放的一条记录。
#[derive(Debug)]
struct HistoryReplay {
    direction: HistoryDirection,
    /// 还没放进队列的项，下一项在末尾：撤销从最后一步倒着来，重做从第一步来。
    remaining: Vec<FileOperationHistoryItem>,
    /// 做完的项变成的反方向的项，按做完的顺序。
    finished: Vec<FileOperationHistoryItem>,
    /// 放进队列、正在等结果的那一项和它的任务。
    running: Option<(u64, FileOperationHistoryItem)>,
}

impl FileOperationHistory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 普通操作（不是重放）结束：做成的部分记成一条。做完的操作即使没有可撤销的东西，
    /// 也让重做作废，因为文件已经变了；什么都没做成的停下不动历史。
    pub(crate) fn record(&mut self, end: TaskEnd<'_>) {
        match end {
            TaskEnd::Completed(outcome) => {
                if let Some(item) = FileOperationHistoryItem::from_outcome(outcome) {
                    self.undo_stack.push(HistoryEntry { items: vec![item] });
                }
                self.redo_stack.clear();
            }
            TaskEnd::Stopped(partial_outcome) => {
                if let Some(item) = FileOperationHistoryItem::from_outcome(partial_outcome) {
                    self.undo_stack.push(HistoryEntry { items: vec![item] });
                    self.redo_stack.clear();
                }
            }
            // 恢复完会再以完成或停下结束一次，那时再记。
            TaskEnd::Interrupted(_) => {}
        }
    }

    pub(crate) fn is_replaying(&self, task_id: u64) -> bool {
        self.running_task_id() == Some(task_id)
    }

    /// Ctrl+Z / Ctrl+Shift+Z。有重放在跑时排队，等它结束再做。返回入队被拒的原因。
    pub(crate) fn request(
        &mut self,
        direction: HistoryDirection,
        enqueue: &mut EnqueueOperation<'_>,
    ) -> Option<String> {
        if self.replay.is_some() {
            self.requested.push_back(direction);
            return None;
        }
        self.start_replay(direction, enqueue)
    }

    /// 重放的那一项结束了：做完就放下一项，停下就把这条拆开放回去。返回入队被拒的原因。
    pub(crate) fn accept_replay_end(
        &mut self,
        task_id: u64,
        end: TaskEnd<'_>,
        enqueue: &mut EnqueueOperation<'_>,
    ) -> Option<String> {
        let replay = self.replay.as_mut()?;
        let (_, item) = replay
            .running
            .take_if(|(running_task_id, _)| *running_task_id == task_id)?;
        match end {
            TaskEnd::Completed(outcome) => {
                let opposite = match replay.direction {
                    HistoryDirection::Undo => Some(item.after_undo(outcome)),
                    HistoryDirection::Redo => FileOperationHistoryItem::from_outcome(outcome),
                };
                replay.finished.extend(opposite);
                self.advance_replay(enqueue)
            }
            TaskEnd::Stopped(partial_outcome) | TaskEnd::Interrupted(partial_outcome) => {
                let (unfinished, finished) =
                    split_stopped_replay(replay.direction, item, partial_outcome);
                replay.finished.extend(finished);
                replay.remaining.extend(unfinished);
                self.stop_replay();
                None
            }
        }
    }

    fn running_task_id(&self) -> Option<u64> {
        self.replay
            .as_ref()
            .and_then(|replay| replay.running.as_ref())
            .map(|(task_id, _)| *task_id)
    }

    fn start_replay(
        &mut self,
        direction: HistoryDirection,
        enqueue: &mut EnqueueOperation<'_>,
    ) -> Option<String> {
        let stack = match direction {
            HistoryDirection::Undo => &mut self.undo_stack,
            HistoryDirection::Redo => &mut self.redo_stack,
        };
        let HistoryEntry { mut items } = stack.pop()?;
        if direction == HistoryDirection::Redo {
            items.reverse();
        }
        self.replay = Some(HistoryReplay {
            direction,
            remaining: items,
            finished: Vec::new(),
            running: None,
        });
        self.advance_replay(enqueue)
    }

    /// 放下一项进队列。这条做完了就收尾，再开始排着的下一次请求。
    fn advance_replay(&mut self, enqueue: &mut EnqueueOperation<'_>) -> Option<String> {
        let replay = self.replay.as_mut()?;
        let Some(item) = replay.remaining.pop() else {
            self.finish_replay();
            let direction = self.requested.pop_front()?;
            return self.start_replay(direction, enqueue);
        };
        let operation = match replay.direction {
            HistoryDirection::Undo => item.undo_operation(),
            HistoryDirection::Redo => item.redo_operation(),
        };
        // 这一项做不了（比如重做时源路径已经变了）：停在这里，整条留给用户。
        let Some(operation) = operation else {
            replay.remaining.push(item);
            self.stop_replay();
            return None;
        };
        match enqueue(operation) {
            FileOperationEnqueueOutcome::Queued { task_id } => {
                replay.running = Some((task_id, item));
                None
            }
            FileOperationEnqueueOutcome::Rejected { error } => {
                replay.remaining.push(item);
                self.stop_replay();
                Some(error)
            }
        }
    }

    /// 整条都做完：做完的项成为另一边的一条。
    fn finish_replay(&mut self) {
        if let Some(replay) = self.replay.take() {
            self.push_finished(replay.direction, replay.finished);
        }
    }

    /// 停下：做完的成为另一边的一条，没做的按原来的顺序放回原来的栈。之后排着的请求作废，
    /// 它们是冲着这一串连着做的，停下以后再接着做会撤到用户没想到的地方。
    fn stop_replay(&mut self) {
        self.requested.clear();
        let Some(replay) = self.replay.take() else {
            return;
        };
        let mut unfinished = replay.remaining;
        match replay.direction {
            HistoryDirection::Undo => push_entry(&mut self.undo_stack, unfinished),
            HistoryDirection::Redo => {
                unfinished.reverse();
                push_entry(&mut self.redo_stack, unfinished);
            }
        }
        self.push_finished(replay.direction, replay.finished);
    }

    fn push_finished(
        &mut self,
        direction: HistoryDirection,
        mut finished: Vec<FileOperationHistoryItem>,
    ) {
        match direction {
            HistoryDirection::Undo => {
                finished.reverse();
                push_entry(&mut self.redo_stack, finished);
            }
            HistoryDirection::Redo => push_entry(&mut self.undo_stack, finished),
        }
    }
}

#[cfg(test)]
impl FileOperationHistory {
    /// 测试用：撤销栈顶那条时第一个要放进队列的操作，不动历史。
    pub(crate) fn next_undo_operation(&self) -> Option<QueuedFileOperation> {
        self.undo_stack.last()?.items.last()?.undo_operation()
    }

    /// 测试用：重做栈顶那条时第一个要放进队列的操作，不动历史。
    pub(crate) fn next_redo_operation(&self) -> Option<QueuedFileOperation> {
        self.redo_stack.last()?.items.first()?.redo_operation()
    }

    pub(crate) fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }
}

fn push_entry(stack: &mut Vec<HistoryEntry>, items: Vec<FileOperationHistoryItem>) {
    if !items.is_empty() {
        stack.push(HistoryEntry { items });
    }
}

/// 重放的一项停下了：(没做完、留在原来那边的部分, 做完了、去另一边的部分)。移动可能只挪回了
/// 一部分，按实际挪过的拆成两段；其余的项整项留在原来那边。
fn split_stopped_replay(
    direction: HistoryDirection,
    item: FileOperationHistoryItem,
    partial_outcome: &FileOperationOutcome,
) -> (
    Option<FileOperationHistoryItem>,
    Option<FileOperationHistoryItem>,
) {
    let completed_move_transfers = match partial_outcome {
        FileOperationOutcome::Move { transfers, .. } => transfers.as_slice(),
        _ => &[],
    };
    let FileOperationHistoryItem::Move { transfers } = item else {
        return (Some(item), None);
    };
    if completed_move_transfers.is_empty() {
        return (Some(FileOperationHistoryItem::Move { transfers }), None);
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
        "completed history replay transfers must belong to the replayed move"
    );
    let as_item = |transfers: Vec<CompletedTransfer>| {
        (!transfers.is_empty()).then_some(FileOperationHistoryItem::Move { transfers })
    };
    (
        as_item(remaining_original_transfers),
        as_item(completed_original_transfers),
    )
}

impl FileOperationHistoryItem {
    fn appeared(paths: Vec<PathBuf>) -> Option<Self> {
        (!paths.is_empty()).then_some(Self::Appeared {
            paths,
            trash_entries: Vec::new(),
        })
    }

    fn from_outcome(outcome: &FileOperationOutcome) -> Option<Self> {
        match outcome {
            // 撤销收纳只在撤销时发生，它本身不再进历史。
            FileOperationOutcome::NoHistory | FileOperationOutcome::NewFolderUngathered { .. } => {
                None
            }
            FileOperationOutcome::Convert { outputs, .. } => Self::appeared(outputs.clone()),
            FileOperationOutcome::CreatedArchive { archive } => {
                Self::appeared(vec![archive.clone()])
            }
            FileOperationOutcome::Extracted { placed } => Self::appeared(vec![placed.clone()]),
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
            FileOperationOutcome::Restore { restored_paths, .. } => {
                Self::appeared(restored_paths.clone())
            }
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
            Self::Appeared { paths, .. } => Some(QueuedFileOperation::Trash {
                paths: paths.clone(),
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
            Self::Appeared { trash_entries, .. } if !trash_entries.is_empty() => {
                Some(QueuedFileOperation::Restore {
                    entries: trash_entries.clone(),
                })
            }
            Self::Appeared { .. } => None,
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
            (Self::Appeared { paths, .. }, FileOperationOutcome::Trash { entries, .. }) => {
                Self::Appeared {
                    paths,
                    trash_entries: entries.clone(),
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
#[path = "operation_history_replay_tests.rs"]
mod replay_tests;
#[cfg(test)]
#[path = "operation_history_tests.rs"]
mod tests;
