//! LocalSend 传输记录：收发过的文件与文字，每条一行 JSON，新的在后（行号递增）。
//!
//! 只留最近若干条：追加时在同一个事务里删掉更早的。记录内容的含义归 app-ui（谁发的、
//! 结果怎么显示）；这里只保存结构化记录，读不懂的一条跳过、不删（读没有副作用），
//! 之后追加挤出去时自然删掉。

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::database::StateDatabase;
use crate::{StoreResult, StoredPath};

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS transfer_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    payload_json TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransferRecord {
    /// app-ui 生成的唯一号：启动读入时与读入前已产生的记录去重。
    pub record_id: String,
    pub direction: StoredTransferDirection,
    /// 对方设备的名字（记下时的）。
    pub device: String,
    pub item: StoredTransferItem,
    /// Unix 毫秒。
    pub at_ms: i64,
    pub result: StoredTransferResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredTransferDirection {
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredTransferItem {
    /// 一个文件；`more` > 0 时是整次没传成的一条（"照片 等 N 个"），`size` 可能未知。
    File {
        name: String,
        path: Option<StoredPath>,
        size: Option<u64>,
        #[serde(default)]
        more: u64,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredTransferResult {
    Done,
    Canceled,
    Declined,
    TooManyPinAttempts,
    Failed { reason: String },
}

#[derive(Debug, Clone)]
pub struct TransferHistoryStore {
    database: StateDatabase,
}

impl From<StateDatabase> for TransferHistoryStore {
    fn from(database: StateDatabase) -> Self {
        Self { database }
    }
}

/// 读出的记录：新的在前；`unreadable` 是跳过的读不懂的条数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferHistoryRead {
    pub records: Vec<StoredTransferRecord>,
    pub unreadable: usize,
}

impl TransferHistoryStore {
    pub fn new(db_path: impl AsRef<Path>) -> StoreResult<Self> {
        StateDatabase::open(db_path).map(Self::from)
    }

    /// 按给的先后追加（先给的旧），然后只留最新的 `keep` 条。
    pub fn append(&self, records: &[StoredTransferRecord], keep: usize) -> StoreResult<()> {
        let mut connection = self.database.write();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for record in records {
            transaction.execute(
                "INSERT INTO transfer_history (payload_json) VALUES (?1)",
                params![serde_json::to_string(record)?],
            )?;
        }
        transaction.execute(
            "DELETE FROM transfer_history WHERE id NOT IN
                 (SELECT id FROM transfer_history ORDER BY id DESC LIMIT ?1)",
            params![i64::try_from(keep).unwrap_or(i64::MAX)],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// 最新的 `limit` 条，新的在前。
    pub fn read(&self, limit: usize) -> StoreResult<TransferHistoryRead> {
        let connection = self.database.read();
        let mut statement = connection
            .prepare("SELECT payload_json FROM transfer_history ORDER BY id DESC LIMIT ?1")?;
        let rows = statement
            .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                row.get::<_, String>(0)
            })?;
        let mut read = TransferHistoryRead {
            records: Vec::new(),
            unreadable: 0,
        };
        for payload in rows {
            match serde_json::from_str(&payload?) {
                Ok(record) => read.records.push(record),
                Err(_) => read.unreadable += 1,
            }
        }
        Ok(read)
    }

    pub fn clear(&self) -> StoreResult<()> {
        self.database
            .write()
            .execute("DELETE FROM transfer_history", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, TransferHistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TransferHistoryStore::new(dir.path().join("state.sqlite")).unwrap();
        (dir, store)
    }

    fn record(id: &str) -> StoredTransferRecord {
        StoredTransferRecord {
            record_id: id.to_owned(),
            direction: StoredTransferDirection::Received,
            device: "Phone".to_owned(),
            item: StoredTransferItem::File {
                name: format!("{id}.jpg"),
                path: Some(StoredPath::from_path(Path::new("/tmp/a.jpg"))),
                size: Some(12),
                more: 0,
            },
            at_ms: 1,
            result: StoredTransferResult::Failed {
                reason: "write failed".to_owned(),
            },
        }
    }

    fn ids(read: &TransferHistoryRead) -> Vec<&str> {
        read.records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect()
    }

    #[test]
    fn records_come_back_newest_first_and_only_the_latest_are_kept() {
        let (_dir, store) = store();

        store.append(&[record("a"), record("b")], 3).unwrap();
        store.append(&[record("c"), record("d")], 3).unwrap();

        let read = store.read(10).unwrap();
        assert_eq!(ids(&read), ["d", "c", "b"]);
        assert_eq!(read.records[0], record("d"));
        assert_eq!(ids(&store.read(2).unwrap()), ["d", "c"]);
    }

    #[test]
    fn an_unreadable_record_is_skipped_and_left_alone() {
        let (_dir, store) = store();
        store.append(&[record("a")], 10).unwrap();
        store
            .database
            .write()
            .execute(
                "INSERT INTO transfer_history (payload_json) VALUES ('not json')",
                [],
            )
            .unwrap();
        store.append(&[record("b")], 10).unwrap();

        let read = store.read(10).unwrap();

        assert_eq!(ids(&read), ["b", "a"]);
        assert_eq!(read.unreadable, 1);
        assert_eq!(store.read(10).unwrap().unreadable, 1);
    }

    #[test]
    fn clearing_removes_everything() {
        let (_dir, store) = store();
        store.append(&[record("a"), record("b")], 10).unwrap();

        store.clear().unwrap();

        assert!(store.read(10).unwrap().records.is_empty());
    }

    #[test]
    fn text_and_whole_send_records_round_trip() {
        let (_dir, store) = store();
        let text = StoredTransferRecord {
            direction: StoredTransferDirection::Sent,
            item: StoredTransferItem::Text {
                text: "今晚 8 点".to_owned(),
            },
            result: StoredTransferResult::Done,
            ..record("t")
        };
        let whole = StoredTransferRecord {
            item: StoredTransferItem::File {
                name: "照片".to_owned(),
                path: None,
                size: None,
                more: 119,
            },
            result: StoredTransferResult::Declined,
            ..record("w")
        };

        store.append(&[text.clone(), whole.clone()], 10).unwrap();

        assert_eq!(store.read(10).unwrap().records, [whole, text]);
    }
}
