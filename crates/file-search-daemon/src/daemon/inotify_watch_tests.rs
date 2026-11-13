use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tempfile::tempdir;

use super::*;

fn spawn_backend() -> (InotifyDirectories, Receiver<WatchEvent>) {
    let (sender, receiver) = mpsc::sync_channel(256);
    let overflow_state = Arc::new(WatchOverflowState::new(Vec::new()));
    (
        InotifyDirectories::spawn(sender, overflow_state).unwrap(),
        receiver,
    )
}

/// 等到收到 `expected` 为止；中间别的事件跳过。
fn wait_for(receiver: &Receiver<WatchEvent>, expected: &WatchEvent) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if receiver.recv_timeout(remaining).ok().as_ref() == Some(expected) {
            return;
        }
    }
    panic!("did not receive {expected:?}");
}

#[test]
fn a_change_names_the_entry_inside_the_watched_directory() {
    let directory = tempdir().unwrap();
    let (mut backend, receiver) = spawn_backend();
    backend.watch_directory(directory.path()).unwrap();

    std::fs::write(directory.path().join("note.txt"), "hello").unwrap();

    wait_for(
        &receiver,
        &WatchEvent::Changed(directory.path().join("note.txt")),
    );
    assert!(backend.is_watched(directory.path()));
    assert_eq!(backend.watched_count(), 1);
}

#[test]
fn a_moved_directory_keeps_reporting_under_its_new_path() {
    let directory = tempdir().unwrap();
    let old = directory.path().join("old");
    let new = directory.path().join("new");
    std::fs::create_dir(&old).unwrap();
    let (mut backend, receiver) = spawn_backend();
    backend.watch_directory(directory.path()).unwrap();
    backend.watch_directory(&old).unwrap();

    std::fs::rename(&old, &new).unwrap();
    wait_for(&receiver, &WatchEvent::Changed(new.clone()));
    // 覆盖逻辑看到移动后会先在新位置注册、再撤旧位置；同一个 inode 的监听不能被撤掉。
    backend.watch_directory(&new).unwrap();
    backend.unwatch_scope(&old);
    std::fs::write(new.join("inside.txt"), "moved").unwrap();

    wait_for(&receiver, &WatchEvent::Changed(new.join("inside.txt")));
    assert!(backend.is_watched(&new));
    assert!(!backend.is_watched(&old));
}

#[test]
fn a_deleted_directory_is_reported_and_then_forgotten() {
    let directory = tempdir().unwrap();
    let child = directory.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let (mut backend, receiver) = spawn_backend();
    backend.watch_directory(directory.path()).unwrap();
    backend.watch_directory(&child).unwrap();

    std::fs::remove_dir(&child).unwrap();

    wait_for(&receiver, &WatchEvent::Changed(child.clone()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while backend.is_watched(&child) {
        assert!(
            Instant::now() < deadline,
            "the kernel never dropped the watch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(backend.watched_count(), 1);
}

#[test]
fn queue_overflow_and_dropped_watches_are_translated() {
    let mut tree = WatchedTree::default();
    let watch = WatchDescriptor::new(3).unwrap();
    tree.insert(Path::new("/r/dir"), watch);

    let overflow = RawEvent {
        descriptor: -1,
        flags: ReadFlags::QUEUE_OVERFLOW,
        name: None,
    };
    assert_eq!(
        resolve(&mut tree, overflow),
        Some(WatchEvent::QueueOverflow)
    );

    let named = RawEvent {
        descriptor: 3,
        flags: ReadFlags::CREATE,
        name: Some(b"a.txt".to_vec()),
    };
    assert_eq!(
        resolve(&mut tree, named),
        Some(WatchEvent::Changed(PathBuf::from("/r/dir/a.txt")))
    );

    let ignored = RawEvent {
        descriptor: 3,
        flags: ReadFlags::IGNORED,
        name: None,
    };
    assert_eq!(resolve(&mut tree, ignored), None);
    assert!(!tree.is_watched(Path::new("/r/dir")));

    let unknown = RawEvent {
        descriptor: 9,
        flags: ReadFlags::MODIFY,
        name: None,
    };
    assert_eq!(resolve(&mut tree, unknown), None);
}

#[test]
fn dropping_the_backend_stops_its_reader_promptly() {
    let (backend, _receiver) = spawn_backend();
    let started = Instant::now();
    drop(backend);
    assert!(started.elapsed() < Duration::from_secs(2));
}
