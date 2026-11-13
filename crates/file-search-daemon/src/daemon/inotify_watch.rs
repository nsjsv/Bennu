//! 搜索服务自己的 inotify 后端：一个 inotify 描述符、一棵被监听目录的树、一个读事件线程。
//! 不借 notify：它给每个监听目录按完整路径各存两张表，7.9 万个目录时光这两份就占二十多 MB。
//! 注册监听的标志与 notify 6.1 相同，监听到的变化不变。

use std::ffi::OsStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;

use crate::error::{SearchError, SearchResult};

use super::watch_coverage::DirectoryWatchBackend;
use super::watch_ingress::{deliver_watch_event, WatchOverflowState};
use super::watched_tree::{WatchDescriptor, WatchedTree};

const EVENT_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WatchEvent {
    /// 被监听目录里的一项、或目录本身变了。
    Changed(PathBuf),
    /// 内核事件队列溢出，变化的身份丢了。
    QueueOverflow,
    /// 读事件失败，监听不再可用。
    Failed(String),
}

/// notify 6.1 对非递归监听用的同一组标志。不加 DONT_FOLLOW：搜索根可以是指向目录的链接。
pub(super) fn directory_watch_flags() -> WatchFlags {
    WatchFlags::ATTRIB
        | WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::MODIFY
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::DELETE_SELF
        | WatchFlags::MOVE_SELF
}

pub(super) struct InotifyDirectories {
    inotify: Arc<OwnedFd>,
    tree: Arc<Mutex<WatchedTree>>,
    stop: Arc<OwnedFd>,
    reader: Option<JoinHandle<()>>,
}

impl InotifyDirectories {
    pub(super) fn spawn(
        event_sender: SyncSender<WatchEvent>,
        overflow_state: Arc<WatchOverflowState>,
    ) -> SearchResult<Self> {
        let setup_failed = |error: Errno| {
            SearchError::WorkerFailed(format!("could not start filesystem watching: {error}"))
        };
        let inotify = Arc::new(
            inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).map_err(setup_failed)?,
        );
        let stop = Arc::new(
            eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).map_err(setup_failed)?,
        );
        let tree = Arc::new(Mutex::new(WatchedTree::default()));
        let reader = thread::Builder::new()
            .name("file-search-inotify".to_owned())
            .spawn({
                let inotify = Arc::clone(&inotify);
                let stop = Arc::clone(&stop);
                let tree = Arc::clone(&tree);
                move || read_events(&inotify, &stop, &tree, &event_sender, &overflow_state)
            })
            .map_err(|error| {
                SearchError::WorkerFailed(format!("could not start filesystem watching: {error}"))
            })?;
        Ok(Self {
            inotify,
            tree,
            stop,
            reader: Some(reader),
        })
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.tree().heap_bytes()
    }

    pub(super) fn shrink_to_fit(&self) {
        self.tree().shrink_to_fit();
    }

    fn tree(&self) -> MutexGuard<'_, WatchedTree> {
        self.tree
            .lock()
            .expect("watched directory tree mutex poisoned")
    }
}

impl DirectoryWatchBackend for InotifyDirectories {
    fn is_watched(&self, directory: &Path) -> bool {
        self.tree().is_watched(directory)
    }

    fn watch_directory(&mut self, directory: &Path) -> io::Result<()> {
        // 注册和记进树在同一把锁里：新目录里马上出现的事件，读线程一定解析得出路径。
        let mut tree = self.tree();
        let descriptor = inotify::add_watch(&*self.inotify, directory, directory_watch_flags())?;
        let watch = WatchDescriptor::new(descriptor)
            .ok_or_else(|| io::Error::other("inotify returned watch descriptor 0"))?;
        if !tree.insert(directory, watch) {
            let _ = inotify::remove_watch(&*self.inotify, descriptor);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a watched directory must be an absolute path",
            ));
        }
        Ok(())
    }

    fn unwatch_scope(&mut self, scope: &Path) {
        let removed = self.tree().remove_scope(scope);
        for watch in removed {
            // 目录已被删时内核早已撤掉监听，这里报错也无妨。
            let _ = inotify::remove_watch(&*self.inotify, watch.get());
        }
    }

    fn watched_count(&self) -> usize {
        self.tree().watch_count()
    }
}

impl Drop for InotifyDirectories {
    fn drop(&mut self) {
        let _ = rustix::io::write(&*self.stop, &1_u64.to_ne_bytes());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct RawEvent {
    descriptor: i32,
    flags: ReadFlags,
    name: Option<Vec<u8>>,
}

fn read_events(
    inotify: &OwnedFd,
    stop: &OwnedFd,
    tree: &Mutex<WatchedTree>,
    event_sender: &SyncSender<WatchEvent>,
    overflow_state: &WatchOverflowState,
) {
    let mut buffer = vec![MaybeUninit::<u8>::uninit(); EVENT_BUFFER_BYTES];
    let mut raw_events = Vec::new();
    loop {
        let mut descriptors = [
            PollFd::new(inotify, PollFlags::IN),
            PollFd::new(stop, PollFlags::IN),
        ];
        match poll(&mut descriptors, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => {
                let failure = WatchEvent::Failed(format!("filesystem watching stopped: {error}"));
                deliver_watch_event(event_sender, overflow_state, failure);
                return;
            }
        }
        if !descriptors[1].revents().is_empty() {
            return;
        }
        let mut reader = inotify::Reader::new(inotify, &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => raw_events.push(RawEvent {
                    descriptor: event.wd(),
                    flags: event.events(),
                    name: event.file_name().map(|name| name.to_bytes().to_vec()),
                }),
                Err(Errno::AGAIN) => break,
                Err(Errno::INTR) => continue,
                Err(error) => {
                    let failure =
                        WatchEvent::Failed(format!("filesystem watching stopped: {error}"));
                    deliver_watch_event(event_sender, overflow_state, failure);
                    return;
                }
            }
            if reader.is_buffer_empty() {
                break;
            }
        }
        // 一批事件在一次加锁里解析，与注册监听互斥；发送放到锁外。
        let events = {
            let mut tree = tree.lock().expect("watched directory tree mutex poisoned");
            raw_events
                .drain(..)
                .filter_map(|raw| resolve(&mut tree, raw))
                .collect::<Vec<_>>()
        };
        for event in events {
            deliver_watch_event(event_sender, overflow_state, event);
        }
    }
}

/// 原始事件 → 下游关心的变化。内核撤掉监听（`IN_IGNORED`）只更新树；同一批里它总排在
/// 目录自删事件之后，所以自删事件还能解析出路径。
fn resolve(tree: &mut WatchedTree, raw: RawEvent) -> Option<WatchEvent> {
    if raw.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
        return Some(WatchEvent::QueueOverflow);
    }
    let watch = WatchDescriptor::new(raw.descriptor)?;
    if raw.flags.contains(ReadFlags::IGNORED) {
        tree.forget(watch);
        return None;
    }
    let directory = tree.path_of(watch)?;
    Some(WatchEvent::Changed(match raw.name {
        Some(name) => directory.join(OsStr::from_bytes(&name)),
        None => directory,
    }))
}

#[cfg(test)]
#[path = "inotify_watch_tests.rs"]
mod tests;
