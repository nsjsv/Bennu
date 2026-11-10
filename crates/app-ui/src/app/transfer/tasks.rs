//! LocalSend 收发任务：任务面板里与文件操作混排的那些行；设备行的状态也从这里读。
//!
//! 文件操作队列（`file-operations`）是先进先出、存盘、能续跑的；LocalSend 收发同时
//! 进行、断了不能续，所以单独记在这里，只在面板上和文件任务按时间混排、共用一个
//! 图标。重启不留（以后看传输记录）。
//!
//! 每次发送（发给一台）、每次接收（接受了的一次会话）各是一个任务。设备行显示这台
//! 设备最新的那次发送；新的一次发送不删旧任务，只把被取代的文字发送（不进面板）去掉。

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use local_send::{CancellationToken, FileMetadata, LinkControl, PrepareProgress};

use super::direct_send::{DirectSendPhase, DirectSendState};

/// 速度至少隔这么久才重算：进度事件很密，相邻两次的差太抖。
const SPEED_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(crate) struct TransferTasks {
    /// 按开始的先后。
    tasks: Vec<TransferTask>,
    /// 还在准备（展开文件夹、算校验）的批次：几台设备共用一次准备。
    pub(super) batches: HashMap<u64, Batch>,
    next_id: u64,
}

#[derive(Debug)]
pub(super) struct Batch {
    pub(super) progress: PrepareProgress,
    /// 等这批的设备都取消了才停下准备。
    pub(super) cancel: CancellationToken,
}

#[derive(Debug)]
pub(crate) struct TransferTask {
    pub(crate) id: u64,
    pub(crate) started_at: SystemTime,
    /// 同文件任务：状态变了且面板没开着就算没看过（角标）。
    read: bool,
    pub(crate) kind: TransferKind,
}

#[derive(Debug)]
pub(crate) enum TransferKind {
    Send(DirectSendState),
    Receive(ReceiveTask),
}

/// 一次接收：谁发来的、哪些文件、每个文件收了多少。
#[derive(Debug)]
pub(crate) struct ReceiveTask {
    pub(crate) session_id: String,
    /// 发送方的名字：收藏里起过名的用收藏名；手机网页是"网页 IP"。
    pub(crate) sender: String,
    pub(crate) source: ReceiveSource,
    pub(crate) files: Vec<FileMetadata>,
    /// fileId → 已收字节（重传时从头算）。
    received: HashMap<String, u64>,
    pub(crate) speed: SpeedMeter,
    pub(crate) phase: ReceivePhase,
}

