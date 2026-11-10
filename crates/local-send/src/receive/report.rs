//! 一次接收的结果：每个文件怎样了（收下的落在哪、没收成的原因、被取消的），传输记录
//! 逐个文件记，任务面板与通知按它数。

use std::path::PathBuf;

use super::{FileSlot, SlotOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveReport {
    pub files: Vec<ReceivedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFile {
    /// 对方发来的名字（文件夹里的是 `文件夹名/相对路径`）。
    pub file_name: String,
    pub size: u64,
    pub outcome: ReceivedOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivedOutcome {
    /// 落盘位置（重名时改过的名字、同名文件夹改名后的目录）。
    Saved(PathBuf),
    /// 英文原因。
    Failed(String),
    /// 会话被结束（对方取消或本机取消）时还没收完；半成品已删。
    Canceled,
}

impl ReceiveReport {
    pub fn saved_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter_map(|file| match &file.outcome {
                ReceivedOutcome::Saved(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    /// 没收成的：(文件名, 原因)；取消的不算。
    pub fn failures(&self) -> Vec<(String, String)> {
        self.files
            .iter()
            .filter_map(|file| match &file.outcome {
                ReceivedOutcome::Failed(reason) => Some((file.file_name.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn canceled(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.outcome == ReceivedOutcome::Canceled)
    }
}

impl ReceivedFile {
    /// 文件槽的终态；还在待收的就是被取消的。
    pub(super) fn from_slot(slot: &FileSlot) -> Self {
        let outcome = match (&slot.outcome, &slot.dest) {
            (SlotOutcome::Done, Some(dest)) => ReceivedOutcome::Saved(dest.clone()),
            (SlotOutcome::Done, None) => ReceivedOutcome::Failed("incomplete".to_owned()),
            (SlotOutcome::Failed(reason), _) => ReceivedOutcome::Failed((*reason).to_owned()),
            (SlotOutcome::Pending, _) => ReceivedOutcome::Canceled,
        };
        Self {
            file_name: slot.meta.file_name.clone(),
            size: slot.meta.size,
            outcome,
        }
    }
}
