use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::directory_metadata::DiscoveredDirectoryEntry;
use crate::FileError;

/// 单窗口聚合后允许的最大条目变更数：超过即视为批量写入（解压、批量生成），
/// 逐条应用反而慢于一次全量重扫，整体退化为 RescanRequired。
const ENTRY_CHANGE_RESCAN_THRESHOLD: usize = 64;

/// watcher 产出的一条条目级变更。新增/改名目标/内容变化的条目都在
/// resolve 阶段完成 stat（携带完整显示元数据），应用方无需再做 IO。
#[derive(Debug, Clone)]
pub enum ResolvedEntryChange {
    Added(DiscoveredDirectoryEntry),
    Removed {
        path: PathBuf,
    },
    Renamed {
        from: PathBuf,
        to: DiscoveredDirectoryEntry,
    },
    ContentChanged(DiscoveredDirectoryEntry),
    RescanRequired,
}

/// 一个受监视目录在一个防抖窗口内的聚合变更。
#[derive(Debug, Clone)]
pub struct DirectoryEntryChanges {
    pub directory: PathBuf,
    pub changes: Vec<ResolvedEntryChange>,
}

/// notify 原始事件到条目级变更的中间形态，仅存在于聚合窗口内部。
pub struct DirectoryWatcher {
    _watcher: RecommendedWatcher,
    events: mpsc::UnboundedReceiver<DirectoryEntryChanges>,
}

impl DirectoryWatcher {
    pub async fn recv(&mut self) -> Option<DirectoryEntryChanges> {
        self.events.recv().await
    }
}

pub fn watch_directory(
    path: impl AsRef<Path>,
    debounce: Duration,
) -> Result<DirectoryWatcher, FileError> {
    let path = path.as_ref().to_path_buf();
    let (raw_tx, raw_rx) = mpsc::unbounded_channel();
    let (change_tx, change_rx) = mpsc::unbounded_channel();

    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                let _ = raw_tx.send(raw_entry_changes(&event));
            }
            Err(_) => {
                let _ = raw_tx.send(vec![RawEntryChange::Rescan]);
            }
        })
        .map_err(|source| FileError::Watch {
            path: path.clone(),
            message: source.to_string(),
        })?;

    watcher
        .watch(&path, RecursiveMode::NonRecursive)
        .map_err(|source| FileError::Watch {
            path: path.clone(),
            message: source.to_string(),
        })?;

    tokio::spawn(debounce_events(path, raw_rx, change_tx, debounce));

    Ok(DirectoryWatcher {
        _watcher: watcher,
        events: change_rx,
    })
}

#[derive(Debug, PartialEq)]
enum RawEntryChange {
    Added(PathBuf),
    Removed(PathBuf),
    Renamed(PathBuf, PathBuf),
    ContentChanged(PathBuf),
    Rescan,
}

fn raw_entry_changes(event: &notify::Event) -> Vec<RawEntryChange> {
    if event.need_rescan() {
        return vec![RawEntryChange::Rescan];
    }
    let paths = &event.paths;
    match &event.kind {
        EventKind::Create(_) => paths
            .first()
            .map(|path| vec![RawEntryChange::Added(path.clone())])
            .unwrap_or_default(),
        EventKind::Remove(_) => paths
            .first()
            .map(|path| vec![RawEntryChange::Removed(path.clone())])
            .unwrap_or_default(),
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::From)) => {
            paths
                .first()
                .map(|path| vec![RawEntryChange::Removed(path.clone())])
                .unwrap_or_default()
        }
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::To)) => paths
            .first()
            .map(|path| vec![RawEntryChange::Added(path.clone())])
            .unwrap_or_default(),
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::Both)) => {
            match (paths.first(), paths.get(1)) {
                (Some(from), Some(to)) => {
                    vec![RawEntryChange::Renamed(from.clone(), to.clone())]
                }
                _ => Vec::new(),
            }
        }
        EventKind::Modify(_) => paths
            .first()
            .map(|path| vec![RawEntryChange::ContentChanged(path.clone())])
            .unwrap_or_default(),
        // inotify 溢出等不可解析信号：保守要求全量对账。
        EventKind::Other | EventKind::Access(_) | EventKind::Any => Vec::new(),
    }
}

