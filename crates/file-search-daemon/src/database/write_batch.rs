use std::ops::Deref;

use rusqlite::Connection;

use crate::error::SearchResult;

use super::SearchDatabase;

/// 单条写命令的边界。写库线程可能正开着一批还没提交的写入（见 `writer.rs`），
/// 所以这里用 SAVEPOINT 而不是事务：命令失败只撤销它自己，同批已成功的写入保留。
/// 没有外层事务时 SAVEPOINT 自成一个事务，RELEASE 即提交。
pub(super) struct WriteSavepoint<'connection> {
    connection: &'connection Connection,
    released: bool,
}

impl<'connection> WriteSavepoint<'connection> {
    pub(super) fn begin(connection: &'connection Connection) -> SearchResult<Self> {
        connection.execute_batch("SAVEPOINT search_write")?;
        Ok(Self {
            connection,
            released: false,
        })
    }

    pub(super) fn release(mut self) -> SearchResult<()> {
        self.connection.execute_batch("RELEASE search_write")?;
        self.released = true;
        Ok(())
    }
}

impl Deref for WriteSavepoint<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.connection
    }
}

impl Drop for WriteSavepoint<'_> {
    fn drop(&mut self) {
        if !self.released {
            // 被中断的写语句会让 SQLite 回滚整个外层事务，此时保存点已不存在，失败可以忽略。
            let _ = self
                .connection
                .execute_batch("ROLLBACK TO search_write; RELEASE search_write");
        }
    }
}

impl SearchDatabase {
    pub(crate) fn begin_write_batch(&self) -> SearchResult<()> {
        self.connection.execute_batch("BEGIN")?;
        Ok(())
    }

    pub(crate) fn commit_write_batch(&self) -> SearchResult<()> {
        if let Err(error) = self.connection.execute_batch("COMMIT") {
            // COMMIT 失败后 SQLite 可能仍留着事务（例如 SQLITE_BUSY）。整批作废，
            // 由拿到这个失败的维护工作按现有失败重查补回。
            if !self.connection.is_autocommit() {
                let _ = self.connection.execute_batch("ROLLBACK");
            }
            return Err(error.into());
        }
        Ok(())
    }

    /// 被中断的写语句会让 SQLite 自动回滚整个事务，批还开没开着只能问连接本身。
    pub(crate) fn write_batch_is_open(&self) -> bool {
        !self.connection.is_autocommit()
    }

    pub(crate) fn total_changes(&self) -> u64 {
        self.connection.total_changes()
    }
}
