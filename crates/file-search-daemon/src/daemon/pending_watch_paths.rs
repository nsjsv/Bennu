//! 监听到的变化按路径各自等一等再交给维护工作。
//!
//! - 最后一次变化后安静一会儿才算写完；一直在变的，最多等一阵。
//! - 删除、移出马上交：建了马上就删的临时文件交出去时已经不在，索引一行都不写；删掉的文件也
//!   马上从搜索里消失。
//! - 刚交出去的路径又变了要退避，越常变退得越久：一直在写的日志、数据库每分钟最多处理一次。
//!
//! 交出的只是路径，处理时按文件系统当时的样子核对，所以合并掉中间的变化不会丢东西。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::bounded_paths::estimated_path_bytes;

/// 最后一次变化后安静这么久才算写完；保存一次文件（写完即关）也只晚这么一点。
const WRITE_SETTLE: Duration = Duration::from_secs(2);
/// 一直在变的路径从第一次变化算起最多等这么久，再受退避约束。
const MAX_SETTLE_WAIT: Duration = Duration::from_secs(10);
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// 交出后这么久没再交，退避从头算。
const BACKOFF_MEMORY: Duration = Duration::from_secs(120);
/// 记退避的路径上限；记不下的路径照常处理，只是不退避。
const MAX_REMEMBERED_HANDOVERS: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathChange {
    /// 内容、属性或名字出现：可能还在写，等它安静。
    Written,
    /// 删除或移出：不用等。
    Removed,
}

#[derive(Debug)]
struct PendingPath {
    first_change: Instant,
    settles_at: Instant,
}

#[derive(Debug)]
struct Handover {
    at: Instant,
    backoff: Duration,
}

#[derive(Debug)]
pub(super) struct PendingWatchPaths {
    pending: HashMap<PathBuf, PendingPath>,
    pending_bytes: usize,
    max_paths: usize,
    max_bytes: usize,
    handovers: HashMap<PathBuf, Handover>,
}

impl PendingWatchPaths {
    pub(super) fn new(max_paths: usize, max_bytes: usize) -> Self {
        Self {
            pending: HashMap::new(),
            pending_bytes: 0,
            max_paths,
            max_bytes,
            handovers: HashMap::new(),
        }
    }

    /// 记下一次变化。装不下时原样退回；调用方先把等着的全部交出去（`take_all`）再记。
    pub(super) fn record(
        &mut self,
        path: PathBuf,
        change: PathChange,
        now: Instant,
    ) -> Result<(), PathBuf> {
        if let Some(pending) = self.pending.get_mut(&path) {
            pending.settles_at = settle_time(pending.first_change, change, now);
            return Ok(());
        }
        let path_bytes = estimated_path_bytes(&path);
        if self.pending.len() >= self.max_paths
            || self.pending_bytes.saturating_add(path_bytes) > self.max_bytes
        {
            return Err(path);
        }
        self.pending_bytes = self.pending_bytes.saturating_add(path_bytes);
        self.pending.insert(
            path,
            PendingPath {
                first_change: now,
                settles_at: settle_time(now, change, now),
            },
        );
        Ok(())
    }

    /// 取出到时候的路径，记下交出时刻：它们下次再变要退避。
    pub(super) fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        let due_paths = self
            .pending
            .iter()
            .filter(|(path, pending)| self.due_at(path, pending) <= now)
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        self.hand_over(due_paths, now)
    }

    /// 装满时整批提前交出：一大批改动照旧按批处理，不会因为要等而溢出成整个根目录重查。
    pub(super) fn take_all(&mut self, now: Instant) -> Vec<PathBuf> {
        let paths = self.pending.keys().cloned().collect();
        self.hand_over(paths, now)
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|(path, pending)| self.due_at(path, pending))
            .min()
    }

    /// 这些根目录要整个重查，其下等着的路径不必再单独交。
    pub(super) fn forget_under(&mut self, roots: &[PathBuf]) {
        let forgotten = self
            .pending
            .keys()
            .filter(|path| roots.iter().any(|root| path.starts_with(root)))
            .cloned()
            .collect::<Vec<_>>();
        for path in forgotten {
            self.remove_pending(&path);
        }
    }

    fn due_at(&self, path: &Path, pending: &PendingPath) -> Instant {
        match self.handovers.get(path) {
            Some(handover) => pending.settles_at.max(handover.at + handover.backoff),
            None => pending.settles_at,
        }
    }

    fn hand_over(&mut self, paths: Vec<PathBuf>, now: Instant) -> Vec<PathBuf> {
        for path in &paths {
            self.remove_pending(path);
            self.remember_handover(path, now);
        }
        paths
    }

    fn remove_pending(&mut self, path: &Path) {
        if self.pending.remove(path).is_some() {
            self.pending_bytes = self
                .pending_bytes
                .saturating_sub(estimated_path_bytes(path));
        }
    }

    fn remember_handover(&mut self, path: &Path, now: Instant) {
        let backoff = match self.handovers.get(path) {
            Some(previous) if now.duration_since(previous.at) < BACKOFF_MEMORY => {
                previous.backoff.saturating_mul(2).min(MAX_BACKOFF)
            }
            _ => MIN_BACKOFF,
        };
        if !self.handovers.contains_key(path) && self.handovers.len() >= MAX_REMEMBERED_HANDOVERS {
            self.handovers
                .retain(|_, handover| now.duration_since(handover.at) < BACKOFF_MEMORY);
            if self.handovers.len() >= MAX_REMEMBERED_HANDOVERS {
                return;
            }
        }
        self.handovers
            .insert(path.to_path_buf(), Handover { at: now, backoff });
    }
}

fn settle_time(first_change: Instant, change: PathChange, now: Instant) -> Instant {
    match change {
        PathChange::Removed => now,
        PathChange::Written => (now + WRITE_SETTLE).min(first_change + MAX_SETTLE_WAIT),
    }
}

#[cfg(test)]
#[path = "pending_watch_paths_tests.rs"]
mod tests;