async fn debounce_events(
    path: PathBuf,
    mut raw_rx: mpsc::UnboundedReceiver<Vec<RawEntryChange>>,
    change_tx: mpsc::UnboundedSender<DirectoryEntryChanges>,
    debounce: Duration,
) {
    while let Some(first_batch) = raw_rx.recv().await {
        let mut raw = first_batch;
        sleep(debounce).await;
        while let Ok(next) = raw_rx.try_recv() {
            raw.extend(next);
        }

        let folded = fold_raw_changes(raw);
        let changes = if folded.len() > ENTRY_CHANGE_RESCAN_THRESHOLD
            || folded.contains(&RawEntryChange::Rescan)
        {
            vec![ResolvedEntryChange::RescanRequired]
        } else {
            resolve_raw_changes(folded).await
        };

        let _ = change_tx.send(DirectoryEntryChanges {
            directory: path.clone(),
            changes,
        });
    }
}

// 仅折叠确定性的模式（创建后立刻删除）；其余依赖应用方的幂等
// （重复删除/内容更新找不到条目时为 no-op），避免有状态折叠引入顺序 bug。
fn fold_raw_changes(raw: Vec<RawEntryChange>) -> Vec<RawEntryChange> {
    let mut folded: Vec<RawEntryChange> = Vec::with_capacity(raw.len());
    for change in raw {
        let cancels_pending_add = matches!(&change, RawEntryChange::Removed(path)
            if matches!(folded.last(), Some(RawEntryChange::Added(added)) if added == path));
        if cancels_pending_add {
            folded.pop();
            continue;
        }
        folded.push(change);
    }
    folded
}

async fn resolve_raw_changes(folded: Vec<RawEntryChange>) -> Vec<ResolvedEntryChange> {
    let mut resolved = Vec::with_capacity(folded.len());
    for change in folded {
        match change {
            RawEntryChange::Rescan => resolved.push(ResolvedEntryChange::RescanRequired),
            RawEntryChange::Added(path) => {
                if let Some(entry) = discovered_entry_at(&path).await {
                    resolved.push(ResolvedEntryChange::Added(entry));
                }
                // stat 失败说明条目已消失；后续 Removed/RescanRequired 会纠正。
            }
            RawEntryChange::Removed(path) => resolved.push(ResolvedEntryChange::Removed { path }),
            RawEntryChange::Renamed(from, to) => match discovered_entry_at(&to).await {
                Some(entry) => resolved.push(ResolvedEntryChange::Renamed { from, to: entry }),
                // 目标已不可见（改名后又被删）：按源删除处理，语义等价。
                None => resolved.push(ResolvedEntryChange::Removed { path: from }),
            },
            RawEntryChange::ContentChanged(path) => {
                if let Some(entry) = discovered_entry_at(&path).await {
                    resolved.push(ResolvedEntryChange::ContentChanged(entry));
                }
            }
        }
    }
    resolved
}

async fn discovered_entry_at(path: &Path) -> Option<DiscoveredDirectoryEntry> {
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    let name = path.file_name()?.to_os_string();
    let is_hidden = crate::scan::is_hidden_name(&name);
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        crate::entry::FileKind::Directory
    } else if file_type.is_file() {
        crate::entry::FileKind::File
    } else if file_type.is_symlink() {
        crate::entry::FileKind::Symlink
    } else {
        crate::entry::FileKind::Other
    };
    Some(DiscoveredDirectoryEntry::with_complete_filesystem_metadata(
        path.to_path_buf(),
        name,
        kind,
        is_hidden,
        file_type.is_symlink(),
        &metadata,
    ))
}
