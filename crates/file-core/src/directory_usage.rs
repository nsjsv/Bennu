//! 文件夹用量（里面有多少文件、文件夹，共多大）：全进程一组工作线程并行走目录树。
//! 单线程逐个 stat 在百万文件的目录上要好几秒，线程数取核数（实测 134 万文件
//! 8 线程 0.92 s、16 线程 0.64 s、32 线程不再变快）。属性窗口、列表视图的总大小、
//! 快传、文件夹预览都用这一份，同时测很多个文件夹也只占这一组线程。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use crate::is_hidden_name;

const WORKER_STACK_BYTES: usize = 256 * 1024;

/// 隐藏条目（名字以点开头）算不算：属性窗口这类要实际占用，全算；文件夹预览没开
/// 「显示隐藏文件」时要和看得见的对上，隐藏的不算、隐藏文件夹也不进去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenEntries {
    Counted,
    Skipped,
}

/// 文件夹里面（不含它自己）的条目：大小是各条目 `len` 之和，含文件夹、符号链接自身
/// 的大小；不跟符号链接，跨挂载点照走。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryUsage {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub disk_bytes: u64,
    /// 读不了、因而没算进来的子目录或条目数；非零时上面的数字偏小。
    pub unreadable: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageProgress {
    Running,
    Finished,
    /// 文件夹本身读不了。
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub usage: DirectoryUsage,
    pub progress: UsageProgress,
}

/// 一次测量。丢掉就停：工作线程看到取消就扔掉它还没走的目录。
#[derive(Debug)]
pub struct UsageMeasurement {
    job: Arc<Job>,
}

impl UsageMeasurement {
    pub fn start(root: PathBuf, hidden: HiddenEntries) -> Self {
        let job = Arc::new(Job::new(root.clone(), hidden));
        pool().enqueue(&job, vec![root]);
        Self { job }
    }

    /// 读到现在为止的数；随时可读，不等。
    pub fn snapshot(&self) -> UsageSnapshot {
        self.job.snapshot()
    }

    /// 等到算完或失败。
    pub async fn finished(&self) -> UsageSnapshot {
        loop {
            // 先登记再看状态：两步之间算完发出的通知也收得到。
            let notified = self.job.settled.notified();
            let snapshot = self.job.snapshot();
            if snapshot.progress != UsageProgress::Running {
                return snapshot;
            }
            notified.await;
        }
    }
}

impl Drop for UsageMeasurement {
    fn drop(&mut self) {
        self.job.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct Job {
    /// 在队列里认它用。
    id: u64,
    root: PathBuf,
    hidden: HiddenEntries,
    cancelled: AtomicBool,
    /// 排着的加正在走的目录数；减到 0 就算完。
    outstanding: AtomicUsize,
    files: AtomicU64,
    folders: AtomicU64,
    bytes: AtomicU64,
    disk_bytes: AtomicU64,
    unreadable: AtomicU64,
    progress: Mutex<UsageProgress>,
    settled: tokio::sync::Notify,
}

impl Job {
    fn new(root: PathBuf, hidden: HiddenEntries) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            root,
            hidden,
            cancelled: AtomicBool::new(false),
            outstanding: AtomicUsize::new(1),
            files: AtomicU64::new(0),
            folders: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            disk_bytes: AtomicU64::new(0),
            unreadable: AtomicU64::new(0),
            progress: Mutex::new(UsageProgress::Running),
            settled: tokio::sync::Notify::new(),
        }
    }

