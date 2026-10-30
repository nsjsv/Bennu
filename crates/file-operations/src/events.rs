//! 执行器在运行中向界面汇报的事件。执行器不认识界面的消息类型:
//! 界面自己把事件映射成消息,终态由执行函数的返回值给出,不走这条通道。

use std::path::PathBuf;

use file_core::TransferWorkKey;

use crate::outcome::CompletedTransfer;
use crate::progress::{FileOperationProgressUpdate, TransferEntrySnapshot};

/// 通道满时发送方会等界面取走:列表增量、路径迁移和搜索刷新都靠这些事件,
/// 丢了就再也补不回来。
pub const FILE_OPERATION_EVENT_CAPACITY: usize = 32;

pub type FileOperationEventSender = tokio::sync::mpsc::Sender<FileOperationEvent>;

/// 已经写进恢复日志的直接移动提交点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableDirectMoveCommit {
    pub work_key: TransferWorkKey,
    pub source: PathBuf,
    pub target: PathBuf,
    pub checkpoint_revision: u64,
}

#[derive(Debug, Clone)]
pub enum FileOperationEvent {
    /// 进度与逐条目占位快照同行发送;快照全量替换界面侧任务状态。
    Progressed {
        task_id: u64,
        update: FileOperationProgressUpdate,
        entries: Vec<TransferEntrySnapshot>,
    },
    /// rename 已全部完成但提交点还没落盘,让列表刷新不等 parent fsync 与日志落盘。
    MovesRenamed {
        task_id: u64,
        moves: Vec<CompletedTransfer>,
    },
    DirectMovesCommitted {
        task_id: u64,
        commits: Vec<DurableDirectMoveCommit>,
    },
    /// 包内成员提取缺密码或密码错误:执行器按失败收尾,界面询问密码后让用户重新入队。
    ArchiveMemberPasswordRequested {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        invalid_retry: bool,
    },
}
