use std::path::Path;

use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::{
    current_time_ms, sqlite_id_to_u64, StoreError, StoreResult, StoredOperation, StoredPath,
    StoredProgress, StoredTaskStatus, StoredTransfer, TaskQueueStore,
};

pub const TRANSFER_JOURNAL_VERSION: u32 = 1;

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS transfer_journal (
    task_id INTEGER NOT NULL REFERENCES task_queue(id) ON DELETE CASCADE,
    transfer_index INTEGER NOT NULL,
    relative_path_json TEXT NOT NULL,
    operation_kind TEXT NOT NULL,
    source_path_json TEXT NOT NULL,
    requested_target_path_json TEXT NOT NULL,
    conflict_strategy TEXT NOT NULL,
    verification TEXT NOT NULL,
    checkpoint_kind TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (task_id, transfer_index, relative_path_json)
);

CREATE TABLE IF NOT EXISTS transfer_manifest (
    task_id INTEGER NOT NULL REFERENCES task_queue(id) ON DELETE CASCADE,
    transfer_index INTEGER NOT NULL,
    relative_path_json TEXT NOT NULL,
    identity_json TEXT NOT NULL,
    PRIMARY KEY (task_id, transfer_index, relative_path_json)
);

CREATE TABLE IF NOT EXISTS transfer_replacement_manifest (
    task_id INTEGER NOT NULL REFERENCES task_queue(id) ON DELETE CASCADE,
    transfer_index INTEGER NOT NULL,
    relative_path_json TEXT NOT NULL,
    identity_json TEXT NOT NULL,
    PRIMARY KEY (task_id, transfer_index, relative_path_json)
);

CREATE TABLE IF NOT EXISTS transfer_merge_completion (
    task_id INTEGER NOT NULL REFERENCES task_queue(id) ON DELETE CASCADE,
    transfer_index INTEGER NOT NULL,
    child_relative_path_json TEXT NOT NULL,
    completion_json TEXT NOT NULL,
    PRIMARY KEY (task_id, transfer_index, child_relative_path_json)
);
"#;

mod lease;
mod model;

use lease::{
    recoverable_restore_coordinator_lock_path, recoverable_task_runner_lock_path, try_acquire_lock,
};
pub use lease::{
    ClaimedRecoverableTask, RecoverableRestoreCoordinatorLease, RecoverableTaskRunnerLease,
};
pub use model::*;

/// One manifest + checkpoint install inside a batch commit. Every update moves
/// a distinct top-level record forward by exactly one revision.
#[derive(Debug, Clone)]
pub struct StoredManifestCheckpointBatchUpdate {
    pub task_id: u64,
    pub key: StoredTransferWorkKey,
    pub expected_revision: u64,
    pub manifest_entries: Vec<StoredManifestEntry>,
    pub replacement_manifest_entries: Vec<StoredManifestEntry>,
    pub checkpoint: StoredTransferCheckpoint,
}

#[derive(Clone, Copy)]
pub struct TransferManifestCheckpointUpdate<'a> {
    pub key: &'a StoredTransferWorkKey,
    pub expected_revision: u64,
    pub manifest_entries: &'a [StoredManifestEntry],
    pub replacement_manifest_entries: &'a [StoredManifestEntry],
    pub checkpoint: &'a StoredTransferCheckpoint,
}

/// One checkpoint compare-and-swap in a batch commit. Every swap moves a
/// distinct top-level transfer record forward by exactly one revision.
#[derive(Debug, Clone)]
pub struct StoredTransferCheckpointSwap {
    pub task_id: u64,
    pub key: StoredTransferWorkKey,
    pub expected_revision: u64,
    pub checkpoint: StoredTransferCheckpoint,
}

impl TaskQueueStore {
    #[cfg(test)]
    pub(crate) fn insert_recoverable_transfer_task(
        &self,
        operation: &StoredOperation,
    ) -> StoreResult<u64> {
        self.insert_claimed_recoverable_transfer_task(operation)
            .map(|claimed| claimed.task_id)
    }

