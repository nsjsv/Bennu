use iced::Task;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use file_core::{validate_entry_name, ResolvedEntryChange};
use file_operations::{
    path_after_completed_migrations, CompletedPathMigration, FileOperationCompletion,
    FileOperationOutcome, RenameRejection,
};

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::{file_operation_driver_task, file_operation_persistence_command};
use crate::model::{
    AdvancedNewFolderAfter, BrowserViewMode, DestructiveActionConfirmation,
    IconGridExpansionMigration, Message, NavigationMode,
};
use crate::operation_history::PendingHistoryOperation;
use crate::view::rename_input_id;
use file_operations::{
    FileOperationEnqueueOutcome, FileOperationFinish, FileOperationPersistenceOutcome,
    QueuedFileOperation,
};

/// 完成后自动进入重命名的入口跟随执行结果的真实路径:执行器按唯一名
/// 规则落地时(重名换「新建文件夹 2」、收纳竞争重选),入队侧的预测
/// 路径不再成立。
fn outcome_created_path(outcome: &FileOperationOutcome) -> Option<PathBuf> {
    match outcome {
        FileOperationOutcome::CreateDirectory { path }
        | FileOperationOutcome::CreateEmptyFile { path } => Some(path.clone()),
        // 完成后重命名落点 = 第一个叶子实际建成的目录(重名时已续编号)。
        FileOperationOutcome::CreateDirectories { leaf_paths, .. } => leaf_paths.first().cloned(),
        FileOperationOutcome::GatheredIntoNewFolder { directory, .. } => Some(directory.clone()),
        _ => None,
    }
}

// ponytail: 重命名会话短且输入有限，完整字符串快照的内存上限随编辑次数和名称长度增长；若支持长文本或长期会话，再升级为合并编辑事务。
#[derive(Debug, Default)]
pub(super) struct RenameInputHistory {
    undo_values: Vec<String>,
    redo_values: Vec<String>,
}

impl RenameInputHistory {
    fn apply_input_change(&mut self, current_value: &mut String, next_value: String) {
        if current_value == &next_value {
            return;
        }

        self.undo_values
            .push(std::mem::replace(current_value, next_value));
        self.redo_values.clear();
    }

    fn undo(&mut self, current_value: &mut String) {
        let Some(previous_value) = self.undo_values.pop() else {
            return;
        };

        self.redo_values
            .push(std::mem::replace(current_value, previous_value));
    }

    fn redo(&mut self, current_value: &mut String) {
        let Some(next_value) = self.redo_values.pop() else {
            return;
        };

        self.undo_values
            .push(std::mem::replace(current_value, next_value));
    }

    fn reset(&mut self) {
        self.undo_values.clear();
        self.redo_values.clear();
    }
}

pub(super) fn queue_finish_from_completion(
    completion: &FileOperationCompletion,
) -> FileOperationFinish {
    match completion {
        FileOperationCompletion::Succeeded(outcome) => {
            match outcome.completion_warning(bennu_localization::current_language()) {
                Some(warning) => FileOperationFinish::SucceededWithWarning(warning),
                None => FileOperationFinish::Succeeded,
            }
        }
        FileOperationCompletion::Canceled(_) => FileOperationFinish::Canceled,
        FileOperationCompletion::Failed { error, .. } => FileOperationFinish::Failed(error.clone()),
        FileOperationCompletion::RecoveryInterrupted(error, _) => {
            FileOperationFinish::RecoveryInterrupted(error.clone())
        }
        FileOperationCompletion::RecoveryBlocked { error, .. } => {
            FileOperationFinish::RecoveryBlocked(error.clone())
        }
        FileOperationCompletion::RenameRejected { reason, .. } => {
            FileOperationFinish::Failed(reason.message(bennu_localization::current_language()))
        }
    }
}

