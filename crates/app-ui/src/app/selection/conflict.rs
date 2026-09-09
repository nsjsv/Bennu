use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use file_core::TransferConflictStrategy;
use iced::Task;

use crate::app::FileBrowser;
use crate::commands::{
    check_transfer_conflicts_command, expand_transfer_conflict_merges_command,
};
use crate::model::{
    entry_exists, unique_duplicated_directory_name, unique_duplicated_file_name, Message,
    TransferConflictChoice, TransferConflictItem, TransferConflictMode, TransferConflictState,
};
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

impl FileBrowser {
    pub(in crate::app) fn enqueue_or_confirm_transfers(
        &mut self,
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
    ) -> Task<Message> {
        check_transfer_conflicts_command(mode, transfers)
    }

    pub(in crate::app) fn accept_transfer_conflicts_checked(
        &mut self,
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
        conflicts: Vec<TransferConflictItem>,
    ) -> Task<Message> {
        if conflicts.is_empty() {
            return self.enqueue_transfer_operation(mode, transfers);
        }

        self.clear_global_error();
        self.context_menu = None;
        self.operation_queue.close_panel();
        self.transfer_conflict = Some(TransferConflictState {
            mode,
            transfers,
            conflicts,
            current_index: 0,
            apply_to_all: false,
        });
        self.schedule_transfer_conflict_thumbnails()
    }

    fn enqueue_transfer_operation(
        &mut self,
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
    ) -> Task<Message> {
        match mode {
            TransferConflictMode::Copy => self.enqueue_file_operation(QueuedFileOperation::Copy {
                transfers,
                verification: self.file_operation_verification(),
            }),
            TransferConflictMode::Move => self.enqueue_file_operation(QueuedFileOperation::Move {
                transfers,
                verification: self.file_operation_verification(),
            }),
        }
    }

    pub(in crate::app) fn resolve_transfer_conflict_choice(
        &mut self,
        choice: TransferConflictChoice,
    ) -> Task<Message> {
        let Some(mut state) = self.transfer_conflict.take() else {
            return Task::none();
        };

        match choice {
            // 「合并」改写传输集本身,交给展开命令复查;「保留两者」改写冲突
            // 传输的目标名。两者都不走按策略标记的旧路径。
            TransferConflictChoice::Merge => {
                return self.expand_transfer_conflict_merges(state);
            }
            TransferConflictChoice::KeepBoth => apply_keep_both_choice(&mut state),
            TransferConflictChoice::Replace
            | TransferConflictChoice::Skip
            | TransferConflictChoice::Rename => {
                let apply_to_all = state.apply_to_all;
                loop {
                    if state.current_conflict().is_none() {
                        break;
                    };
                    apply_conflict_choice(&mut state, choice);
                    state.current_index += 1;
                    if !apply_to_all {
                        break;
                    }
                }
            }
        }

        self.finish_or_continue_transfer_conflicts(state)
    }

