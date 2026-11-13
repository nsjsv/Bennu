use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use super::*;

fn watch(value: i32) -> WatchDescriptor {
    WatchDescriptor::new(value).unwrap()
}

fn tree_with(paths: &[(&str, i32)]) -> WatchedTree {
    let mut tree = WatchedTree::default();
    for (path, descriptor) in paths {
        assert!(tree.insert(Path::new(path), watch(*descriptor)));
    }
    tree
}

#[test]
fn watched_directories_resolve_in_both_directions() {
    let tree = tree_with(&[
        ("/home/me", 1),
        ("/home/me/docs", 2),
        ("/home/me/docs/2024", 3),
    ]);

    assert!(tree.is_watched(Path::new("/home/me/docs")));
    assert!(!tree.is_watched(Path::new("/home")));
    assert!(!tree.is_watched(Path::new("/home/me/music")));
    assert_eq!(
        tree.path_of(watch(3)),
        Some(PathBuf::from("/home/me/docs/2024"))
    );
    assert_eq!(tree.path_of(watch(9)), None);
    assert_eq!(tree.watch_count(), 3);
}

#[test]
fn a_scope_is_unwatched_deepest_first_and_its_empty_ancestors_go_away() {
    let mut tree = tree_with(&[
        ("/r/a", 1),
        ("/r/a/b", 2),
        ("/r/a/b/c", 3),
        ("/r/a/d", 4),
        ("/r/keep", 5),
    ]);

    let removed = tree.remove_scope(Path::new("/r/a"));

    assert_eq!(removed.last(), Some(&watch(1)));
    let mut sorted = removed.clone();
    sorted.sort();
    assert_eq!(sorted, [watch(1), watch(2), watch(3), watch(4)]);
    let position = |value: i32| removed.iter().position(|w| *w == watch(value)).unwrap();
    assert!(position(3) < position(2));
    assert!(!tree.is_watched(Path::new("/r/a/b")));
    assert!(tree.is_watched(Path::new("/r/keep")));
    assert_eq!(tree.watch_count(), 1);
    assert!(tree.remove_scope(Path::new("/r/missing")).is_empty());

    tree.forget(watch(5));
    assert_eq!(tree.watch_count(), 0);
    assert_eq!(tree.nodes[ROOT as usize].children.len(), 0);
}

#[test]
fn a_moved_directory_keeps_one_watch_at_its_new_path() {
    let mut tree = tree_with(&[("/r/old", 7)]);

    assert!(tree.insert(Path::new("/r/new"), watch(7)));

    assert_eq!(tree.path_of(watch(7)), Some(PathBuf::from("/r/new")));
    assert!(!tree.is_watched(Path::new("/r/old")));
    assert!(tree.remove_scope(Path::new("/r/old")).is_empty());
    assert!(tree.is_watched(Path::new("/r/new")));
}

#[test]
fn shared_prefixes_are_stored_once() {
    let mut tree = WatchedTree::default();
    for index in 0..1_000 {
        let path = format!("/home/someone/a-rather-long-directory-name/project/{index:04}");
        assert!(tree.insert(Path::new(&path), watch(index + 1)));
    }

    // 1000 个四字名字加一次公共前缀，而不是 1000 份完整路径。
    assert!(tree.names.len() < 1_000 * 4 + 64);
}

#[test]
fn non_utf8_names_round_trip() {
    let path = PathBuf::from(OsString::from_vec(b"/r/\xff\xfe".to_vec()));
    let mut tree = WatchedTree::default();

    assert!(tree.insert(&path, watch(1)));

    assert_eq!(tree.path_of(watch(1)), Some(path.clone()));
    assert!(tree.is_watched(&path));
}

#[test]
fn relative_or_dotted_paths_are_not_watch_identities() {
    let mut tree = WatchedTree::default();
    assert!(!tree.insert(Path::new("relative/dir"), watch(1)));
    assert!(!tree.insert(Path::new("/r/../escape"), watch(2)));
    assert_eq!(tree.watch_count(), 0);
}

#[test]
fn removed_names_are_compacted_once_they_dominate() {
    let mut tree = WatchedTree::default();
    let long_name = "n".repeat(200);
    for index in 0..1_000 {
        tree.insert(
            Path::new(&format!("/r/{long_name}{index}")),
            watch(index + 1),
        );
    }
    tree.insert(Path::new("/keep/me"), watch(5_000));

    tree.remove_scope(Path::new("/r"));

    assert_eq!(tree.garbage_name_bytes, 0);
    assert!(tree.names.len() < 64);
    assert_eq!(tree.path_of(watch(5_000)), Some(PathBuf::from("/keep/me")));
    assert!(tree.insert(Path::new("/r/again"), watch(5_001)));
    assert_eq!(tree.path_of(watch(5_001)), Some(PathBuf::from("/r/again")));
}