    fn snapshot(&self) -> UsageSnapshot {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        UsageSnapshot {
            usage: DirectoryUsage {
                files: self.files.load(Ordering::Relaxed),
                folders: self.folders.load(Ordering::Relaxed),
                bytes: self.bytes.load(Ordering::Relaxed),
                disk_bytes: self.disk_bytes.load(Ordering::Relaxed),
                unreadable: self.unreadable.load(Ordering::Relaxed),
            },
            progress,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// 走一层：把这一层的数加上，返回要接着走的子目录。
    fn scan(&self, directory: &Path) -> Vec<PathBuf> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if directory == self.root => {
                self.settle(UsageProgress::Failed(error.to_string()));
                return Vec::new();
            }
            Err(_) => {
                self.unreadable.fetch_add(1, Ordering::Relaxed);
                return Vec::new();
            }
        };
        let mut layer = DirectoryUsage::default();
        let mut subdirectories = Vec::new();
        for entry in entries {
            if self.is_cancelled() {
                return Vec::new();
            }
            let Ok((entry, file_type, metadata)) = entry.and_then(|entry| {
                let file_type = entry.file_type()?;
                let metadata = entry.metadata()?;
                Ok((entry, file_type, metadata))
            }) else {
                layer.unreadable += 1;
                continue;
            };
            if self.hidden == HiddenEntries::Skipped && is_hidden_name(&entry.file_name()) {
                continue;
            }
            if file_type.is_dir() {
                layer.folders += 1;
                subdirectories.push(entry.path());
            } else {
                layer.files += 1;
            }
            layer.bytes = layer.bytes.saturating_add(metadata.len());
            layer.disk_bytes = layer.disk_bytes.saturating_add(disk_size(&metadata));
        }
        self.files.fetch_add(layer.files, Ordering::Relaxed);
        self.folders.fetch_add(layer.folders, Ordering::Relaxed);
        self.bytes.fetch_add(layer.bytes, Ordering::Relaxed);
        self.disk_bytes
            .fetch_add(layer.disk_bytes, Ordering::Relaxed);
        self.unreadable
            .fetch_add(layer.unreadable, Ordering::Relaxed);
        subdirectories
    }

    /// 走完一个目录（它的子目录已经记进 `outstanding`）。
    fn finish_directory(&self) {
        if self.outstanding.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.settle(UsageProgress::Finished);
        }
    }

    /// 只认第一次：根读不了已经记成失败，之后走完不能盖成「算完」。
    fn settle(&self, outcome: UsageProgress) {
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *progress == UsageProgress::Running {
            *progress = outcome;
            drop(progress);
            self.settled.notify_waiters();
        }
    }
}

#[cfg(unix)]
fn disk_size(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn disk_size(metadata: &std::fs::Metadata) -> u64 {
    metadata.len()
}

/// 每个测量一个目录栈（后进先出：栈短，刚读过的目录项还在缓存里）；工作线程在
/// 所有测量之间轮流各取一个目录，大文件夹不会把同时在测的小文件夹饿住。
struct Pool {
    queues: Mutex<Queues>,
    work_ready: Condvar,
}

#[derive(Default)]
struct Queues {
    /// 还有目录没走的测量，按轮到的先后。
    turns: VecDeque<u64>,
    pending: HashMap<u64, (Arc<Job>, Vec<PathBuf>)>,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .max(2);
        for index in 0..workers {
            std::thread::Builder::new()
                .name(format!("bennu-usage-{index}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn(|| pool().work())
                .expect("start directory usage worker");
        }
        Pool {
            queues: Mutex::new(Queues::default()),
            work_ready: Condvar::new(),
        }
    })
}

impl Pool {
    fn lock(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn enqueue(&self, job: &Arc<Job>, directories: Vec<PathBuf>) {
        if directories.is_empty() {
            return;
        }
        let count = directories.len();
        let mut queues = self.lock();
        let (_, stack) = queues
            .pending
            .entry(job.id)
            .or_insert_with(|| (job.clone(), Vec::new()));
        let was_idle = stack.is_empty();
        stack.extend(directories);
        if was_idle {
            queues.turns.push_back(job.id);
        }
        drop(queues);
        if count == 1 {
            self.work_ready.notify_one();
        } else {
            self.work_ready.notify_all();
        }
    }

    fn take(&self) -> (Arc<Job>, PathBuf) {
        let mut queues = self.lock();
        loop {
            while let Some(id) = queues.turns.pop_front() {
                let Some((job, mut stack)) = queues.pending.remove(&id) else {
                    continue;
                };
                // 取消了的整个扔掉；别的线程手里还有它的目录时，那些走完不再推子目录。
                if job.is_cancelled() {
                    continue;
                }
                let directory = stack.pop().expect("a queued job has a directory");
                if !stack.is_empty() {
                    queues.pending.insert(id, (job.clone(), stack));
                    queues.turns.push_back(id);
                }
                return (job, directory);
            }
            queues = self
                .work_ready
                .wait(queues)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn work(&self) {
        loop {
            let (job, directory) = self.take();
            if !job.is_cancelled() {
                let subdirectories = job.scan(&directory);
                if !job.is_cancelled() {
                    job.outstanding
                        .fetch_add(subdirectories.len(), Ordering::AcqRel);
                    self.enqueue(&job, subdirectories);
                }
            }
            job.finish_directory();
        }
    }
}

#[cfg(test)]
#[path = "directory_usage_tests.rs"]
mod tests;
