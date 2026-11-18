use std::path::PathBuf;

use file_core::{EntryMetadata, FileKind};

use super::*;

fn entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn names(snapshot: &DirectoryEntrySnapshot, paths: &[&str]) -> Vec<Option<String>> {
    paths
        .iter()
        .map(|path| {
            snapshot
                .find(Path::new(path))
                .map(|entry| entry.path().display().to_string())
        })
        .collect()
}

#[test]
fn entries_are_found_by_path() {
    let snapshot: DirectoryEntrySnapshot = (0..1000)
        .map(|index| entry(&format!("/dir/file-{index}")))
        .collect();

    assert_eq!(
        names(
            &snapshot,
            &["/dir/file-0", "/dir/file-999", "/dir/file-1000"]
        ),
        [
            Some("/dir/file-0".to_owned()),
            Some("/dir/file-999".to_owned()),
            None
        ]
    );
}

/// 改了条目（这里是原地改名、删一项）之后按新内容找，索引不会留着旧位置。
#[test]
fn changing_entries_drops_the_index() {
    let mut snapshot =
        DirectoryEntrySnapshot::from(vec![entry("/dir/a"), entry("/dir/b"), entry("/dir/c")]);
    assert!(snapshot.find(Path::new("/dir/b")).is_some());

    snapshot.make_mut()[1].relocate(PathBuf::from("/dir/renamed"));
    snapshot.make_mut().remove(0);

    assert_eq!(
        names(&snapshot, &["/dir/b", "/dir/renamed", "/dir/c", "/dir/a"]),
        [
            None,
            Some("/dir/renamed".to_owned()),
            Some("/dir/c".to_owned()),
            None
        ]
    );
}

/// 克隆共用条目；改其中一份先复制，另一份照旧能按原来的内容找到。
#[test]
fn clones_share_entries_until_one_changes() {
    let original = DirectoryEntrySnapshot::from(vec![entry("/dir/a"), entry("/dir/b")]);
    assert!(original.find(Path::new("/dir/a")).is_some());
    let mut changed = original.clone();
    assert!(changed.shares_entries_with(&original));

    changed
        .make_mut()
        .retain(|entry| entry.path() != Path::new("/dir/a"));

    assert!(!changed.shares_entries_with(&original));
    assert!(changed.find(Path::new("/dir/a")).is_none());
    assert!(original.find(Path::new("/dir/a")).is_some());
}

/// 版本号认内容：克隆沿用，任何一次改动（哪怕独占、原地改）都换新号，两份不同的快照号不同。
#[test]
fn revision_changes_with_content_only() {
    let mut snapshot = DirectoryEntrySnapshot::from(vec![entry("/dir/a")]);
    let other = DirectoryEntrySnapshot::from(vec![entry("/dir/a")]);
    let clone = snapshot.clone();
    assert_eq!(clone.revision(), snapshot.revision());
    assert_ne!(other.revision(), snapshot.revision());

    drop(clone);
    let before = snapshot.revision();
    snapshot.make_mut()[0].relocate(PathBuf::from("/dir/b"));
    assert_ne!(snapshot.revision(), before);
}
