//! LocalSend 传输记录：收发过的文件（一个文件一条，同官方）与文字，存在 `state.sqlite`
//! 里，最多 200 条，新的在前。
//!
//! 内存里的列表是界面读的；每次产生记录都同时写库（`record_history` 是唯一入口）。
//! 启动时从库里读入，与读入前就已产生的按记录号合并去重，不丢不重。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bennu_state::{
    StoredPath, StoredTransferDirection, StoredTransferItem, StoredTransferRecord,
    StoredTransferResult, TransferHistoryStore,
};
use iced::{window, Task};
use local_send::{FileOutcome, ReceiveReport, ReceivedOutcome, SendReport};

use super::super::FileBrowser;
use super::{IncomingMessage, MessageTitle, TransferMessage};
use crate::model::Message;

/// 最多留几条：满了挤掉最早的。
pub(crate) const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Default)]
pub(crate) struct TransferHistory {
    /// 新的在前。
    pub(crate) records: Vec<HistoryRecord>,
    /// 本次运行产生的记录编号：与时间、进程号一起保证记录号不重。
    next_sequence: u64,
    /// 状态库就绪之前产生的（旧的在前）：随启动读入一起先写进去。
    unsaved: Vec<StoredTransferRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryRecord {
    pub(crate) id: String,
    pub(crate) direction: Direction,
    pub(crate) device: String,
    pub(crate) item: HistoryItem,
    pub(crate) at: SystemTime,
    pub(crate) result: HistoryResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryItem {
    /// 一个文件；`more` > 0 是整次没传成的一条（"照片 等 N 个"），点了不动。
    File {
        name: String,
        path: Option<PathBuf>,
        size: Option<u64>,
        more: u64,
    },
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryResult {
    Done,
    Canceled,
    Declined,
    TooManyPinAttempts,
    /// 英文原因。
    Failed(String),
}

/// 一条记录的内容（还没有编号与时间）。
pub(crate) struct NewRecord {
    pub(crate) direction: Direction,
    pub(crate) device: String,
    pub(crate) item: HistoryItem,
    pub(crate) result: HistoryResult,
}

impl NewRecord {
    /// 对方接受之后的一次发送：每个文件一条。
    pub(crate) fn per_sent_file(device: &str, report: &SendReport) -> Vec<Self> {
        report
            .files
            .iter()
            .map(|file| Self {
                direction: Direction::Sent,
                device: device.to_owned(),
                item: HistoryItem::File {
                    name: file.file_name.clone(),
                    path: file.path.clone(),
                    size: Some(file.size),
                    more: 0,
                },
                result: match &file.outcome {
                    FileOutcome::Sent => HistoryResult::Done,
                    FileOutcome::Failed(reason) => HistoryResult::Failed(reason.clone()),
                    FileOutcome::Canceled => HistoryResult::Canceled,
                },
            })
            .collect()
    }

    /// 一次接收：每个文件一条（收下的记落盘位置）。
    pub(crate) fn per_received_file(device: &str, report: &ReceiveReport) -> Vec<Self> {
        report
            .files
            .iter()
            .map(|file| {
                let (path, result) = match &file.outcome {
                    ReceivedOutcome::Saved(path) => (Some(path.clone()), HistoryResult::Done),
                    ReceivedOutcome::Failed(reason) => {
                        (None, HistoryResult::Failed(reason.clone()))
                    }
                    ReceivedOutcome::Canceled => (None, HistoryResult::Canceled),
                };
                Self {
                    direction: Direction::Received,
                    device: device.to_owned(),
                    item: HistoryItem::File {
                        name: file.file_name.clone(),
                        path,
                        size: Some(file.size),
                        more: 0,
                    },
                    result,
                }
            })
            .collect()
    }
}

impl HistoryRecord {
    fn to_stored(&self) -> StoredTransferRecord {
        StoredTransferRecord {
            record_id: self.id.clone(),
            direction: match self.direction {
                Direction::Sent => StoredTransferDirection::Sent,
                Direction::Received => StoredTransferDirection::Received,
            },
            device: self.device.clone(),
            item: match &self.item {
                HistoryItem::File {
                    name,
                    path,
                    size,
                    more,
                } => StoredTransferItem::File {
                    name: name.clone(),
                    path: path.as_deref().map(StoredPath::from_path),
                    size: *size,
                    more: *more,
                },
                HistoryItem::Text(text) => StoredTransferItem::Text { text: text.clone() },
            },
            at_ms: self.at.duration_since(UNIX_EPOCH).map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            }),
            result: match &self.result {
                HistoryResult::Done => StoredTransferResult::Done,
                HistoryResult::Canceled => StoredTransferResult::Canceled,
                HistoryResult::Declined => StoredTransferResult::Declined,
                HistoryResult::TooManyPinAttempts => StoredTransferResult::TooManyPinAttempts,
                HistoryResult::Failed(reason) => StoredTransferResult::Failed {
                    reason: reason.clone(),
                },
            },
        }
    }

    fn from_stored(stored: StoredTransferRecord) -> Self {
        Self {
            id: stored.record_id,
            direction: match stored.direction {
                StoredTransferDirection::Sent => Direction::Sent,
                StoredTransferDirection::Received => Direction::Received,
            },
            device: stored.device,
            item: match stored.item {
                StoredTransferItem::File {
                    name,
                    path,
                    size,
                    more,
                } => HistoryItem::File {
                    name,
                    path: path.map(|path| path.to_path_buf()),
                    size,
                    more,
                },
                StoredTransferItem::Text { text } => HistoryItem::Text(text),
            },
            at: UNIX_EPOCH + Duration::from_millis(u64::try_from(stored.at_ms).unwrap_or(0)),
            result: match stored.result {
                StoredTransferResult::Done => HistoryResult::Done,
                StoredTransferResult::Canceled => HistoryResult::Canceled,
                StoredTransferResult::Declined => HistoryResult::Declined,
                StoredTransferResult::TooManyPinAttempts => HistoryResult::TooManyPinAttempts,
                StoredTransferResult::Failed { reason } => HistoryResult::Failed(reason),
            },
        }
    }
}

impl TransferHistory {
    /// 放进内存（新的在前，同一批按给的先后），返回要写库的记录（旧的在前）。
    fn add(&mut self, records: Vec<NewRecord>) -> Vec<HistoryRecord> {
        let at = SystemTime::now();
        let stamp = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let records: Vec<HistoryRecord> = records
            .into_iter()
            .map(|record| {
                self.next_sequence += 1;
                HistoryRecord {
                    id: format!("{stamp:x}-{:x}-{}", std::process::id(), self.next_sequence),
                    direction: record.direction,
                    device: record.device,
                    item: record.item,
                    at,
                    result: record.result,
                }
            })
            .collect();
        self.records.splice(0..0, records.iter().cloned());
        self.records.truncate(HISTORY_LIMIT);
        records.into_iter().rev().collect()
    }

