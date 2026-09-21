use std::sync::Arc;

use file_operation_store::{
    RecoverableTaskRunnerLease, StoredOperation, StoredProgress, StoredTaskStatus, TaskQueueStore,
};
use iced::Task;

use crate::model::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStatePersistence {
    Update,
    FinalizeRecovery,
    PreserveRecovery,
}

#[derive(Debug, Clone)]
pub(crate) enum PersistenceEffect {
    None,
    DeleteUiTasks(Vec<u64>),
    PersistRecoverableTerminalState { stored_task_id: u64 },
    ReleaseRecoverableRunnerLease { stored_task_id: u64 },
}

pub(crate) enum FileOperationPersistenceAction {
    Insert {
        ui_task_id: u64,
        store: TaskQueueStore,
        operation: StoredOperation,
        requires_recovery_journal: bool,
    },
    UpdateStatus {
        store: TaskQueueStore,
        stored_task_id: u64,
        status: StoredTaskStatus,
    },
    UpdateState {
        store: TaskQueueStore,
        stored_task_id: u64,
        status: StoredTaskStatus,
        progress: StoredProgress,
        error: Option<String>,
        persistence: TaskStatePersistence,
    },
    DeleteTasks {
        store: TaskQueueStore,
        stored_task_ids: Vec<u64>,
        ui_task_ids: Vec<u64>,
    },
}

#[derive(Debug, Clone)]
enum PersistenceWorkerFailureTarget {
    Insert {
        ui_task_id: u64,
        requires_recovery_journal: bool,
    },
    Mutation(PersistenceEffect),
}

impl PersistenceWorkerFailureTarget {
    fn completion(self, error: tokio::task::JoinError) -> FileOperationPersistenceCompletion {
        let error = format!("File operation queue persistence worker failed: {error}");
        match self {
            Self::Insert {
                ui_task_id,
                requires_recovery_journal,
            } => FileOperationPersistenceCompletion::Insert {
                ui_task_id,
                requires_recovery_journal,
                outcome: Err(error),
            },
            Self::Mutation(effect) => FileOperationPersistenceCompletion::Mutation {
                effect,
                outcome: Err(error),
            },
        }
    }
}