    pub fn insert_claimed_recoverable_transfer_task(
        &self,
        operation: &StoredOperation,
    ) -> StoreResult<ClaimedRecoverableTask> {
        let seeds = recoverable_transfer_seeds(operation)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = current_time_ms();
        let payload_json = serde_json::to_string(operation)?;
        transaction.execute(
            "INSERT INTO task_queue (
                operation_kind, payload_json, status, progress_fraction, error,
                created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4)",
            params![
                operation.kind(),
                payload_json,
                StoredTaskStatus::Pending.as_str(),
                now
            ],
        )?;
        let sqlite_task_id = transaction.last_insert_rowid();
        let task_id = sqlite_id_to_u64(sqlite_task_id)?;
        let runner_lease = self
            .try_acquire_recoverable_task_runner(task_id)?
            .ok_or(StoreError::RecoverableTaskAlreadyRunning { task_id })?;

        for seed in seeds {
            let checkpoint = StoredTransferCheckpoint::awaiting_manifest();
            transaction.execute(
                "INSERT INTO transfer_journal (
                    task_id, transfer_index, relative_path_json, operation_kind,
                    source_path_json, requested_target_path_json, conflict_strategy,
                    verification, checkpoint_kind, checkpoint_json, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
                params![
                    sqlite_task_id,
                    sqlite_index(seed.transfer_index)?,
                    serde_json::to_string(&StoredPath::from_path(Path::new("")))?,
                    seed.operation.as_str(),
                    serde_json::to_string(&seed.transfer.source)?,
                    serde_json::to_string(&seed.transfer.target)?,
                    seed.transfer.conflict_strategy.as_str(),
                    seed.verification.as_str(),
                    checkpoint.kind.as_str(),
                    checkpoint.state_json,
                ],
            )?;
        }

