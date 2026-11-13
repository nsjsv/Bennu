use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use file_core::mount_table::PROC_SELF_MOUNTINFO;
use rustix::fs::inotify::{self, CreateFlags, ReadFlags};
use rustix::io::Errno;

use crate::error::{SearchError, SearchResult};
use crate::logging::bounded_search_log_detail;

use super::inotify_watch::directory_watch_flags;
use super::DaemonState;

/// 监视线程被唤醒的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RootChange {
    /// 挂载表变了：顺带让失败待重查的位置立刻重查。
    MountTable,
    /// 某个位置或它的上级目录被建、删、换，或者搜索位置配置变了。
    RootEntries,
}

/// 盯住挂载表和各搜索位置的上级目录，有变化就让所有者重核。查询和查状态因此不用自己去看。
pub(super) struct RootAvailabilityMonitor {
    wake: Arc<MonitorWake>,
    thread: thread::JoinHandle<()>,
}

impl RootAvailabilityMonitor {
    pub(super) fn spawn(state: Arc<DaemonState>) -> SearchResult<Self> {
        let mountinfo = File::open(PROC_SELF_MOUNTINFO).map_err(|source| SearchError::Io {
            path: PathBuf::from(PROC_SELF_MOUNTINFO),
            source,
        })?;
        let wake = Arc::new(MonitorWake::new()?);
        let ancestors = AncestorWatch::new()?;
        let thread_wake = Arc::clone(&wake);
        let thread = thread::Builder::new()
            .name("file-search-root-monitor".to_owned())
            .spawn(move || follow_root_changes(&state, &mountinfo, &thread_wake, ancestors))
            .map_err(|error| {
                SearchError::WorkerFailed(format!("could not spawn search root monitor: {error}"))
            })?;
        Ok(Self { wake, thread })
    }

    /// 搜索位置配置变了，按新位置重新盯。
    pub(super) fn follow_new_roots(&self) {
        self.wake.wake();
    }

    pub(super) fn stop(self) {
        self.wake.stop.store(true, Ordering::Release);
        self.wake.wake();
        let _ = self.thread.join();
    }
}

fn follow_root_changes(
    state: &DaemonState,
    mountinfo: &File,
    wake: &MonitorWake,
    mut ancestors: AncestorWatch,
) {
    let mut change = RootChange::RootEntries;
    loop {
        // 先盯上再核对：核对之后才出现的变化一定会再唤醒一次。
        ancestors.follow(&state.configured_roots());
        if let Err(error) = state.reconcile_root_availability(change) {
            let log_error = bounded_search_log_detail(&error.to_string());
            tracing::warn!(
                target: "file_search::daemon",
                event = "root_reconcile_failed",
                error = %log_error,
                "search root availability could not be reconciled"
            );
        }
        change = match wait_for_change(mountinfo, wake, &mut ancestors) {
            Ok(Some(change)) => change,
            Ok(None) => return,
            Err(error) => {
                state.record_root_monitor_failure(format!(
                    "search roots are no longer monitored: {error}"
                ));
                return;
            }
        };
    }
}