/// 这次接收从哪来：取消要发回给它。
#[derive(Debug)]
pub(crate) enum ReceiveSource {
    /// 常驻 LocalSend 服务（装了 LocalSend 的设备发来）。
    LocalSend,
    /// 手机网页，经那次链接的临时服务。
    Link(LinkControl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReceivePhase {
    Receiving,
    Canceling,
    /// `failed`：(文件名, 原因)。
    Finished {
        saved: usize,
        failed: Vec<(String, String)>,
    },
    Canceled {
        saved: usize,
    },
}

/// 按进度样本差分算速度。
#[derive(Debug, Clone, Default)]
pub(crate) struct SpeedMeter {
    last: Option<(u64, Instant)>,
    pub(crate) bytes_per_second: u64,
}

impl SpeedMeter {
    pub(super) fn sample(&mut self, bytes: u64, now: Instant) {
        match self.last {
            // 重传让已传字节倒退：从这里重新开始量。
            Some((earlier, _)) if bytes < earlier => self.last = Some((bytes, now)),
            Some((earlier, at)) => {
                let elapsed = now.duration_since(at);
                if elapsed >= SPEED_SAMPLE_INTERVAL {
                    self.bytes_per_second =
                        ((bytes - earlier) as f64 / elapsed.as_secs_f64()) as u64;
                    self.last = Some((bytes, now));
                }
            }
            None => self.last = Some((bytes, now)),
        }
    }
}

impl ReceiveTask {
    pub(super) fn new(
        session_id: String,
        sender: String,
        source: ReceiveSource,
        files: Vec<FileMetadata>,
    ) -> Self {
        Self {
            session_id,
            sender,
            source,
            files,
            received: HashMap::new(),
            speed: SpeedMeter::default(),
            phase: ReceivePhase::Receiving,
        }
    }

    pub(super) fn record_progress(&mut self, file_id: String, received: u64) {
        self.received.insert(file_id, received);
        self.speed.sample(self.bytes_done(), Instant::now());
    }

    pub(crate) fn bytes_done(&self) -> u64 {
        self.received.values().sum()
    }

    pub(crate) fn bytes_total(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// 收完了几个文件。
    pub(crate) fn files_done(&self) -> usize {
        self.files
            .iter()
            .filter(|file| self.received.get(&file.id) == Some(&file.size))
            .count()
    }
}

impl TransferTask {
    pub(crate) fn is_terminal(&self) -> bool {
        match &self.kind {
            TransferKind::Send(send) => matches!(
                send.phase,
                DirectSendPhase::Finished { .. }
                    | DirectSendPhase::Canceled
                    | DirectSendPhase::Failed(_)
            ),
            TransferKind::Receive(receive) => matches!(
                receive.phase,
                ReceivePhase::Finished { .. } | ReceivePhase::Canceled { .. }
            ),
        }
    }

    /// 标红：整次失败，或有文件没收发成。
    pub(crate) fn is_failed(&self) -> bool {
        match &self.kind {
            TransferKind::Send(send) => match &send.phase {
                DirectSendPhase::Failed(_) => true,
                DirectSendPhase::Finished { failed, .. } => !failed.is_empty(),
                _ => false,
            },
            TransferKind::Receive(receive) => matches!(
                &receive.phase,
                ReceivePhase::Finished { failed, .. } if !failed.is_empty()
            ),
        }
    }

    /// 进行中的确定进度（0–1）；校验中算校验的进度。等对方接受、没开始传时为 None。
    fn fraction(&self, batches: &HashMap<u64, Batch>) -> Option<f32> {
        let (done, total) = match &self.kind {
            TransferKind::Send(send) => match send.phase {
                DirectSendPhase::Checking => {
                    let progress = send.batch().and_then(|batch| batches.get(&batch))?;
                    (progress.progress.bytes_done, progress.progress.bytes_total)
                }
                DirectSendPhase::Sending | DirectSendPhase::Canceling => {
                    (send.bytes_done, send.bytes_total)
                }
                _ => return None,
            },
            TransferKind::Receive(receive) => (receive.bytes_done(), receive.bytes_total()),
        };
        // 有总量才算确定：发送端第一次报进度前还在等对方接受。
        (total > 0).then(|| (done.min(total) as f32) / (total as f32))
    }

    fn is_moving(&self) -> bool {
        match &self.kind {
            TransferKind::Send(send) => matches!(
                send.phase,
                DirectSendPhase::Checking | DirectSendPhase::Sending | DirectSendPhase::Canceling
            ),
            TransferKind::Receive(receive) => matches!(
                receive.phase,
                ReceivePhase::Receiving | ReceivePhase::Canceling
            ),
        }
    }

    /// 文字发送一下就完，不进面板。
    pub(crate) fn shows_in_panel(&self) -> bool {
        match &self.kind {
            TransferKind::Send(send) => !send.is_text(),
            TransferKind::Receive(_) => true,
        }
    }

    pub(super) fn send_mut(&mut self) -> Option<&mut DirectSendState> {
        match &mut self.kind {
            TransferKind::Send(send) => Some(send),
            TransferKind::Receive(_) => None,
        }
    }
}

impl TransferTasks {
    /// 新任务；`panel_open` 时直接算看过。发给同一台设备的旧文字发送被它取代。
    pub(super) fn push(&mut self, kind: TransferKind, panel_open: bool) -> u64 {
        if let TransferKind::Send(send) = &kind {
            let fingerprint = send.target.fingerprint.clone();
            self.tasks.retain(|task| {
                !matches!(&task.kind, TransferKind::Send(old)
                    if old.is_text() && old.target.fingerprint == fingerprint && !old.is_busy())
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(TransferTask {
            id,
            started_at: SystemTime::now(),
            read: panel_open,
            kind,
        });
        id
    }

    pub(super) fn next_batch(&mut self) -> u64 {
        let batch = self.next_id;
        self.next_id += 1;
        batch
    }

    pub(super) fn get_mut(&mut self, id: u64) -> Option<&mut TransferTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    pub(super) fn send(&self, id: u64) -> Option<&DirectSendState> {
        self.tasks.iter().find_map(|task| match &task.kind {
            TransferKind::Send(send) if task.id == id => Some(send),
            _ => None,
        })
    }

    pub(super) fn send_mut(&mut self, id: u64) -> Option<&mut DirectSendState> {
        self.get_mut(id).and_then(TransferTask::send_mut)
    }

    pub(super) fn sends_mut(&mut self) -> impl Iterator<Item = (u64, &mut DirectSendState)> {
        self.tasks
            .iter_mut()
            .filter_map(|task| match &mut task.kind {
                TransferKind::Send(send) => Some((task.id, send)),
                TransferKind::Receive(_) => None,
            })
    }

    /// 这台设备最新的那次发送（设备行显示它、按它判断能不能再发）。
    pub(crate) fn latest_send_to(&self, fingerprint: &str) -> Option<(u64, &DirectSendState)> {
        self.tasks.iter().rev().find_map(|task| match &task.kind {
            TransferKind::Send(send) if send.target.fingerprint == fingerprint => {
                Some((task.id, send))
            }
            _ => None,
        })
    }

    pub(super) fn receive_mut(&mut self, session_id: &str) -> Option<&mut TransferTask> {
        self.tasks.iter_mut().find(|task| {
            matches!(&task.kind, TransferKind::Receive(receive) if receive.session_id == session_id)
        })
    }

    /// 状态变到终态：面板没开着就算没看过（同文件任务）。
    pub(super) fn mark_changed(&mut self, id: u64, panel_open: bool) {
        if let Some(task) = self.get_mut(id) {
            task.read = panel_open;
        }
    }

    /// 进行中的确定进度（0–1），见 `TransferTask::fraction`。
    pub(crate) fn fraction(&self, task: &TransferTask) -> Option<f32> {
        task.fraction(&self.batches)
    }

    pub(crate) fn panel_tasks(&self) -> impl DoubleEndedIterator<Item = &TransferTask> {
        self.tasks.iter().filter(|task| task.shows_in_panel())
    }

    pub(crate) fn unread_count(&self) -> usize {
        self.panel_tasks().filter(|task| !task.read).count()
    }

    pub(crate) fn has_unread_failed(&self) -> bool {
        self.panel_tasks()
            .any(|task| !task.read && task.is_failed())
    }

    pub(crate) fn mark_all_read(&mut self) {
        for task in &mut self.tasks {
            task.read = true;
        }
    }

    pub(crate) fn has_terminal(&self) -> bool {
        self.panel_tasks().any(TransferTask::is_terminal)
    }

    /// 面板"清除"：只清已结束的。
    pub(crate) fn clear(&mut self, id: u64) {
        self.tasks
            .retain(|task| task.id != id || !task.is_terminal() || !task.shows_in_panel());
    }

    pub(crate) fn clear_finished(&mut self) {
        self.tasks
            .retain(|task| !task.is_terminal() || !task.shows_in_panel());
    }

    pub(crate) fn has_active(&self) -> bool {
        self.panel_tasks().any(|task| !task.is_terminal())
    }

    /// 图标的进度环：第一个有确定进度的进行中任务。
    pub(crate) fn active_fraction(&self) -> Option<f32> {
        self.panel_tasks()
            .filter(|task| !task.is_terminal())
            .find_map(|task| task.fraction(&self.batches))
    }

    /// 正在动但没有确定进度的任务（等对方接受）：要转动画。等 PIN 不算，那是在等人。
    pub(crate) fn has_indeterminate(&self) -> bool {
        self.panel_tasks()
            .any(|task| task.is_moving() && task.fraction(&self.batches).is_none())
    }
}

#[cfg(test)]
mod tests;