    /// 启动读入：与读入前已产生的合并（按记录号去重），新的在前，截到上限。
    fn merge_loaded(&mut self, loaded: Vec<HistoryRecord>) {
        for record in loaded {
            if !self.records.iter().any(|existing| existing.id == record.id) {
                self.records.push(record);
            }
        }
        // 稳定排序：同一时刻的一批保持原来的先后。
        self.records.sort_by(|a, b| b.at.cmp(&a.at));
        self.records.truncate(HISTORY_LIMIT);
    }
}

impl FileBrowser {
    /// 产生记录的唯一入口：放进内存并写库。
    pub(super) fn record_history(&mut self, records: Vec<NewRecord>) -> Task<Message> {
        if records.is_empty() {
            return Task::none();
        }
        let stored: Vec<StoredTransferRecord> = self
            .transfer_history
            .add(records)
            .iter()
            .map(HistoryRecord::to_stored)
            .collect();
        let Some(store) = self.history_store() else {
            self.transfer_history.unsaved.extend(stored);
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || store.append(&stored, HISTORY_LIMIT))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|written| written.map_err(|error| error.to_string()))
            },
            |outcome| Message::Transfer(TransferMessage::HistorySaved(outcome.err())),
        )
    }

    /// 启动时读入（状态库就绪之后）：先写进就绪前产生的，再读，同一个后台任务里依次做。
    pub(in crate::app) fn load_transfer_history(&mut self) -> Task<Message> {
        let Some(store) = self.history_store() else {
            return Task::none();
        };
        let unsaved = std::mem::take(&mut self.transfer_history.unsaved);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    store.append(&unsaved, HISTORY_LIMIT)?;
                    store.read(HISTORY_LIMIT)
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|read| read.map_err(|error| error.to_string()))
                .map(|read| {
                    read.records
                        .into_iter()
                        .map(HistoryRecord::from_stored)
                        .collect()
                })
            },
            |outcome| Message::Transfer(TransferMessage::HistoryLoaded(outcome)),
        )
    }

    pub(super) fn accept_history_loaded(&mut self, outcome: Result<Vec<HistoryRecord>, String>) {
        match outcome {
            Ok(records) => self.transfer_history.merge_loaded(records),
            // 读不出来不拦着用：本次产生的照样记、照样写。
            Err(error) => tracing::warn!(
                target: "app_ui::transfer",
                event = "localsend_history_load_failed",
                %error,
                "could not read the LocalSend transfer history"
            ),
        }
    }

    pub(super) fn clear_transfer_history(&mut self) -> Task<Message> {
        self.transfer_history.records.clear();
        let Some(store) = self.history_store() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || store.clear())
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|cleared| cleared.map_err(|error| error.to_string()))
            },
            |outcome| Message::Transfer(TransferMessage::HistorySaved(outcome.err())),
        )
    }

    /// 点一条：文件 → 主窗口新标签开它所在的文件夹并选中；文字 → 主窗口再弹一次。
    pub(super) fn open_history_record(&mut self, id: &str) -> Task<Message> {
        let Some(record) = self
            .transfer_history
            .records
            .iter()
            .find(|record| record.id == id)
            .cloned()
        else {
            return Task::none();
        };
        let opened = match record.item {
            HistoryItem::Text(text) => {
                let title = match record.direction {
                    Direction::Sent => MessageTitle::Sent(record.device),
                    Direction::Received => MessageTitle::Received(record.device),
                };
                self.show_message(IncomingMessage {
                    title,
                    text,
                    copied: false,
                })
            }
            HistoryItem::File {
                path: Some(path),
                more: 0,
                ..
            } => {
                let Some(parent) = path.parent().filter(|_| path.exists()) else {
                    self.show_global_error(tr!("The file is no longer there: %1", path.display()));
                    return Task::none();
                };
                let open = self.open_directory_in_new_tab(parent.to_path_buf());
                self.select_path(path);
                open
            }
            // 整次没传成的、没有落盘位置的：没东西可开。
            HistoryItem::File { .. } => return Task::none(),
        };
        self.focused_window = self.main_window;
        Task::batch([opened, window::gain_focus(self.main_window)])
    }

    fn history_store(&self) -> Option<TransferHistoryStore> {
        self.operation_queue
            .state_database()
            .map(TransferHistoryStore::from)
    }
}

#[cfg(test)]
mod tests;