    /// 「合并」:把可合并的目录冲突从传输集中摘出、展开成子项传输,
    /// 再回到统一的冲突复查;子项同名会作为新冲突重新进对话框,
    /// 嵌套同名目录由此逐层推进。
    fn expand_transfer_conflict_merges(&mut self, state: TransferConflictState) -> Task<Message> {
        let merge_pairs = mergeable_conflict_pairs(&state);
        if merge_pairs.is_empty() {
            self.transfer_conflict = Some(state);
            return Task::none();
        }
        let remaining_transfers = state
            .transfers
            .iter()
            .filter(|transfer| {
                !merge_pairs
                    .iter()
                    .any(|(source, target)| {
                        transfer.source == *source && transfer.target == *target
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let remaining_conflicts = state
            .conflicts
            .iter()
            .filter(|conflict| {
                !merge_pairs.iter().any(|(source, target)| {
                    conflict.source == *source && conflict.target == *target
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        expand_transfer_conflict_merges_command(
            state.mode,
            merge_pairs,
            remaining_transfers,
            remaining_conflicts,
        )
    }

    pub(in crate::app) fn accept_expanded_transfer_conflict_merges(
        &mut self,
        mode: TransferConflictMode,
        expansion: Result<(Vec<QueuedTransfer>, Vec<TransferConflictItem>), String>,
    ) -> Task<Message> {
        match expansion {
            Ok((transfers, conflicts)) => {
                self.accept_transfer_conflicts_checked(mode, transfers, conflicts)
            }
            Err(error) => {
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    pub(in crate::app) fn apply_transfer_conflict_message(
        &mut self,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::TransferConflictChoiceSelected(choice) => {
                self.resolve_transfer_conflict_choice(choice)
            }
            Message::TransferConflictApplyToAllToggled => {
                self.toggle_transfer_conflict_apply_to_all();
                Task::none()
            }
            Message::TransferConflictCancelRequested => {
                self.transfer_conflict = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub(in crate::app) fn toggle_transfer_conflict_apply_to_all(&mut self) {
        if let Some(state) = &mut self.transfer_conflict {
            state.apply_to_all = !state.apply_to_all;
        }
    }

    fn finish_or_continue_transfer_conflicts(
        &mut self,
        state: TransferConflictState,
    ) -> Task<Message> {
        if state.current_index >= state.conflicts.len() {
            self.transfer_conflict = None;
            return self.enqueue_transfer_operation(state.mode, state.transfers);
        }

        self.transfer_conflict = Some(state);
        self.schedule_transfer_conflict_thumbnails()
    }
}

/// 待展开的合并对:单次确认只并当前冲突;勾选应用到全部时并掉所有可合并冲突
/// (不可合并的条目留在清单里继续走冲突流程)。
fn mergeable_conflict_pairs(state: &TransferConflictState) -> Vec<(PathBuf, PathBuf)> {
    let conflicts = if state.apply_to_all {
        state.conflicts.as_slice()
    } else {
        match state.current_conflict() {
            Some(conflict) => std::slice::from_ref(conflict),
            None => &[],
        }
    };
    conflicts
        .iter()
        .filter(|conflict| conflict.can_merge())
        .map(|conflict| (conflict.source.clone(), conflict.target.clone()))
        .collect()
}

fn apply_conflict_choice(state: &mut TransferConflictState, choice: TransferConflictChoice) {
    let Some(conflict) = state.current_conflict().cloned() else {
        return;
    };
    let Some(position) = conflict_transfer_position(state, &conflict) else {
        return;
    };

    let strategy = match choice {
        TransferConflictChoice::Replace => TransferConflictStrategy::Replace,
        TransferConflictChoice::Skip => TransferConflictStrategy::Skip,
        TransferConflictChoice::Rename => TransferConflictStrategy::KeepBoth,
        // 这两个选择有专用解析路径,不会走到按策略标记的旧路径。
        TransferConflictChoice::KeepBoth | TransferConflictChoice::Merge => return,
    };
    state.transfers[position].conflict_strategy = strategy;
}

/// 「保留两者」:不改策略,直接把冲突传输的目标改成共享命名规则起出的新名;
/// 批内其余传输目标与磁盘占用都算占用,保证一次应用全部时互不撞名。
fn apply_keep_both_choice(state: &mut TransferConflictState) {
    let apply_to_all = state.apply_to_all;
    let mut reserved: HashSet<PathBuf> = state
        .transfers
        .iter()
        .map(|transfer| transfer.target.clone())
        .collect();
    loop {
        let Some(conflict) = state.current_conflict().cloned() else {
            break;
        };
        let Some(position) = conflict_transfer_position(state, &conflict) else {
            break;
        };
        let parent = conflict
            .target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let target = keep_both_transfer_target(&conflict, |candidate| {
            entry_exists(&parent.join(candidate)) || reserved.contains(&parent.join(candidate))
        });
        reserved.insert(target.clone());
        state.transfers[position].target = target;
        state.current_index += 1;
        if !apply_to_all {
            break;
        }
    }
}

/// 「保留两者」的命名:源是目录就整体追加「副本」,否则插在扩展名前;
/// 占用判定由调用方注入(磁盘 + 批内保留),纯函数便于锁定命名规则。
fn keep_both_transfer_target(
    conflict: &TransferConflictItem,
    is_taken: impl FnMut(&OsStr) -> bool,
) -> PathBuf {
    let target = &conflict.target;
    let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = target.file_name().unwrap_or_else(|| OsStr::new("item"));
    let unique = if conflict.source_metadata.is_directory {
        unique_duplicated_directory_name(name, is_taken)
    } else {
        unique_duplicated_file_name(name, is_taken)
    };
    parent.join(unique)
}

fn conflict_transfer_position(
    state: &TransferConflictState,
    conflict: &TransferConflictItem,
) -> Option<usize> {
    state.transfers.iter().position(|transfer| {
        transfer.source == conflict.source && transfer.target == conflict.target
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{TransferConflictMetadata, TransferConflictStrategy};

    use super::*;

    fn file_conflict(source: &str, target: &str) -> TransferConflictItem {
        TransferConflictItem {
            source: PathBuf::from(source),
            target: PathBuf::from(target),
            source_metadata: TransferConflictMetadata {
                is_directory: false,
                len: 3,
                modified: None,
            },
            target_metadata: TransferConflictMetadata {
                is_directory: false,
                len: 3,
                modified: None,
            },
        }
    }

    fn test_conflict_state() -> TransferConflictState {
        let source = PathBuf::from("/source/report.txt");
        let target = PathBuf::from("/target/report.txt");

        TransferConflictState {
            mode: TransferConflictMode::Copy,
            transfers: vec![QueuedTransfer::new(source.clone(), target.clone())],
            conflicts: vec![file_conflict(
                "/source/report.txt",
                "/target/report.txt",
            )],
            current_index: 0,
            apply_to_all: false,
        }
    }

    #[test]
    fn transfer_conflict_rename_uses_keep_both_strategy() {
        let mut state = test_conflict_state();

        apply_conflict_choice(&mut state, TransferConflictChoice::Rename);

        assert_eq!(
            state.transfers[0].conflict_strategy,
            TransferConflictStrategy::KeepBoth
        );
    }

    #[test]
    fn keep_both_renames_file_target_with_duplicate_suffix() {
        let conflict = file_conflict("/source/report.txt", "/target/report.txt");
        let taken = |candidate: &OsStr| candidate == OsStr::new("report副本.txt");

        let target = keep_both_transfer_target(&conflict, taken);

        assert_eq!(target, PathBuf::from("/target/report副本 2.txt"));
    }

    #[test]
    fn keep_both_renames_directory_target_by_tail_suffix() {
        let conflict = TransferConflictItem {
            source: PathBuf::from("/source/photos"),
            target: PathBuf::from("/target/photos"),
            source_metadata: TransferConflictMetadata {
                is_directory: true,
                len: 3,
                modified: None,
            },
            target_metadata: TransferConflictMetadata {
                is_directory: true,
                len: 3,
                modified: None,
            },
        };

        let target = keep_both_transfer_target(&conflict, |_| false);

        assert_eq!(target, PathBuf::from("/target/photos副本"));
    }

    #[test]
    fn keep_both_choice_rewrites_conflicting_transfer_target_only() {
        let mut state = test_conflict_state();
        let untouched = QueuedTransfer::new(
            PathBuf::from("/source/other.txt"),
            PathBuf::from("/target/other.txt"),
        );
        state.transfers.push(untouched);

        apply_keep_both_choice(&mut state);

        assert_eq!(state.transfers[0].target, PathBuf::from("/target/report副本.txt"));
        assert_eq!(state.transfers[1].target, PathBuf::from("/target/other.txt"));
    }

    #[test]
    fn merge_pairs_follow_apply_to_all_scope() {
        let mut state = test_conflict_state();
        let directory_conflict = TransferConflictItem {
            source: PathBuf::from("/source/archive"),
            target: PathBuf::from("/target/archive"),
            source_metadata: TransferConflictMetadata {
                is_directory: true,
                len: 3,
                modified: None,
            },
            target_metadata: TransferConflictMetadata {
                is_directory: true,
                len: 3,
                modified: None,
            },
        };
        state.conflicts.push(directory_conflict);

        // 未勾选应用全部:只并当前冲突(文件冲突不可合并,返回空)。
        assert!(mergeable_conflict_pairs(&state).is_empty());
        state.current_index = 1;
        assert_eq!(
            mergeable_conflict_pairs(&state),
            vec![(
                PathBuf::from("/source/archive"),
                PathBuf::from("/target/archive")
            )]
        );

        // 勾选应用全部:可合并的目录冲突被选中,文件冲突跳过。
        state.apply_to_all = true;
        state.current_index = 0;
        assert_eq!(
            mergeable_conflict_pairs(&state),
            vec![(
                PathBuf::from("/source/archive"),
                PathBuf::from("/target/archive")
            )]
        );
    }
}