impl FileBrowser {
    pub(super) fn accept_file_operation_direct_moves_committed(
        &mut self,
        task_id: u64,
        commits: Vec<crate::commands::DurableDirectMoveCommit>,
    ) -> Task<Message> {
        let mut migrations = Vec::with_capacity(commits.len());
        for commit in commits {
            if !self.operation_queue.accept_durable_direct_move_commit(
                task_id,
                &commit.work_key,
                &commit.source,
                commit.checkpoint_revision,
            ) {
                continue;
            }
            tracing::debug!(
                target: "app_ui::file_operations",
                event = "direct_move_committed",
                task_id,
                checkpoint_revision = commit.checkpoint_revision,
                source = %commit.source.display(),
                target_path = %commit.target.display(),
                "durable basic direct move target became visible"
            );
            migrations.push(CompletedPathMigration::new(commit.source, commit.target));
        }
        if migrations.is_empty() {
            return Task::none();
        }

        // rename 事实已由 FileOperationMovesRenamed 处理（列表增量 + 路径迁移 + 搜索刷新）。
        // 这里只剩 summaries 失效重算：journal 落盘晚于 rename，整页 reload 不再等它。
        let operation = self.operation_queue.operation(task_id).cloned();
        if let Some(operation) = operation.as_ref() {
            self.invalidate_list_directory_summaries_for_file_operation(operation);
            return self.schedule_visible_list_directory_summaries();
        }
        Task::none()
    }

    // 操作完成后的刷新分流：成功 Move 的条目变更已由 renamed 消息增量应用，
    // 这里只对账（对不上才全量兜底）；Copy 交给 watcher 增量补入 + 溢出兜底；
    // 其余操作类型维持全量重扫。
    fn finish_refresh_task_for_operation(
        &mut self,
        operation: &QueuedFileOperation,
        completed_successfully: bool,
    ) -> Task<Message> {
        match (operation, completed_successfully) {
            (QueuedFileOperation::Move { transfers, .. }, true) => {
                let pairs = transfers
                    .iter()
                    .map(|transfer| (transfer.source.clone(), transfer.target.clone()))
                    .collect::<Vec<_>>();
                if self.transfers_are_reflected_in_visible_directories(&pairs) {
                    self.schedule_visible_list_directory_summaries()
                } else {
                    self.reload_visible_panes_after_file_operation_preserving_list_directory_summaries()
                }
            }
            (QueuedFileOperation::Copy { .. }, true) => {
                self.schedule_visible_list_directory_summaries()
            }
            _ => {
                self.reload_visible_panes_after_file_operation_preserving_list_directory_summaries()
            }
        }
    }

    // rename 已在文件系统发生：条目变更立即增量应用（源目录移除 + 目标目录插入，
    // 元数据从源条目继承——rename 不改变文件元数据），不等 journal 落盘。
    // 源条目不可见（目录未显示）时跳过立即应用，交给 watcher 与对账兜底。
    pub(super) fn accept_file_operation_moves_renamed(
        &mut self,
        moves: Vec<file_operations::CompletedTransfer>,
    ) -> Task<Message> {
        if moves.is_empty() {
            return Task::none();
        }
        let mut per_directory: HashMap<PathBuf, Vec<ResolvedEntryChange>> = HashMap::new();
        for completed in &moves {
            let Some(source_directory) = completed.source.parent().map(Path::to_path_buf) else {
                continue;
            };
            let Some(target_directory) = completed.target.parent().map(Path::to_path_buf) else {
                continue;
            };
            let Some(target_name) = completed
                .target
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
            else {
                continue;
            };
            let Some(existing) = self.find_discovered_entry(&completed.source) else {
                continue;
            };
            let to = existing.renamed_to(completed.target.clone(), target_name);
            // 跨目录移动要同时更新两个目录的列表：源移除 + 目标插入。
            if source_directory == target_directory {
                per_directory.entry(source_directory).or_default().push(
                    ResolvedEntryChange::Renamed {
                        from: completed.source.clone(),
                        to,
                    },
                );
            } else {
                per_directory.entry(source_directory).or_default().push(
                    ResolvedEntryChange::Removed {
                        path: completed.source.clone(),
                    },
                );
                per_directory
                    .entry(target_directory)
                    .or_default()
                    .push(ResolvedEntryChange::Added(to));
            }
        }

        let mut tasks = Vec::new();
        for (directory, changes) in per_directory {
            if let Some(task) = self.apply_entry_changes(&directory, &changes) {
                tasks.push(task);
            }
        }

        let migrations = moves
            .iter()
            .map(|completed| {
                CompletedPathMigration::new(completed.source.clone(), completed.target.clone())
            })
            .collect::<Vec<_>>();
        tasks.push(self.migrate_completed_paths(&migrations));
        if self.search_workspace.is_some() {
            tasks.push(self.submit_search());
        }
        Task::batch(tasks)
    }

