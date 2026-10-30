use std::path::PathBuf;

use file_core::TransferConflictStrategy;
use iced::Task;

use crate::app::FileBrowser;
use crate::commands::{check_transfer_conflicts_command, expand_transfer_conflict_merges_command};
use crate::model::{
    Message, TransferConflictChoice, TransferConflictItem, TransferConflictMode,
    TransferConflictState,
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

    /// 首次冲突检查与「合并」展开后的复查共用：检查失败（读不到某项的信息）时报错，整批不入队。
    pub(in crate::app) fn accept_transfer_conflict_review(
        &mut self,
        mode: TransferConflictMode,
        review: Result<(Vec<QueuedTransfer>, Vec<TransferConflictItem>), String>,
    ) -> Task<Message> {
        match review {
            Ok((transfers, conflicts)) => {
                self.review_transfer_conflicts(mode, transfers, conflicts)
            }
            Err(error) => {
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    fn review_transfer_conflicts(
        &mut self,
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
        conflicts: Vec<TransferConflictItem>,
    ) -> Task<Message> {
        if conflicts.is_empty() {
            return self.enqueue_transfer_operation(mode, transfers);
        }

        self.dismiss_global_error();
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

        let strategy = match choice {
            // 「合并」改写传输集本身,交给展开命令复查,不按策略标记。
            TransferConflictChoice::Merge => {
                return self.expand_transfer_conflict_merges(state);
            }
            TransferConflictChoice::Replace => TransferConflictStrategy::Replace,
            TransferConflictChoice::Skip => TransferConflictStrategy::Skip,
            TransferConflictChoice::KeepBoth => TransferConflictStrategy::KeepBoth,
        };
        let apply_to_all = state.apply_to_all;
        while let Some(conflict) = state.current_conflict().cloned() {
            mark_conflict_strategy(&mut state, &conflict, strategy);
            state.current_index += 1;
            if !apply_to_all {
                break;
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
                !merge_pairs.iter().any(|(source, target)| {
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

/// 冲突的处理方式记在对应传输上,由引擎提交时执行(保留两者在提交时原子续编号,
/// 批内同名、磁盘新出现的同名都由那一次 no-replace 判定兜住)。
fn mark_conflict_strategy(
    state: &mut TransferConflictState,
    conflict: &TransferConflictItem,
    strategy: TransferConflictStrategy,
) {
    if let Some(position) = conflict_transfer_position(state, conflict) {
        state.transfers[position].conflict_strategy = strategy;
    }
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
            conflicts: vec![file_conflict("/source/report.txt", "/target/report.txt")],
            current_index: 0,
            apply_to_all: false,
        }
    }

    /// 带任务存储的浏览器（复制走可恢复管线），冲突对话框停在 `state` 上。
    fn browser_resolving(state: TransferConflictState) -> (FileBrowser, tempfile::TempDir) {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let state_directory = tempfile::tempdir().unwrap();
        crate::operation_queue::test_persistence::attach_store(
            &mut browser.operation_queue,
            file_operation_store::TaskQueueStore::new(state_directory.path().join("state.sqlite"))
                .unwrap(),
        );
        browser.transfer_conflict = Some(state);
        (browser, state_directory)
    }

    fn queued_copy_transfers(browser: &FileBrowser) -> Vec<QueuedTransfer> {
        match &browser.operation_queue.tasks()[0].operation {
            crate::operation_queue::QueuedFileOperation::Copy { transfers, .. } => {
                transfers.clone()
            }
            other => panic!("expected a copy, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_conflict_check_shows_the_error_and_queues_nothing() {
        let (mut browser, _state) = browser_resolving(test_conflict_state());
        browser.transfer_conflict = None;

        drop(browser.accept_transfer_conflict_review(
            TransferConflictMode::Copy,
            Err("could not read /target/report.txt: permission denied".to_owned()),
        ));

        assert!(browser.operation_queue.tasks().is_empty());
        assert!(browser.transfer_conflict.is_none());
        assert!(browser.global_error_notification.is_some());
    }

    #[test]
    fn keep_both_leaves_the_target_for_the_engine_to_number() {
        let (mut browser, _state) = browser_resolving(test_conflict_state());

        drop(browser.resolve_transfer_conflict_choice(TransferConflictChoice::KeepBoth));

        assert!(browser.transfer_conflict.is_none());
        let transfers = queued_copy_transfers(&browser);
        assert_eq!(transfers[0].target, PathBuf::from("/target/report.txt"));
        assert_eq!(
            transfers[0].conflict_strategy,
            TransferConflictStrategy::KeepBoth
        );
    }

    #[test]
    fn keep_both_for_all_marks_every_conflict_and_nothing_else() {
        let mut state = test_conflict_state();
        state.transfers.push(QueuedTransfer::new(
            PathBuf::from("/source/notes.txt"),
            PathBuf::from("/target/notes.txt"),
        ));
        state
            .conflicts
            .push(file_conflict("/source/notes.txt", "/target/notes.txt"));
        state.transfers.push(QueuedTransfer::new(
            PathBuf::from("/source/other.txt"),
            PathBuf::from("/target/other.txt"),
        ));
        state.apply_to_all = true;
        let (mut browser, _state) = browser_resolving(state);

        drop(browser.resolve_transfer_conflict_choice(TransferConflictChoice::KeepBoth));

        let strategies = queued_copy_transfers(&browser)
            .iter()
            .map(|transfer| transfer.conflict_strategy)
            .collect::<Vec<_>>();
        assert_eq!(
            strategies,
            [
                TransferConflictStrategy::KeepBoth,
                TransferConflictStrategy::KeepBoth,
                TransferConflictStrategy::Fail
            ]
        );
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
