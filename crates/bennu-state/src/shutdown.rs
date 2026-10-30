//! 关闭程序时的一次性提交：任务收尾、设置与浏览会话落在同一个事务里。
//! 它跨越三类存储，所以挂在共用的 `StateDatabase` 上，而不属于其中任何一类。

use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

use crate::browser_session::upsert_browser_session;
use crate::database::{current_time_ms, StateDatabase};
use crate::preferences_payload::write_user_preferences;
use crate::{
    StoreError, StoreResult, StoredBrowserSession, StoredInterruptedRecoverableTask,
    StoredTaskStatus,
};

#[derive(Debug, Clone, PartialEq)]
pub enum StoredBrowserSessionShutdown {
    Skip,
    Persist(StoredBrowserSession),
}

/// `T` 是应用的设置文档类型，存储层只把它序列化成设置行。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredApplicationShutdown<T> {
    pub browser_session: StoredBrowserSessionShutdown,
    pub user_preferences: Option<T>,
    pub interrupted_recoverable_tasks: Vec<StoredInterruptedRecoverableTask>,
    pub transient_task_ids: Vec<u64>,
}

impl StateDatabase {
    pub fn commit_application_shutdown<T: Serialize>(
        &self,
        shutdown: StoredApplicationShutdown<T>,
    ) -> StoreResult<()> {
        let StoredApplicationShutdown {
            browser_session,
            user_preferences,
            interrupted_recoverable_tasks,
            transient_task_ids,
        } = shutdown;
        let browser_session_json = match &browser_session {
            StoredBrowserSessionShutdown::Skip => None,
            StoredBrowserSessionShutdown::Persist(session) => Some(serde_json::to_string(session)?),
        };
        let mut connection = self.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        for task in interrupted_recoverable_tasks {
            if !matches!(
                task.status,
                StoredTaskStatus::RecoveryPending | StoredTaskStatus::Canceling
            ) || !task_has_recovery_rows(&transaction, task.task_id)?
            {
                return Err(StoreError::InvalidRecoverableOperation(
                    "shutdown recoverable task has no preserved recovery state",
                ));
            }
            let updated = transaction.execute(
                "UPDATE task_queue
                 SET status = ?1, progress_fraction = ?2, error = ?3, updated_at_ms = ?4
                 WHERE id = ?5",
                params![
                    task.status.as_str(),
                    task.progress.fraction,
                    task.error,
                    current_time_ms(),
                    task.task_id
                ],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidRecoverableOperation(
                    "shutdown recoverable task row is missing",
                ));
            }
        }

        for task_id in transient_task_ids {
            if task_has_recovery_rows(&transaction, task_id)? {
                return Err(StoreError::InvalidRecoverableOperation(
                    "transient shutdown task owns recovery state",
                ));
            }
            transaction.execute("DELETE FROM task_queue WHERE id = ?1", params![task_id])?;
        }

        if let Some(preferences) = &user_preferences {
            write_user_preferences(&transaction, self.path(), preferences)?;
        }
        if let Some(payload_json) = browser_session_json {
            upsert_browser_session(&transaction, &payload_json)?;
        }
        transaction.commit()?;
        Ok(())
    }
}

fn task_has_recovery_rows(
    transaction: &rusqlite::Transaction<'_>,
    task_id: u64,
) -> StoreResult<bool> {
    let has_rows = transaction.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM transfer_journal WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_manifest WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_replacement_manifest WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_merge_completion WHERE task_id = ?1)",
        params![task_id],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(has_rows)
}
