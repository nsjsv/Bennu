use std::sync::Arc;

use file_operation_store::StoredProgress;

use super::*;

#[derive(Debug, Clone)]
pub(crate) enum PersistedShutdownFileOperation {
    Recoverable(StoredInterruptedRecoverableTask),
    Transient(u64),
}

#[derive(Debug, Default)]
pub(crate) struct FileOperationPersistenceAcceptance {
    pub(crate) error: Option<String>,
    pub(crate) task_id_remap: Option<(u64, u64)>,
    pub(crate) rejected_task_id: Option<u64>,
    pub(crate) canceled_before_start_task_id: Option<u64>,
    pub(crate) persisted_recoverable_terminal_stored_id: Option<u64>,
    pub(crate) persisted_shutdown_operation: Option<PersistedShutdownFileOperation>,
}

impl FileOperationQueue {
    pub(super) fn start_next(&mut self) -> Option<String> {
        if self.active_subscription().is_some() {
            return None;
        }
        if let Some(position) = self
            .tasks
            .iter()
            .find(|task| {
                task.status == FileOperationStatus::Pending
                    && (!task.operation.uses_recovery_journal() || task.stored_id.is_some())
            })
            .map(|task| task.id)
            .and_then(|id| self.tasks.iter().position(|task| task.id == id))
        {
            let task = &mut self.tasks[position];
            task.status = FileOperationStatus::Running;
            task.execution_phase = Some(FileOperationExecutionPhase::Preparing);
            task.progress = FileOperationProgress::pending();
            task.completion_warning = None;
            task.error = None;
            let _ = task.run_state_sender.send(FileOperationRunState::Running);
            self.queue_task_state(
                position,
                StoredTaskStatus::Running,
                TaskStatePersistence::Update,
            );
        }
        None
    }

    pub(super) fn restore_stored_task(&mut self, stored_task: StoredTask) -> Option<String> {
        let StoredTask {
            id,
            operation,
            status,
            progress,
            ..
        } = stored_task;

        if stored_status_is_terminal(status) {
            return None;
        }

        let Some(operation) = QueuedFileOperation::from_resumable_stored(operation) else {
            self.mark_interrupted_non_resumable_task_failed(id, progress);
            return None;
        };
        let runner_lease = match self
            .store
            .as_ref()
            .expect("restored queue has an operation store")
            .try_acquire_recoverable_task_runner(id)
        {
            Ok(Some(runner_lease)) => runner_lease,
            Ok(None) => return None,
            Err(error) => return Some(storage_error(error)),
        };

        let (task_status, initial_run_state, cancel_on_restore) = match status {
            StoredTaskStatus::Paused => (
                FileOperationStatus::Paused,
                FileOperationRunState::Paused,
                false,
            ),
            StoredTaskStatus::Canceling => (
                FileOperationStatus::Canceling,
                FileOperationRunState::Running,
                true,
            ),
            StoredTaskStatus::Pending
            | StoredTaskStatus::Running
            | StoredTaskStatus::RecoveryPending => (
                FileOperationStatus::Pending,
                FileOperationRunState::Running,
                false,
            ),
            StoredTaskStatus::Failed | StoredTaskStatus::Completed | StoredTaskStatus::Canceled => {
                return None;
            }
        };
        let (run_state_sender, run_state_receiver) = watch::channel(initial_run_state);
        let cancel = CancellationToken::new();
        let is_read = self.is_panel_open;
        if cancel_on_restore {
            cancel.cancel();
        }
        self.tasks.push(FileOperationTask {
            id,
            transfer_progress: initial_transfer_progress(&operation),
            enqueued_at: std::time::SystemTime::now(),
            operation,
            status: task_status,
            progress: FileOperationProgress::pending(),
            completion_warning: None,
            error: None,
            is_read,
            cancel,
            _runner_lease: Some(Arc::new(runner_lease)),
            run_state_sender,
            run_state_receiver,
            stored_id: Some(id),
            execution_phase: matches!(
                task_status,
                FileOperationStatus::Paused | FileOperationStatus::Canceling
            )
            .then_some(FileOperationExecutionPhase::Preparing),
            post_insert_disposition: PostInsertDisposition::Continue,
            terminal_persistence_pending: false,
            accepted_direct_move_revisions: HashMap::new(),
        });
        let position = self.tasks.len().saturating_sub(1);
        self.queue_task_state(
            position,
            self.tasks[position].status.to_stored(),
            TaskStatePersistence::Update,
        );
        None
    }