        transaction.commit()?;
        Ok(ClaimedRecoverableTask {
            task_id,
            runner_lease,
        })
    }

    pub fn try_acquire_recoverable_task_runner(
        &self,
        task_id: u64,
    ) -> StoreResult<Option<RecoverableTaskRunnerLease>> {
        try_acquire_lock(recoverable_task_runner_lock_path(&self.db_path, task_id))
            .map(|lock_file| lock_file.map(|_lock_file| RecoverableTaskRunnerLease { _lock_file }))
    }

    pub fn try_acquire_recoverable_restore_coordinator(
        &self,
    ) -> StoreResult<Option<RecoverableRestoreCoordinatorLease>> {
        try_acquire_lock(recoverable_restore_coordinator_lock_path(&self.db_path)).map(
            |lock_file| {
                lock_file.map(|_lock_file| RecoverableRestoreCoordinatorLease { _lock_file })
            },
        )
    }

    pub fn read_transfer_recovery(
        &self,
        task_id: u64,
    ) -> StoreResult<StoredTransferRecoverySnapshot> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let sqlite_task_id = sqlite_index(task_id)?;
        let mut statement = transaction.prepare(
            "SELECT transfer_index, relative_path_json, operation_kind,
                    source_path_json, requested_target_path_json, conflict_strategy,
                    verification, checkpoint_kind, checkpoint_json, revision
             FROM transfer_journal
             WHERE task_id = ?1
             ORDER BY transfer_index ASC, relative_path_json ASC",
        )?;
        let rows = statement.query_map(params![sqlite_task_id], |row| {
            Ok(JournalRow {
                transfer_index: row.get(0)?,
                relative_path_json: row.get(1)?,
                operation_kind: row.get(2)?,
                source_path_json: row.get(3)?,
                requested_target_path_json: row.get(4)?,
                conflict_strategy: row.get(5)?,
                verification: row.get(6)?,
                checkpoint_kind: row.get(7)?,
                checkpoint_json: row.get(8)?,
                revision: row.get(9)?,
            })
        })?;
        let mut journal_entries = Vec::new();
        for row in rows {
            journal_entries.push(row?.try_into()?);
        }
        drop(statement);

        let mut statement = transaction.prepare(
            "SELECT transfer_index, relative_path_json, identity_json
             FROM transfer_manifest
             WHERE task_id = ?1
             ORDER BY transfer_index ASC, relative_path_json ASC",
        )?;
        let rows = statement.query_map(params![sqlite_task_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut manifest_entries = Vec::new();
        for row in rows {
            let (transfer_index, relative_path_json, identity_json) = row?;
            manifest_entries.push(StoredManifestEntry {
                transfer_index: sqlite_id_to_u64(transfer_index)?,
                relative_path: serde_json::from_str(&relative_path_json)?,
                identity: serde_json::from_str(&identity_json)?,
            });
        }
        drop(statement);

        let mut statement = transaction.prepare(
            "SELECT transfer_index, relative_path_json, identity_json
             FROM transfer_replacement_manifest
             WHERE task_id = ?1
             ORDER BY transfer_index ASC, relative_path_json ASC",
        )?;
        let rows = statement.query_map(params![sqlite_task_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut replacement_manifest_entries = Vec::new();
        for row in rows {
            let (transfer_index, relative_path_json, identity_json) = row?;
            replacement_manifest_entries.push(StoredManifestEntry {
                transfer_index: sqlite_id_to_u64(transfer_index)?,
                relative_path: serde_json::from_str(&relative_path_json)?,
                identity: serde_json::from_str(&identity_json)?,
            });
        }
        drop(statement);

        let mut statement = transaction.prepare(
            "SELECT transfer_index, child_relative_path_json, completion_json
             FROM transfer_merge_completion
             WHERE task_id = ?1
             ORDER BY transfer_index ASC, child_relative_path_json ASC",
        )?;
        let rows = statement.query_map(params![sqlite_task_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut merge_completions = Vec::new();
        for row in rows {
            let (transfer_index, child_relative_path_json, completion_json) = row?;
            merge_completions.push(StoredMergeChildCompletion {
                transfer_index: sqlite_id_to_u64(transfer_index)?,
                child_relative_path: serde_json::from_str(&child_relative_path_json)?,
                completion_json,
            });
        }
        drop(statement);
        transaction.commit()?;

        Ok(StoredTransferRecoverySnapshot {
            journal_entries,
            manifest_entries,
            replacement_manifest_entries,
            merge_completions,
        })
    }

    pub fn install_transfer_manifest_and_checkpoint(
        &self,
        task_id: u64,
        key: &StoredTransferWorkKey,
        expected_revision: u64,
        manifest_entries: &[StoredManifestEntry],
        checkpoint: &StoredTransferCheckpoint,
    ) -> StoreResult<u64> {
        self.install_transfer_manifests_and_checkpoint(
            task_id,
            key,
            expected_revision,
            manifest_entries,
            &[],
            checkpoint,
        )
    }

    pub fn install_transfer_manifests_and_checkpoint(
        &self,
        task_id: u64,
        key: &StoredTransferWorkKey,
        expected_revision: u64,
        manifest_entries: &[StoredManifestEntry],
        replacement_manifest_entries: &[StoredManifestEntry],
        checkpoint: &StoredTransferCheckpoint,
    ) -> StoreResult<u64> {
        let update = TransferManifestCheckpointUpdate {
            key,
            expected_revision,
            manifest_entries,
            replacement_manifest_entries,
            checkpoint,
        };
        Ok(self
            .install_transfer_manifests_and_checkpoint_while(task_id, update, || true)?
            .expect("unconditional manifest transaction cannot be interrupted"))
    }

    pub fn install_transfer_manifests_and_checkpoint_while(
        &self,
        task_id: u64,
        update: TransferManifestCheckpointUpdate<'_>,
        mut continue_transaction: impl FnMut() -> bool,
    ) -> StoreResult<Option<u64>> {
        let TransferManifestCheckpointUpdate {
            key,
            expected_revision,
            manifest_entries,
            replacement_manifest_entries,
            checkpoint,
        } = update;
        ensure_top_level_work_key(key)?;
        checkpoint.validate()?;
        if manifest_entries
            .iter()
            .chain(replacement_manifest_entries)
            .any(|entry| entry.transfer_index != key.transfer_index)
        {
            return Err(StoreError::InvalidRecoverableOperation(
                "manifest entry belongs to another transfer",
            ));
        }
        if !continue_transaction() {
            return Ok(None);
        }

        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_revision =
            expected_revision
                .checked_add(1)
                .ok_or(StoreError::InvalidRecoverableOperation(
                    "transfer revision overflow",
                ))?;
        let updated = update_checkpoint(
            &transaction,
            task_id,
            key,
            expected_revision,
            next_revision,
            checkpoint,
        )?;
        ensure_revision_updated(updated, task_id, key.transfer_index, expected_revision)?;

        transaction.execute(
            "DELETE FROM transfer_manifest WHERE task_id = ?1 AND transfer_index = ?2",
            params![sqlite_index(task_id)?, sqlite_index(key.transfer_index)?],
        )?;
        for entry in manifest_entries {
            if !continue_transaction() {
                return Ok(None);
            }
            transaction.execute(
                "INSERT INTO transfer_manifest (
                    task_id, transfer_index, relative_path_json, identity_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    sqlite_index(task_id)?,
                    sqlite_index(entry.transfer_index)?,
                    serde_json::to_string(&entry.relative_path)?,
                    serde_json::to_string(&entry.identity)?,
                ],
            )?;
        }
        transaction.execute(
            "DELETE FROM transfer_replacement_manifest
             WHERE task_id = ?1 AND transfer_index = ?2",
            params![sqlite_index(task_id)?, sqlite_index(key.transfer_index)?],
        )?;
        for entry in replacement_manifest_entries {
            if !continue_transaction() {
                return Ok(None);
            }
            transaction.execute(
                "INSERT INTO transfer_replacement_manifest (
                    task_id, transfer_index, relative_path_json, identity_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    sqlite_index(task_id)?,
                    sqlite_index(entry.transfer_index)?,
                    serde_json::to_string(&entry.relative_path)?,
                    serde_json::to_string(&entry.identity)?,
                ],
            )?;
        }
        if !continue_transaction() {
            return Ok(None);
        }
        transaction.commit()?;
        Ok(Some(next_revision))
    }

    pub fn install_transfer_manifests_and_checkpoints_batch(
        &self,
        updates: &[StoredManifestCheckpointBatchUpdate],
    ) -> StoreResult<Vec<u64>> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut revisions = Vec::with_capacity(updates.len());
        for update in updates {
            ensure_top_level_work_key(&update.key)?;
            update.checkpoint.validate()?;
            if update
                .manifest_entries
                .iter()
                .chain(update.replacement_manifest_entries.iter())
                .any(|entry| entry.transfer_index != update.key.transfer_index)
            {
                return Err(StoreError::InvalidRecoverableOperation(
                    "manifest entry belongs to another transfer",
                ));
            }
            let next_revision = update.expected_revision.checked_add(1).ok_or(
                StoreError::InvalidRecoverableOperation("transfer revision overflow"),
            )?;
            let updated = update_checkpoint(
                &transaction,
                update.task_id,
                &update.key,
                update.expected_revision,
                next_revision,
                &update.checkpoint,
            )?;
            ensure_revision_updated(
                updated,
                update.task_id,
                update.key.transfer_index,
                update.expected_revision,
            )?;
            transaction.execute(
                "DELETE FROM transfer_manifest WHERE task_id = ?1 AND transfer_index = ?2",
                params![
                    sqlite_index(update.task_id)?,
                    sqlite_index(update.key.transfer_index)?
                ],
            )?;
            for entry in &update.manifest_entries {
                transaction.execute(
                    "INSERT INTO transfer_manifest (
                        task_id, transfer_index, relative_path_json, identity_json
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        sqlite_index(update.task_id)?,
                        sqlite_index(entry.transfer_index)?,
                        serde_json::to_string(&entry.relative_path)?,
                        serde_json::to_string(&entry.identity)?,
                    ],
                )?;
            }
            transaction.execute(
                "DELETE FROM transfer_replacement_manifest
                 WHERE task_id = ?1 AND transfer_index = ?2",
                params![
                    sqlite_index(update.task_id)?,
                    sqlite_index(update.key.transfer_index)?
                ],
            )?;
            for entry in &update.replacement_manifest_entries {
                transaction.execute(
                    "INSERT INTO transfer_replacement_manifest (
                        task_id, transfer_index, relative_path_json, identity_json
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        sqlite_index(update.task_id)?,
                        sqlite_index(entry.transfer_index)?,
                        serde_json::to_string(&entry.relative_path)?,
                        serde_json::to_string(&entry.identity)?,
                    ],
                )?;
            }
            revisions.push(next_revision);
        }
        transaction.commit()?;
        Ok(revisions)
    }

    pub fn compare_and_swap_transfer_checkpoint(
        &self,
        task_id: u64,
        key: &StoredTransferWorkKey,
        expected_revision: u64,
        checkpoint: &StoredTransferCheckpoint,
    ) -> StoreResult<u64> {
        ensure_top_level_work_key(key)?;
        checkpoint.validate()?;
        let next_revision =
            expected_revision
                .checked_add(1)
                .ok_or(StoreError::InvalidRecoverableOperation(
                    "transfer revision overflow",
                ))?;
        let connection = self.connection()?;
        let updated = update_checkpoint(
            &connection,
            task_id,
            key,
            expected_revision,
            next_revision,
            checkpoint,
        )?;
        ensure_revision_updated(updated, task_id, key.transfer_index, expected_revision)?;
        Ok(next_revision)
    }

    pub fn compare_and_swap_transfer_checkpoints(
        &self,
        swaps: &[StoredTransferCheckpointSwap],
    ) -> StoreResult<Vec<u64>> {
        if swaps.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut revisions = Vec::with_capacity(swaps.len());
        for swap in swaps {
            ensure_top_level_work_key(&swap.key)?;
            swap.checkpoint.validate()?;
            let next_revision = swap.expected_revision.checked_add(1).ok_or(
                StoreError::InvalidRecoverableOperation("transfer revision overflow"),
            )?;
            let updated = update_checkpoint(
                &transaction,
                swap.task_id,
                &swap.key,
                swap.expected_revision,
                next_revision,
                &swap.checkpoint,
            )?;
            ensure_revision_updated(
                updated,
                swap.task_id,
                swap.key.transfer_index,
                swap.expected_revision,
            )?;
            revisions.push(next_revision);
        }
        transaction.commit()?;
        Ok(revisions)
    }

    pub fn compare_and_swap_transfer_merge_completion(
        &self,
        task_id: u64,
        key: &StoredTransferWorkKey,
        expected_revision: u64,
        completion: &StoredMergeChildCompletion,
        checkpoint: &StoredTransferCheckpoint,
    ) -> StoreResult<u64> {
        ensure_top_level_work_key(key)?;
        validate_merge_completion(key, completion)?;
        serde_json::from_str::<serde_json::Value>(&completion.completion_json)?;
        checkpoint.validate()?;
        if checkpoint.kind != StoredTransferCheckpointKind::Merging {
            return Err(StoreError::InvalidRecoverableOperation(
                "merge completion requires a merging checkpoint",
            ));
        }
        let next_revision =
            expected_revision
                .checked_add(1)
                .ok_or(StoreError::InvalidRecoverableOperation(
                    "transfer revision overflow",
                ))?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let updated = update_checkpoint(
            &transaction,
            task_id,
            key,
            expected_revision,
            next_revision,
            checkpoint,
        )?;
        ensure_revision_updated(updated, task_id, key.transfer_index, expected_revision)?;
        transaction.execute(
            "INSERT INTO transfer_merge_completion (
                task_id, transfer_index, child_relative_path_json, completion_json
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                sqlite_index(task_id)?,
                sqlite_index(key.transfer_index)?,
                serde_json::to_string(&completion.child_relative_path)?,
                completion.completion_json,
            ],
        )?;
        transaction.commit()?;
        Ok(next_revision)
    }

    pub fn finalize_recoverable_transfer_task(
        &self,
        task_id: u64,
        status: StoredTaskStatus,
        progress: StoredProgress,
        error: Option<&str>,
    ) -> StoreResult<()> {
        if !matches!(
            status,
            StoredTaskStatus::Completed | StoredTaskStatus::Canceled | StoredTaskStatus::Failed
        ) {
            return Err(StoreError::InvalidRecoverableOperation(
                "only completed, canceled, or failed transfers can discard recovery details",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sqlite_task_id = sqlite_index(task_id)?;
        let journal_rows: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM transfer_journal WHERE task_id = ?1",
            params![sqlite_task_id],
            |row| row.get(0),
        )?;
        let unfinished_rows: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM transfer_journal
             WHERE task_id = ?1
               AND checkpoint_kind NOT IN ('completed', 'canceled', 'failed', 'skipped')",
            params![sqlite_task_id],
            |row| row.get(0),
        )?;
        if journal_rows == 0 || unfinished_rows != 0 {
            return Err(StoreError::InvalidRecoverableOperation(
                "transfer recovery details are not terminal",
            ));
        }
        transaction.execute(
            "UPDATE task_queue
             SET status = ?1, progress_fraction = ?2, error = ?3, updated_at_ms = ?4
             WHERE id = ?5",
            params![
                status.as_str(),
                progress.fraction,
                error,
                current_time_ms(),
                sqlite_task_id
            ],
        )?;
        transaction.execute(
            "DELETE FROM transfer_manifest WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_replacement_manifest WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_merge_completion WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_journal WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_transfer_recovery(&self, task_id: u64) -> StoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sqlite_task_id = sqlite_index(task_id)?;
        transaction.execute(
            "DELETE FROM transfer_manifest WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_replacement_manifest WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_merge_completion WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.execute(
            "DELETE FROM transfer_journal WHERE task_id = ?1",
            params![sqlite_task_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn validate_merge_completion(
    journal_key: &StoredTransferWorkKey,
    completion: &StoredMergeChildCompletion,
) -> StoreResult<()> {
    if completion.transfer_index != journal_key.transfer_index
        || completion
            .child_relative_path
            .to_path_buf()
            .as_os_str()
            .is_empty()
    {
        return Err(StoreError::InvalidRecoverableOperation(
            "merge completion does not belong to the journal transfer",
        ));
    }
    Ok(())
}

fn ensure_top_level_work_key(key: &StoredTransferWorkKey) -> StoreResult<()> {
    if key.relative_path.to_path_buf().as_os_str().is_empty() {
        Ok(())
    } else {
        Err(StoreError::InvalidRecoverableOperation(
            "SQLite transfer journal only stores top-level work keys",
        ))
    }
}

fn update_checkpoint(
    connection: &rusqlite::Connection,
    task_id: u64,
    key: &StoredTransferWorkKey,
    expected_revision: u64,
    next_revision: u64,
    checkpoint: &StoredTransferCheckpoint,
) -> StoreResult<usize> {
    Ok(connection.execute(
        "UPDATE transfer_journal
         SET checkpoint_kind = ?1, checkpoint_json = ?2, revision = ?3
         WHERE task_id = ?4 AND transfer_index = ?5
           AND relative_path_json = ?6 AND revision = ?7",
        params![
            checkpoint.kind.as_str(),
            checkpoint.state_json,
            sqlite_index(next_revision)?,
            sqlite_index(task_id)?,
            sqlite_index(key.transfer_index)?,
            serde_json::to_string(&key.relative_path)?,
            sqlite_index(expected_revision)?,
        ],
    )?)
}

fn ensure_revision_updated(
    updated: usize,
    task_id: u64,
    transfer_index: u64,
    expected_revision: u64,
) -> StoreResult<()> {
    if updated == 1 {
        return Ok(());
    }
    Err(StoreError::StaleTransferRevision {
        task_id,
        transfer_index,
        expected_revision,
    })
}

struct JournalRow {
    transfer_index: i64,
    relative_path_json: String,
    operation_kind: String,
    source_path_json: String,
    requested_target_path_json: String,
    conflict_strategy: String,
    verification: String,
    checkpoint_kind: String,
    checkpoint_json: String,
    revision: i64,
}

impl TryFrom<JournalRow> for StoredTransferJournalEntry {
    type Error = StoreError;

    fn try_from(row: JournalRow) -> StoreResult<Self> {
        Ok(Self {
            key: StoredTransferWorkKey {
                transfer_index: sqlite_id_to_u64(row.transfer_index)?,
                relative_path: serde_json::from_str(&row.relative_path_json)?,
            },
            operation: StoredTransferOperation::parse(row.operation_kind)?,
            source: serde_json::from_str(&row.source_path_json)?,
            requested_target: serde_json::from_str(&row.requested_target_path_json)?,
            conflict_strategy: StoredTransferConflictStrategy::parse(row.conflict_strategy)?,
            verification: StoredFileOperationVerification::parse(row.verification)?,
            checkpoint: StoredTransferCheckpoint::new(
                StoredTransferCheckpointKind::parse(row.checkpoint_kind)?,
                row.checkpoint_json,
            )?,
            revision: sqlite_id_to_u64(row.revision)?,
        })
    }
}

fn sqlite_index(value: u64) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidRecoverableOperation("value exceeds SQLite integer"))
}

fn invalid_transfer_value(field: &'static str, value: String) -> StoreError {
    StoreError::InvalidTransferValue { field, value }
}

#[cfg(test)]
#[path = "recoverable_transfer/tests.rs"]
mod tests;
