//! 超级输入框执行过的命令最后一次的结果：同一条命令只留最新一次，一共只留最近若干条。
//! 内容的含义归 app-ui；这里只存结构化记录，读不懂的一条跳过、不删，之后挤出去时自然删掉。

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::database::StateDatabase;
use crate::StoreResult;

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS command_outputs (
    command TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCommandOutput {
    /// 框里打的命令（不带模式前缀）。
    pub command: String,
    /// Unix 毫秒。
    pub at_ms: i64,
    pub result: StoredCommandResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredCommandResult {
    /// 程序退出了：怎么退出的，和它终端里最后的输出。
    Exited { exit: StoredExit, output: String },
    /// 等不到它的退出情况：给用户看的原因。
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredExit {
    Code(u32),
    /// 被信号结束：信号的名字。
    Signal(String),
}

#[derive(Debug, Clone)]
pub struct CommandOutputStore {
    database: StateDatabase,
}

impl From<StateDatabase> for CommandOutputStore {
    fn from(database: StateDatabase) -> Self {
        Self { database }
    }
}

impl CommandOutputStore {
    pub fn new(db_path: impl AsRef<Path>) -> StoreResult<Self> {
        StateDatabase::open(db_path).map(Self::from)
    }

    /// 记下一条命令这次的结果（替换它上一次的），只留最近 `keep` 条。
    pub fn save(&self, record: &StoredCommandOutput, keep: usize) -> StoreResult<()> {
        let mut connection = self.database.write();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO command_outputs (command, payload_json, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(command) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 updated_at_ms = excluded.updated_at_ms",
            params![record.command, serde_json::to_string(record)?, record.at_ms],
        )?;
        transaction.execute(
            "DELETE FROM command_outputs WHERE command NOT IN
                 (SELECT command FROM command_outputs ORDER BY updated_at_ms DESC LIMIT ?1)",
            params![i64::try_from(keep).unwrap_or(i64::MAX)],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// 最近的在前；读不懂的跳过。
    pub fn read(&self) -> StoreResult<Vec<StoredCommandOutput>> {
        let connection = self.database.read();
        let mut statement = connection
            .prepare("SELECT payload_json FROM command_outputs ORDER BY updated_at_ms DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for payload in rows {
            if let Ok(record) = serde_json::from_str(&payload?) {
                records.push(record);
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CommandOutputStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CommandOutputStore::new(dir.path().join("state.sqlite")).unwrap();
        (dir, store)
    }

    fn record(command: &str, at_ms: i64, output: &str) -> StoredCommandOutput {
        StoredCommandOutput {
            command: command.to_owned(),
            at_ms,
            result: StoredCommandResult::Exited {
                exit: StoredExit::Code(0),
                output: output.to_owned(),
            },
        }
    }

    #[test]
    fn a_command_keeps_only_its_latest_result_and_old_ones_are_dropped() {
        let (_dir, store) = store();
        store.save(&record("ls", 1, "a"), 2).unwrap();
        store.save(&record("pwd", 2, "/w"), 2).unwrap();
        store.save(&record("ls", 3, "a b"), 2).unwrap();
        assert_eq!(
            store.read().unwrap(),
            [record("ls", 3, "a b"), record("pwd", 2, "/w")]
        );

        store
            .save(
                &StoredCommandOutput {
                    command: "make".to_owned(),
                    at_ms: 4,
                    result: StoredCommandResult::Exited {
                        exit: StoredExit::Signal("Terminated".to_owned()),
                        output: String::new(),
                    },
                },
                2,
            )
            .unwrap();
        let commands = store
            .read()
            .unwrap()
            .into_iter()
            .map(|record| record.command)
            .collect::<Vec<_>>();
        assert_eq!(commands, ["make", "ls"]);
    }

    #[test]
    fn an_unreadable_row_is_skipped() {
        let (_dir, store) = store();
        store.save(&record("ls", 1, "a"), 5).unwrap();
        store
            .database
            .write()
            .execute(
                "INSERT INTO command_outputs VALUES ('bad', 'not json', 2)",
                [],
            )
            .unwrap();
        assert_eq!(store.read().unwrap(), [record("ls", 1, "a")]);
    }
}