    pub(super) fn mark_interrupted_non_resumable_task_failed(
        &mut self,
        id: u64,
        progress: StoredProgress,
    ) {
        let Some(store) = self.store.clone() else {
            return;
        };
        self.queue_persistence_action(FileOperationPersistenceAction::UpdateState {
            store,
            stored_task_id: id,
            status: StoredTaskStatus::Failed,
            progress,
            error: Some("Task was interrupted and cannot safely resume".to_owned()),
            persistence: TaskStatePersistence::Update,
        });
    }

    pub(super) fn allocate_local_id(&mut self) -> u64 {
        loop {
            let id = self.next_local_id;
            self.next_local_id = id.checked_add(1).unwrap_or(LOCAL_TASK_ID_START);
            if !self.tasks.iter().any(|task| task.id == id) {
                return id;
            }
        }
    }

    pub(super) fn mark_all_read(&mut self) {
        for task in &mut self.tasks {
            task.is_read = true;
        }
    }

    pub(super) fn queue_persistence_action(&mut self, action: FileOperationPersistenceAction) {
        let request_id = self.next_persistence_request_id;
        self.next_persistence_request_id = request_id.wrapping_add(1).max(1);
        self.persistence_requests
            .push_back(FileOperationPersistenceRequest { request_id, action });
    }

    pub(crate) fn take_next_persistence_request(
        &mut self,
    ) -> Option<FileOperationPersistenceRequest> {
        if self.persistence_in_flight.is_some() {
            return None;
        }
        let request = self.persistence_requests.pop_front()?;
        self.persistence_in_flight = Some(request.request_id);
        Some(request)
    }

    pub(crate) fn persistence_is_idle(&self) -> bool {
        self.persistence_in_flight.is_none() && self.persistence_requests.is_empty()
    }

    pub(crate) fn accept_persistence_outcome(
        &mut self,
        persistence_outcome: FileOperationPersistenceOutcome,
    ) -> FileOperationPersistenceAcceptance {
        if self.persistence_in_flight != Some(persistence_outcome.request_id) {
            return FileOperationPersistenceAcceptance::default();
        }
        self.persistence_in_flight = None;
        let mut acceptance = FileOperationPersistenceAcceptance::default();
        match persistence_outcome.completion {
            FileOperationPersistenceCompletion::Insert {
                ui_task_id,
                requires_recovery_journal,
                outcome,
            } => match outcome {
                Ok(persisted) => {
                    let local_task_id = persisted.ui_task_id;
                    let stored_task_id = persisted.stored_task_id;
                    acceptance.persisted_shutdown_operation = self.accept_inserted_task(persisted);
                    acceptance.task_id_remap = Some((local_task_id, stored_task_id));
                    acceptance.canceled_before_start_task_id = self
                        .tasks
                        .iter()
                        .find(|task| task.id == local_task_id)
                        .filter(|task| {
                            task.status == FileOperationStatus::Canceled
                                && task.post_insert_disposition == PostInsertDisposition::Continue
                        })
                        .map(|_| stored_task_id);
                }
                Err(error) => {
                    acceptance.error = Some(error.clone());
                    if let Some(position) = self.tasks.iter().position(|task| task.id == ui_task_id)
                    {
                        if requires_recovery_journal {
                            record_file_operation_failure(
                                ui_task_id,
                                self.tasks[position].operation.title(),
                                &sanitized_application_log_detail(&error),
                            );
                            acceptance.rejected_task_id = Some(ui_task_id);
                            #[cfg(test)]
                            if self.persist_synchronously_for_tests {
                                let task = &mut self.tasks[position];
                                task.status = FileOperationStatus::Failed;
                                task.error = Some(error);
                                task.is_read = self.is_panel_open;
                            } else {
                                self.tasks.remove(position);
                            }
                            #[cfg(not(test))]
                            self.tasks.remove(position);
                        } else if self.tasks[position].status == FileOperationStatus::Canceled {
                            acceptance.canceled_before_start_task_id = Some(ui_task_id);
                        }
                        let _ = self.start_next();
                    }
                }
            },
            FileOperationPersistenceCompletion::Mutation { effect, outcome } => {
                if let Err(error) = outcome {
                    if let PersistenceEffect::DeleteUiTasks(ui_task_ids) = &effect {
                        for ui_task_id in ui_task_ids {
                            self.pending_deletions.remove(ui_task_id);
                        }
                    }
                    acceptance.error = Some(error);
                } else {
                    match effect {
                        PersistenceEffect::None => {}
                        PersistenceEffect::DeleteUiTasks(ui_task_ids) => {
                            for ui_task_id in &ui_task_ids {
                                self.pending_deletions.remove(ui_task_id);
                            }
                            self.tasks.retain(|task| !ui_task_ids.contains(&task.id));
                        }
                        PersistenceEffect::PersistRecoverableTerminalState { stored_task_id } => {
                            if let Some(task) = self
                                .tasks
                                .iter_mut()
                                .find(|task| task.stored_id == Some(stored_task_id))
                            {
                                task._runner_lease = None;
                                task.terminal_persistence_pending = false;
                            }
                            acceptance.persisted_recoverable_terminal_stored_id =
                                Some(stored_task_id);
                        }
                        PersistenceEffect::ReleaseRecoverableRunnerLease { stored_task_id } => {
                            if let Some(task) = self
                                .tasks
                                .iter_mut()
                                .find(|task| task.stored_id == Some(stored_task_id))
                            {
                                task._runner_lease = None;
                                task.terminal_persistence_pending = false;
                            }
                        }
                    }
                }
            }
        }
        acceptance
    }