    pub(super) fn accept_file_operation_finished(
        &mut self,
        task_id: u64,
        completion: FileOperationCompletion,
    ) -> Task<Message> {
        let completed_operation = self.operation_queue.operation(task_id).cloned();
        let queue_outcome = queue_finish_from_completion(&completion);
        let Some(terminal_status) = self.operation_queue.finish(task_id, queue_outcome) else {
            return Task::none();
        };

        let completed_successfully = matches!(completion, FileOperationCompletion::Succeeded(_));
        let is_history_replay = self.operation_history.is_replaying(task_id);
        let created_path = (completed_successfully && !is_history_replay)
            .then(|| match &completion {
                FileOperationCompletion::Succeeded(outcome) => outcome_created_path(outcome),
                _ => None,
            })
            .flatten();

        // 高级新建的收尾动作(进入/新标签进入)替代默认的重命名态;
        // 历史重放不消费收尾意图;创建失败时丢弃,避免遗留到无关操作。
        let mut tasks: Vec<Task<Message>> = Vec::new();
        let is_create_directories = completed_operation.as_ref().is_some_and(|operation| {
            matches!(operation, QueuedFileOperation::CreateDirectories { .. })
        });
        let after = if !is_history_replay && is_create_directories {
            self.pending_advanced_new_folder_after.take()
        } else {
            None
        }
        .filter(|_| completed_successfully);
        match (after, created_path.clone()) {
            (Some(AdvancedNewFolderAfter::Enter), Some(path)) => {
                // 多栏视图:在另一栏打开新文件夹;其他视图:直接进入。
                if self.tab().view_mode == BrowserViewMode::Columns {
                    tasks.push(self.open_column_for_directory(path));
                } else {
                    tasks.push(self.navigate_to(path, NavigationMode::RecordHistory));
                }
            }
            (Some(AdvancedNewFolderAfter::EnterInNewTab), Some(path)) => {
                tasks.push(self.open_directory_in_new_tab(path));
            }
            (_, _) => {
                if let Some(path) = created_path {
                    self.pending_created_entry_rename = Some(path);
                }
            }
        }

        // 复制副本对齐 Finder:成功后整批选中新副本(撤销重放不抢焦点)。
        if !is_history_replay
            && completed_operation
                .as_ref()
                .is_some_and(QueuedFileOperation::selects_completed_copies)
        {
            if let FileOperationCompletion::Succeeded(FileOperationOutcome::Copy {
                transfers,
                ..
            }) = &completion
            {
                self.select_operation_result_paths(
                    transfers
                        .iter()
                        .map(|transfer| transfer.target.clone())
                        .collect(),
                );
            }
        }

        let desktop_notification_task = match completed_operation.as_ref() {
            Some(operation) => {
                self.file_operation_notification_command(operation, terminal_status, &completion)
            }
            None => Task::none(),
        };
        // 删掉后从目录里消失的路径。放不进回收站的还在原地，同 Nautilus 问要不要永久删除。
        let removed_file_paths = match (completed_operation.as_ref(), &completion) {
            (
                Some(QueuedFileOperation::Trash { paths }),
                FileOperationCompletion::Succeeded(FileOperationOutcome::Trash {
                    untrashable, ..
                }),
            ) => {
                if !untrashable.is_empty() {
                    self.request_destructive_action_confirmation(
                        DestructiveActionConfirmation::DeleteUntrashable {
                            paths: untrashable.clone(),
                        },
                    );
                }
                Some(
                    paths
                        .iter()
                        .filter(|path| !untrashable.contains(path))
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            }
            (
                Some(QueuedFileOperation::DeletePermanently { paths }),
                FileOperationCompletion::Succeeded(_),
            ) => Some(paths.clone()),
            _ => None,
        };
        let deletion_focus_task = if completed_successfully {
            self.focus_after_file_operation_removal(
                completed_operation.as_ref(),
                removed_file_paths.as_deref(),
            )
        } else {
            Task::none()
        };

        let path_migration_task = self.migrate_paths_after_file_operation(&completion);
        if let Some(removed_paths) = &removed_file_paths {
            self.reconcile_icon_grid_removed_paths(removed_paths);
        }
        match &completion {
            FileOperationCompletion::Succeeded(outcome) => {
                self.operation_history.accept_completed(task_id, outcome);
            }
            FileOperationCompletion::Canceled(partial_outcome)
            | FileOperationCompletion::Failed {
                partial_outcome, ..
            } => self
                .operation_history
                .accept_failed(task_id, partial_outcome),
            FileOperationCompletion::RecoveryInterrupted(_, partial_outcome)
            | FileOperationCompletion::RecoveryBlocked {
                partial_outcome, ..
            } => self
                .operation_history
                .accept_interrupted(task_id, partial_outcome),
            FileOperationCompletion::RenameRejected {
                path,
                attempted_name,
                reason,
            } => {
                self.operation_history
                    .accept_failed(task_id, &FileOperationOutcome::NoHistory);
                // 原因已由 queue_finish_from_completion 写进任务行，这是文件操作错误的
                // 归属位置。撤销/重做的新名字来自历史而不是输入框，没有可以接着改的输入。
                if !is_history_replay {
                    tasks.push(self.reopen_rejected_rename(path, attempted_name, *reason));
                }
            }
        }

        let pane_reload_task = if let Some(operation) = completed_operation.as_ref() {
            self.invalidate_list_directory_summaries_for_file_operation(operation);
            self.finish_refresh_task_for_operation(operation, completed_successfully)
        } else {
            self.reload_visible_panes_after_file_operation()
        };
        // 挂起期间回收站可能被本批任务改了很多条;终结时若仍有别的批次在跑,
        // 这次补刷会被挂起逻辑再次拦下并重新记脏,最后一个批次结束后刷到终态。
        let trash_batch_rescan_task = if completed_operation
            .as_ref()
            .is_some_and(|op| op.changes_trash())
            && self.trash_batch_rescan_pending
        {
            self.trash_batch_rescan_pending = false;
            self.refresh_trash_snapshot_for_trash_tabs()
        } else {
            Task::none()
        };
        let search_refresh_task = if self.search_workspace.is_some() {
            self.submit_search()
        } else {
            Task::none()
        };
        Task::batch([
            deletion_focus_task,
            desktop_notification_task,
            path_migration_task,
            search_refresh_task,
            pane_reload_task,
            trash_batch_rescan_task,
            self.continue_file_operation_persistence(),
            self.launch_ready_driver(),
        ])
        .chain(Task::batch(tasks))
    }

    pub(super) fn accept_file_operation_persistence_finished(
        &mut self,
        persistence_outcome: FileOperationPersistenceOutcome,
    ) -> Task<Message> {
        let acceptance = self
            .operation_queue
            .accept_persistence_outcome(persistence_outcome);
        if let Some((local_task_id, stored_task_id)) = acceptance.task_id_remap {
            self.operation_history
                .remap_pending_task(local_task_id, stored_task_id);
        }
        if let Some(error) = acceptance.error {
            self.show_global_error(error);
        }
        if let Some(task_id) = acceptance.rejected_task_id {
            self.operation_history
                .accept_failed(task_id, &FileOperationOutcome::NoHistory);
        }
        if let Some(task_id) = acceptance.canceled_before_start_task_id {
            self.operation_history
                .accept_failed(task_id, &FileOperationOutcome::NoHistory);
        }
        let next_persistence = self.continue_file_operation_persistence();
        let shutdown_progress = self.accept_file_operation_persistence_progress(
            acceptance.persisted_recoverable_terminal_stored_id,
            acceptance.persisted_shutdown_operation,
        );
        Task::batch([next_persistence, shutdown_progress])
    }

    pub(super) fn continue_file_operation_persistence(&mut self) -> Task<Message> {
        self.operation_queue
            .take_next_persistence_request()
            .map(file_operation_persistence_command)
            .unwrap_or_else(Task::none)
    }

    /// 队列状态转换后签发驱动者(Task::stream)。凡是能让任务进入
    /// Running/Canceling 的入口（入队、终结、取消、写库回执、监督 tick、
    /// 启动恢复）都在转换当处调用；无待签发任务时为空操作。
    pub(super) fn launch_ready_driver(&mut self) -> Task<Message> {
        self.operation_queue
            .poll_driver_launch()
            .map(file_operation_driver_task)
            .unwrap_or_else(Task::none)
    }

    fn migrate_paths_after_file_operation(
        &mut self,
        completion: &FileOperationCompletion,
    ) -> Task<Message> {
        let migrations = completion.completed_path_migrations();
        self.migrate_completed_paths(&migrations)
    }

    fn migrate_completed_paths(&mut self, migrations: &[CompletedPathMigration]) -> Task<Message> {
        if migrations.is_empty() {
            return Task::none();
        }

        self.cancel_file_drag_interaction();
        self.cancel_expansion_follow_plans();

        let invalidate_icon_grid_expansion =
            self.icon_grid_expansion.as_mut().is_some_and(|state| {
                state.migrate_completed_paths(migrations) == IconGridExpansionMigration::Invalidated
            });
        if invalidate_icon_grid_expansion {
            self.clear_icon_grid_expansion();
        }

        for pane in &mut self.panes {
            pane.migrate_completed_paths(migrations);
        }
        self.entries_replaced();
        self.column_return_targets = self
            .column_return_targets
            .drain()
            .map(|(directory, target)| {
                (
                    path_after_completed_migrations(&directory, migrations),
                    path_after_completed_migrations(&target, migrations),
                )
            })
            .collect();
        if let Some(path) = &mut self.pending_created_entry_rename {
            *path = path_after_completed_migrations(path, migrations);
        }
        if let Some(path) = &mut self.renaming {
            *path = path_after_completed_migrations(path, migrations);
        }
        if let Some(address_editing) = &mut self.address_editing {
            for suggestion in &mut address_editing.session.suggestions {
                *suggestion = path_after_completed_migrations(suggestion, migrations);
            }
        }

        self.follow_terminal_panel_directory();
        self.clamp_visible_pane_viewports()
    }

    pub(super) fn commit_rename(&mut self) -> Task<Message> {
        let Some(path) = self
            .renaming
            .clone()
            .or_else(|| self.tab().selected.clone())
        else {
            return Task::none();
        };

        let name = self.rename_input.trim();
        if name.is_empty() {
            self.renaming = None;
            return Task::none();
        }

        let old_name = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        if old_name == name {
            self.renaming = None;
            return Task::none();
        }
        if self.rename_input_rejection(&path, name).is_some() {
            // 原因已经浮在输入框旁：保持编辑让用户改名，不入队。
            return focus_rename_input_command();
        }

        self.renaming = None;
        self.context_menu = None;
        self.enqueue_file_operation(QueuedFileOperation::Rename {
            path,
            new_name: name.into(),
        })
    }

    /// 重命名输入框旁要显示的原因：执行端的拒绝优先，其次是当前输入的预检。
    pub(crate) fn rename_hint(&self) -> Option<RenameRejection> {
        let path = self.renaming.as_ref()?;
        self.rename_rejection
            .or_else(|| self.rename_input_rejection(path, self.rename_input.trim()))
    }

    /// 按已加载的同目录条目预检新名字。没显示的隐藏文件查不到，最终以执行端
    /// 的原子判定为准。空名回车等于取消重命名，不算被拒。
    fn rename_input_rejection(&self, path: &Path, name: &str) -> Option<RenameRejection> {
        if name.is_empty() {
            return None;
        }
        if let Err(reason) = validate_entry_name(OsStr::new(name)) {
            return Some(RenameRejection::InvalidName(reason));
        }
        let target = path.parent()?.join(name);
        (target.as_path() != path && self.entry_for_path(&target).is_some())
            .then_some(RenameRejection::NameTaken)
    }

    /// 用户发起的重命名被拒：在原条目上重新打开输入框，填回尝试的名字并提示原因。
    /// 条目已不在显示里，或用户已经在改别的名字时，没有地方重开，原因只留在任务行上。
    fn reopen_rejected_rename(
        &mut self,
        path: &Path,
        attempted_name: &OsStr,
        reason: RenameRejection,
    ) -> Task<Message> {
        if self.renaming.is_some() || self.entry_for_path(path).is_none() {
            return Task::none();
        }
        self.context_menu = None;
        self.select_path(path.to_path_buf());
        self.rename_input = attempted_name.to_string_lossy().into_owned();
        self.rename_input_history.reset();
        self.renaming = Some(path.to_path_buf());
        self.rename_rejection = Some(reason);
        focus_rename_input_command()
    }

    pub(super) fn commit_rename_if_active(&mut self) -> Task<Message> {
        if self.renaming.is_some() {
            self.commit_rename()
        } else {
            Task::none()
        }
    }

    pub(super) fn begin_rename(&mut self, path: PathBuf) -> Task<Message> {
        if self.tab().is_trash_view {
            return Task::none();
        }

        self.context_menu = None;
        self.select_path(path.clone());
        self.rename_input_history.reset();
        self.rename_rejection = None;
        self.renaming = Some(path);
        focus_rename_input_command()
    }

    pub(super) fn begin_rename_selected(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let Some(path) = self.tab().selected.clone() else {
            return Task::none();
        };
        self.begin_rename(path)
    }

    pub(super) fn focus_created_entry_for_rename(&mut self) -> Task<Message> {
        let Some(path) = self.pending_created_entry_rename.clone() else {
            return Task::none();
        };
        if self.entry_for_path(&path).is_none() {
            return Task::none();
        }

        self.pending_created_entry_rename = None;
        self.begin_rename(path)
    }

    pub(super) fn apply_rename_input_change(&mut self, value: String) -> Task<Message> {
        self.rename_input_history
            .apply_input_change(&mut self.rename_input, value);
        self.rename_rejection = None;
        Task::none()
    }

    pub(super) fn undo_rename_input_change(&mut self) -> Task<Message> {
        self.rename_input_history.undo(&mut self.rename_input);
        self.rename_rejection = None;
        Task::none()
    }

    pub(super) fn redo_rename_input_change(&mut self) -> Task<Message> {
        self.rename_input_history.redo(&mut self.rename_input);
        self.rename_rejection = None;
        Task::none()
    }

    pub(super) fn enqueue_file_operation(
        &mut self,
        operation: QueuedFileOperation,
    ) -> Task<Message> {
        self.enqueue_file_operation_with_history(operation, None)
    }

    pub(super) fn undo_file_operation(&mut self) -> Task<Message> {
        self.context_menu = None;
        let Some((operation, pending_history)) = self.operation_history.take_undo_operation()
        else {
            return Task::none();
        };
        self.enqueue_file_operation_with_history(operation, Some(pending_history))
    }

    pub(super) fn redo_file_operation(&mut self) -> Task<Message> {
        self.context_menu = None;
        let Some((operation, pending_history)) = self.operation_history.take_redo_operation()
        else {
            return Task::none();
        };
        self.enqueue_file_operation_with_history(operation, Some(pending_history))
    }

    fn enqueue_file_operation_with_history(
        &mut self,
        operation: QueuedFileOperation,
        pending_history: Option<PendingHistoryOperation>,
    ) -> Task<Message> {
        self.retract_global_error(GlobalErrorSource::FileOperationEnqueue);
        match self.operation_queue.enqueue(operation) {
            FileOperationEnqueueOutcome::Queued { task_id } => {
                if let Some(pending_history) = pending_history {
                    self.operation_history
                        .track_pending(task_id, pending_history);
                }
            }
            FileOperationEnqueueOutcome::Rejected { error } => {
                self.show_global_error_from(GlobalErrorSource::FileOperationEnqueue, error);
                if let Some(pending_history) = pending_history {
                    self.operation_history.reject_pending(pending_history);
                }
            }
        }
        Task::batch([
            self.continue_file_operation_persistence(),
            self.launch_ready_driver(),
        ])
    }
    fn focus_after_file_operation_removal(
        &mut self,
        operation: Option<&QueuedFileOperation>,
        removed_file_paths: Option<&[PathBuf]>,
    ) -> Task<Message> {
        if self.search_workspace.is_some() {
            return Task::none();
        }
        if let Some(paths) = removed_file_paths {
            return self.focus_after_removed_file_operation_paths(paths);
        }
        let Some(operation) = operation else {
            return Task::none();
        };
        match operation {
            QueuedFileOperation::DeleteTrashEntries { entries }
            | QueuedFileOperation::Restore { entries } => {
                let removed_paths = entries
                    .iter()
                    .map(|entry| entry.trash_path.clone())
                    .collect::<Vec<_>>();
                self.focus_after_removed_file_operation_paths(&removed_paths)
            }
            _ => Task::none(),
        }
    }
}

fn focus_rename_input_command() -> Task<Message> {
    let input_id = rename_input_id();
    Task::batch([
        iced::widget::operation::focus(input_id.clone()),
        iced::widget::operation::select_all(input_id),
    ])
}

#[cfg(test)]
mod created_entry_tests;
#[cfg(test)]
mod driver_launch_tests;
#[cfg(test)]
mod path_migration_tests;
#[cfg(test)]
mod rename_rejection_tests;
#[cfg(test)]
mod tests;