pub(crate) struct FileOperationPersistenceRequest {
    pub(crate) request_id: u64,
    pub(crate) action: FileOperationPersistenceAction,
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedFileOperation {
    pub(crate) ui_task_id: u64,
    pub(crate) stored_task_id: u64,
    pub(crate) runner_lease: Option<Arc<RecoverableTaskRunnerLease>>,
}

#[derive(Debug, Clone)]
pub(crate) enum FileOperationPersistenceCompletion {
    Insert {
        ui_task_id: u64,
        requires_recovery_journal: bool,
        outcome: Result<PersistedFileOperation, String>,
    },
    Mutation {
        effect: PersistenceEffect,
        outcome: Result<(), String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct FileOperationPersistenceOutcome {
    pub(crate) request_id: u64,
    pub(crate) completion: FileOperationPersistenceCompletion,
}

pub(crate) fn file_operation_persistence_command(
    request: FileOperationPersistenceRequest,
) -> Task<Message> {
    let request_id = request.request_id;
    let worker_failure_target = persistence_worker_failure_target(&request.action);
    Task::perform(
        async move {
            let outcome =
                tokio::task::spawn_blocking(move || execute_file_operation_persistence(request))
                    .await
                    .unwrap_or_else(|error| FileOperationPersistenceOutcome {
                        request_id,
                        completion: worker_failure_target.completion(error),
                    });
            outcome
        },
        Message::FileOperationPersistenceFinished,
    )
}

pub(crate) fn execute_file_operation_persistence(
    request: FileOperationPersistenceRequest,
) -> FileOperationPersistenceOutcome {
    let request_id = request.request_id;
    let completion = match request.action {
        FileOperationPersistenceAction::Insert {
            ui_task_id,
            store,
            operation,
            requires_recovery_journal,
        } => {
            let outcome = if requires_recovery_journal {
                store
                    .insert_claimed_recoverable_transfer_task(&operation)
                    .map(|claimed| PersistedFileOperation {
                        ui_task_id,
                        stored_task_id: claimed.task_id,
                        runner_lease: Some(Arc::new(claimed.runner_lease)),
                    })
            } else {
                store
                    .insert_task(&operation)
                    .map(|stored_task_id| PersistedFileOperation {
                        ui_task_id,
                        stored_task_id,
                        runner_lease: None,
                    })
            }
            .map_err(storage_error);
            FileOperationPersistenceCompletion::Insert {
                ui_task_id,
                requires_recovery_journal,
                outcome,
            }
        }
        FileOperationPersistenceAction::UpdateStatus {
            store,
            stored_task_id,
            status,
        } => FileOperationPersistenceCompletion::Mutation {
            effect: PersistenceEffect::None,
            outcome: store
                .update_status(stored_task_id, status)
                .map_err(storage_error),
        },
        FileOperationPersistenceAction::UpdateState {
            store,
            stored_task_id,
            status,
            progress,
            error,
            persistence,
        } => {
            let outcome = match persistence {
                TaskStatePersistence::Update | TaskStatePersistence::PreserveRecovery => {
                    store.update_task_state(stored_task_id, status, progress, error.as_deref())
                }
                TaskStatePersistence::FinalizeRecovery => store.finalize_recoverable_transfer_task(
                    stored_task_id,
                    status,
                    progress,
                    error.as_deref(),
                ),
            }
            .map_err(storage_error);
            let effect = persistence_effect(persistence, status, stored_task_id);
            FileOperationPersistenceCompletion::Mutation { effect, outcome }
        }
        FileOperationPersistenceAction::DeleteTasks {
            store,
            stored_task_ids,
            ui_task_ids,
        } => FileOperationPersistenceCompletion::Mutation {
            effect: PersistenceEffect::DeleteUiTasks(ui_task_ids),
            outcome: store.delete_tasks(&stored_task_ids).map_err(storage_error),
        },
    };
    FileOperationPersistenceOutcome {
        request_id,
        completion,
    }
}

fn persistence_worker_failure_target(
    action: &FileOperationPersistenceAction,
) -> PersistenceWorkerFailureTarget {
    match action {
        FileOperationPersistenceAction::Insert {
            ui_task_id,
            requires_recovery_journal,
            ..
        } => PersistenceWorkerFailureTarget::Insert {
            ui_task_id: *ui_task_id,
            requires_recovery_journal: *requires_recovery_journal,
        },
        FileOperationPersistenceAction::UpdateState {
            stored_task_id,
            status,
            persistence,
            ..
        } => PersistenceWorkerFailureTarget::Mutation(persistence_effect(
            *persistence,
            *status,
            *stored_task_id,
        )),
        FileOperationPersistenceAction::DeleteTasks { ui_task_ids, .. } => {
            PersistenceWorkerFailureTarget::Mutation(PersistenceEffect::DeleteUiTasks(
                ui_task_ids.clone(),
            ))
        }
        FileOperationPersistenceAction::UpdateStatus { .. } => {
            PersistenceWorkerFailureTarget::Mutation(PersistenceEffect::None)
        }
    }
}

fn persistence_effect(
    persistence: TaskStatePersistence,
    status: StoredTaskStatus,
    stored_task_id: u64,
) -> PersistenceEffect {
    match persistence {
        TaskStatePersistence::FinalizeRecovery => {
            PersistenceEffect::PersistRecoverableTerminalState { stored_task_id }
        }
        TaskStatePersistence::PreserveRecovery if status == StoredTaskStatus::RecoveryPending => {
            PersistenceEffect::ReleaseRecoverableRunnerLease { stored_task_id }
        }
        TaskStatePersistence::PreserveRecovery
            if matches!(
                status,
                StoredTaskStatus::Failed | StoredTaskStatus::Completed | StoredTaskStatus::Canceled
            ) =>
        {
            PersistenceEffect::PersistRecoverableTerminalState { stored_task_id }
        }
        TaskStatePersistence::Update | TaskStatePersistence::PreserveRecovery => {
            PersistenceEffect::None
        }
    }
}

fn storage_error(error: impl std::fmt::Display) -> String {
    format!("File operation queue storage failed: {error}")
}