    fn accept_inserted_task(
        &mut self,
        persisted: PersistedFileOperation,
    ) -> Option<PersistedShutdownFileOperation> {
        let Some(position) = self
            .tasks
            .iter()
            .position(|task| task.id == persisted.ui_task_id)
        else {
            self.queue_persistence_action(FileOperationPersistenceAction::DeleteTasks {
                store: self
                    .store
                    .clone()
                    .expect("inserted task has an operation store"),
                stored_task_ids: vec![persisted.stored_task_id],
                ui_task_ids: Vec::new(),
            });
            return None;
        };
        self.tasks[position].stored_id = Some(persisted.stored_task_id);
        self.tasks[position]._runner_lease = persisted.runner_lease;

        let post_insert_disposition = self.tasks[position].post_insert_disposition;
        match post_insert_disposition {
            PostInsertDisposition::ShutdownRecoverable(status) => {
                return Some(PersistedShutdownFileOperation::Recoverable(
                    StoredInterruptedRecoverableTask {
                        task_id: persisted.stored_task_id,
                        status,
                        progress: self.tasks[position].progress.to_stored(),
                        error: Some("Application stopped with recoverable work pending".to_owned()),
                    },
                ));
            }
            PostInsertDisposition::ShutdownTransient => {
                return Some(PersistedShutdownFileOperation::Transient(
                    persisted.stored_task_id,
                ));
            }
            PostInsertDisposition::Continue => {}
        }

        match self.tasks[position].status {
            FileOperationStatus::Pending => {
                let _ = self.start_next();
            }
            FileOperationStatus::Running => {
                self.queue_task_state(
                    position,
                    StoredTaskStatus::Running,
                    TaskStatePersistence::Update,
                );
            }
            FileOperationStatus::Paused => self.queue_task_status(position),
            FileOperationStatus::Canceling => self.queue_task_status(position),
            FileOperationStatus::Canceled => {
                self.queue_task_state(
                    position,
                    StoredTaskStatus::Canceled,
                    TaskStatePersistence::Update,
                );
                let _ = self.start_next();
            }
            FileOperationStatus::Failed | FileOperationStatus::Completed => {
                let status = self.tasks[position].status;
                self.queue_task_state(position, status.to_stored(), TaskStatePersistence::Update);
            }
        }
        None
    }

    pub(super) fn queue_task_status(&mut self, position: usize) {
        let task = &self.tasks[position];
        let (Some(store), Some(stored_task_id)) = (self.store.clone(), task.stored_id) else {
            return;
        };
        self.queue_persistence_action(FileOperationPersistenceAction::UpdateStatus {
            store,
            stored_task_id,
            status: task.status.to_stored(),
        });
    }

    pub(super) fn queue_task_state(
        &mut self,
        position: usize,
        status: StoredTaskStatus,
        persistence: TaskStatePersistence,
    ) {
        let task = &self.tasks[position];
        let (Some(store), Some(stored_task_id)) = (self.store.clone(), task.stored_id) else {
            return;
        };
        self.queue_persistence_action(FileOperationPersistenceAction::UpdateState {
            store,
            stored_task_id,
            status,
            progress: task.progress.to_stored(),
            error: task.error.clone(),
            persistence,
        });
    }

    #[cfg(test)]
    pub(super) fn complete_test_persistence(&mut self) -> Option<String> {
        if !self.persist_synchronously_for_tests {
            return None;
        }
        let mut storage_error = None;
        while let Some(request) = self.take_next_persistence_request() {
            let outcome = execute_file_operation_persistence(request);
            let acceptance = self.accept_persistence_outcome(outcome);
            storage_error = combine_storage_errors(storage_error, acceptance.error);
        }
        storage_error
    }
}