fn wait_for_change(
    mountinfo: &File,
    wake: &MonitorWake,
    ancestors: &mut AncestorWatch,
) -> io::Result<Option<RootChange>> {
    loop {
        let mut descriptors = [
            libc::pollfd {
                fd: mountinfo.as_raw_fd(),
                events: libc::POLLPRI,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.event.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: ancestors.inotify.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if let Some(change) = wait_once(&mut descriptors, wake, ancestors)? {
            return Ok(change);
        }
    }
}

/// 等一次 poll。只有上级目录里与搜索位置无关的变化时返回 None，继续等。
fn wait_once(
    descriptors: &mut [libc::pollfd; 3],
    wake: &MonitorWake,
    ancestors: &mut AncestorWatch,
) -> io::Result<Option<Option<RootChange>>> {
    loop {
        let ready = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                -1,
            )
        };
        if ready >= 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    let woken = descriptors[1].revents & libc::POLLIN != 0;
    if woken {
        wake.drain();
    }
    if wake.stop.load(Ordering::Acquire) {
        return Ok(Some(None));
    }
    // 挂载表变化后内核对 mountinfo 报 POLLPRI|POLLERR，poll 本身会记下已看过这次变化（同 GLib GUnixMountMonitor）。
    if descriptors[0].revents & (libc::POLLPRI | libc::POLLERR) != 0 {
        return Ok(Some(Some(RootChange::MountTable)));
    }
    let ancestors_changed =
        descriptors[2].revents & libc::POLLIN != 0 && ancestors.drain_relevant_events()?;
    Ok((woken || ancestors_changed).then_some(Some(RootChange::RootEntries)))
}

/// 用 eventfd 而不是管道：唤醒只是累加计数，再多次唤醒也不会把写端塞满。
struct MonitorWake {
    event: OwnedFd,
    stop: AtomicBool,
}

impl MonitorWake {
    fn new() -> SearchResult<Self> {
        let descriptor = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if descriptor < 0 {
            return Err(SearchError::WorkerFailed(format!(
                "could not create search root monitor wakeup: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(Self {
            event: unsafe { OwnedFd::from_raw_fd(descriptor) },
            stop: AtomicBool::new(false),
        })
    }

    fn wake(&self) {
        let increment = 1_u64;
        let _ = unsafe {
            libc::write(
                self.event.as_raw_fd(),
                (&increment as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
    }

    fn drain(&self) {
        let mut count = 0_u64;
        let _ = unsafe {
            libc::read(
                self.event.as_raw_fd(),
                (&mut count as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
    }
}

/// 每个位置盯它最近一层存在的上级目录（同 KDE `KDirWatch` 对不存在路径的做法）：
/// 位置存在时就是父目录，能看到它被建、被删、被换成链接。只有几项，用自己的小 inotify，
/// 和挂载表一起在监视线程的 poll 里等。
struct AncestorWatch {
    inotify: OwnedFd,
    followed_roots: Vec<PathBuf>,
    watched: BTreeMap<PathBuf, i32>,
}

impl AncestorWatch {
    fn new() -> SearchResult<Self> {
        let inotify =
            inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).map_err(|error| {
                SearchError::WorkerFailed(format!("could not watch search root parents: {error}"))
            })?;
        Ok(Self {
            inotify,
            followed_roots: Vec::new(),
            watched: BTreeMap::new(),
        })
    }

    /// 反复计算直到监听集合不再变：刚算完上级、还没盯上时被建出来的子目录，下一轮就会算进去。
    fn follow(&mut self, roots: &[PathBuf]) {
        self.followed_roots = roots.to_vec();
        let mut applied = None;
        loop {
            let wanted = roots
                .iter()
                .filter_map(|root| nearest_existing_ancestor(root))
                .collect::<BTreeSet<_>>();
            if applied.as_ref() == Some(&wanted) {
                return;
            }
            let stale: Vec<PathBuf> = self
                .watched
                .keys()
                .filter(|directory| !wanted.contains(*directory))
                .cloned()
                .collect();
            for directory in stale {
                if let Some(descriptor) = self.watched.remove(&directory) {
                    // 目录已被删时内核早已撤掉监听，这里报错也无妨。
                    let _ = inotify::remove_watch(&self.inotify, descriptor);
                }
            }
            let added: Vec<PathBuf> = wanted
                .iter()
                .filter(|directory| !self.watched.contains_key(*directory))
                .cloned()
                .collect();
            for directory in added {
                match inotify::add_watch(&self.inotify, &directory, directory_watch_flags()) {
                    Ok(descriptor) => {
                        self.watched.insert(directory, descriptor);
                    }
                    Err(error) => {
                        let log_error = bounded_search_log_detail(&error.to_string());
                        tracing::warn!(
                            target: "file_search::daemon",
                            event = "root_parent_watch_failed",
                            error = %log_error,
                            "search root parent could not be watched; retrying on the next change"
                        );
                    }
                }
            }
            applied = Some(wanted);
        }
    }

    /// 读完已到的事件。队列溢出、监听被内核撤掉，或事件落在某个位置本身或它的上级上，
    /// 都算与搜索位置有关。
    fn drain_relevant_events(&mut self) -> io::Result<bool> {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(&self.inotify, &mut buffer);
        let mut relevant = false;
        loop {
            let event = match reader.next() {
                Ok(event) => event,
                Err(Errno::AGAIN) => return Ok(relevant),
                Err(Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            };
            if event
                .events()
                .intersects(ReadFlags::QUEUE_OVERFLOW | ReadFlags::IGNORED)
            {
                if event.events().contains(ReadFlags::IGNORED) {
                    self.watched
                        .retain(|_, descriptor| *descriptor != event.wd());
                }
                relevant = true;
                continue;
            }
            let Some(directory) = self
                .watched
                .iter()
                .find(|(_, descriptor)| **descriptor == event.wd())
                .map(|(directory, _)| directory.clone())
            else {
                continue;
            };
            let path = match event.file_name() {
                Some(name) => directory.join(OsStr::from_bytes(name.to_bytes())),
                None => directory,
            };
            relevant |= touches_followed_root(&[path], &self.followed_roots);
        }
    }
}

fn nearest_existing_ancestor(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .skip(1)
        .find(|ancestor| std::fs::metadata(ancestor).is_ok_and(|metadata| metadata.is_dir()))
        .map(Path::to_path_buf)
}

/// 事件路径是某个位置本身或它的上级时才算数，父目录里其他条目的变化不唤醒。
fn touches_followed_root(paths: &[PathBuf], roots: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|path| roots.iter().any(|root| root.starts_with(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_events_on_a_root_or_its_ancestors_count() {
        let roots = [PathBuf::from("/home/user/Projects/site")];

        assert!(touches_followed_root(
            &[PathBuf::from("/home/user/Projects")],
            &roots
        ));
        assert!(touches_followed_root(
            &[PathBuf::from("/home/user/Projects/site")],
            &roots
        ));
        assert!(!touches_followed_root(
            &[PathBuf::from("/home/user/notes.txt")],
            &roots
        ));
        assert!(!touches_followed_root(
            &[PathBuf::from("/home/user/Projects/site/index.html")],
            &roots
        ));
    }

    #[test]
    fn a_missing_root_is_followed_through_its_nearest_existing_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("later").join("deeper");

        assert_eq!(
            nearest_existing_ancestor(&missing),
            Some(directory.path().to_path_buf())
        );
        std::fs::create_dir(directory.path().join("later")).unwrap();
        assert_eq!(
            nearest_existing_ancestor(&missing),
            Some(directory.path().join("later"))
        );
    }
}
