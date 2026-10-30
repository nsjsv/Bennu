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

/// inotify 用不了时轮询目录本身的间隔，取 KDE `KDirWatch` 本地目录的默认值：
/// 每次只 stat 一个目录，代价很小。
const DIRECTORY_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct DirectoryWatcher {
    source: WatchSource,
    events: mpsc::UnboundedReceiver<DirectoryEntryChanges>,
}

/// 变化从哪来：inotify，或者它建不起来时的目录轮询（同 KDirWatch 退回 stat）。
enum WatchSource {
    /// 只为持有：丢掉就停止监听。
    Notify {
        _watcher: RecommendedWatcher,
    },
    Polling(tokio::task::JoinHandle<()>),
}

impl DirectoryWatcher {
    pub async fn recv(&mut self) -> Option<DirectoryEntryChanges> {
        self.events.recv().await
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        // 轮询任务不随接收端自动结束，没人看了就停掉。
        if let WatchSource::Polling(task) = &self.source {
            task.abort();
        }
    }
}

/// 监听一个目录的条目变化。inotify 建不起来（如名额用完）时退回轮询目录本身，
/// 只能发现条目增删改名（发现不了已有文件内容变化），与 KDirWatch 的 stat 方式相同；
/// 只有目录连 stat 都做不到时才失败。
pub fn watch_directory(
    path: impl AsRef<Path>,
    debounce: Duration,
) -> Result<DirectoryWatcher, FileError> {
    watch_directory_with(path.as_ref(), debounce, start_notify_watcher)
}

fn start_notify_watcher(
    path: &Path,
    raw_tx: mpsc::UnboundedSender<Vec<RawEntryChange>>,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                let _ = raw_tx.send(raw_entry_changes(&event));
            }
            Err(_) => {
                let _ = raw_tx.send(vec![RawEntryChange::Rescan]);
            }
        })?;
    watcher.watch(path, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

fn watch_directory_with(
    path: &Path,
    debounce: Duration,
    start_notify: impl FnOnce(
        &Path,
        mpsc::UnboundedSender<Vec<RawEntryChange>>,
    ) -> notify::Result<RecommendedWatcher>,
) -> Result<DirectoryWatcher, FileError> {
    let path = path.to_path_buf();
    let (raw_tx, raw_rx) = mpsc::unbounded_channel();
    let (change_tx, change_rx) = mpsc::unbounded_channel();

    let source = match start_notify(&path, raw_tx.clone()) {
        Ok(watcher) => WatchSource::Notify { _watcher: watcher },
        Err(notify_error) => {
            let stamp = std::fs::symlink_metadata(&path)
                .map(|metadata| directory_stamp(&metadata))
                .map_err(|source| FileError::Watch {
                    path: path.clone(),
                    message: format!("{notify_error}; polling also failed: {source}"),
                })?;
            tracing::warn!(
                target: "file_core::watch",
                event = "directory_watch_polling",
                path = %path.display(),
                error = %notify_error,
                "directory cannot be watched; polling it for changes"
            );
            WatchSource::Polling(tokio::spawn(poll_directory(path.clone(), stamp, raw_tx)))
        }
    };

    tokio::spawn(debounce_events(path, raw_rx, change_tx, debounce));

    Ok(DirectoryWatcher {
        source,
        events: change_rx,
    })
}

/// 目录本身的身份与时间：条目增删改名会改 mtime/ctime，目录被换会改 dev/ino。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryStamp {
    #[cfg(unix)]
    identity: (u64, u64),
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

fn directory_stamp(metadata: &std::fs::Metadata) -> DirectoryStamp {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        DirectoryStamp {
            identity: (metadata.dev(), metadata.ino()),
            modified: metadata.modified().ok(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
    #[cfg(not(unix))]
    {
        DirectoryStamp {
            modified: metadata.modified().ok(),
        }
    }
}

/// 目录本身变了就要求整目录重读；读不到（被删、被换、没权限）也算变了，由重读给出真实结果。
async fn poll_directory(
    path: PathBuf,
    stamp: DirectoryStamp,
    raw_tx: mpsc::UnboundedSender<Vec<RawEntryChange>>,
) {
    let mut last = Some(stamp);
    loop {
        sleep(DIRECTORY_POLL_INTERVAL).await;
        let current = tokio::fs::symlink_metadata(&path)
            .await
            .ok()
            .map(|metadata| directory_stamp(&metadata));
        if current != last {
            last = current;
            if raw_tx.send(vec![RawEntryChange::Rescan]).is_err() {
                return;
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tracing_subscriber::layer::{Context, SubscriberExt};

    use super::*;

    /// 模拟 inotify 名额用完。
    fn unavailable_notify(
        _: &Path,
        _: mpsc::UnboundedSender<Vec<RawEntryChange>>,
    ) -> notify::Result<RecommendedWatcher> {
        Err(notify::Error::generic("inotify watch limit reached"))
    }

    struct PollingWarningCounter(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PollingWarningCounter {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            struct EventName(bool);
            impl tracing::field::Visit for EventName {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "event" && value == "directory_watch_polling" {
                        self.0 = true;
                    }
                }
                fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            }
            let mut name = EventName(false);
            event.record(&mut name);
            if name.0 {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[tokio::test]
    async fn a_folder_that_cannot_be_watched_is_polled_for_new_entries() {
        let directory = tempfile::tempdir().unwrap();
        let warnings = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(PollingWarningCounter(Arc::clone(&warnings)));
        let mut watcher = tracing::subscriber::with_default(subscriber, || {
            watch_directory_with(
                directory.path(),
                Duration::from_millis(20),
                unavailable_notify,
            )
        })
        .unwrap();
        assert_eq!(warnings.load(Ordering::SeqCst), 1);

        std::fs::write(directory.path().join("new.txt"), b"new").unwrap();

        let changes = tokio::time::timeout(Duration::from_secs(5), watcher.recv())
            .await
            .expect("polling should notice the new entry")
            .unwrap();
        assert_eq!(changes.directory, directory.path());
        assert!(matches!(
            changes.changes.as_slice(),
            [ResolvedEntryChange::RescanRequired]
        ));
    }

    #[tokio::test]
    async fn dropping_a_polling_watcher_stops_polling() {
        let directory = tempfile::tempdir().unwrap();
        let watcher = watch_directory_with(
            directory.path(),
            Duration::from_millis(20),
            unavailable_notify,
        )
        .unwrap();
        let WatchSource::Polling(task) = &watcher.source else {
            panic!("the watcher should have fallen back to polling");
        };
        let poll = task.abort_handle();

        drop(watcher);
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(poll.is_finished());
    }

    #[test]
    fn a_folder_that_cannot_even_be_read_fails_to_watch() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");

        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                watch_directory_with(&missing, Duration::from_millis(20), unavailable_notify)
                    .map(|_| ())
            });

        assert!(matches!(outcome, Err(FileError::Watch { path, .. }) if path == missing));
    }
}
