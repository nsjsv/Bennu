use std::path::{Path, PathBuf};

use file_core::{DirectoryEntryChanges, DiscoveredDirectoryEntry, FileKind, ResolvedEntryChange};

use super::*;

fn node(path: &str) -> FolderNode {
    FolderNode {
        path: PathBuf::from(path),
        name: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    }
}

/// 把请求按给的子文件夹读完。
fn answer(tree: &mut FolderTree, request: &ListingRequest, children: &[&str]) {
    tree.accept_listing(
        &request.directory,
        request.request,
        Some(children.iter().map(|path| node(path)).collect()),
    );
}

fn row_names(tree: &FolderTree) -> Vec<String> {
    tree.rows()
        .iter()
        .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
        .collect()
}

fn entry(path: &str, kind: FileKind) -> DiscoveredDirectoryEntry {
    let metadata = std::fs::metadata("/").unwrap();
    DiscoveredDirectoryEntry::with_complete_filesystem_metadata(
        PathBuf::from(path),
        Path::new(path).file_name().unwrap().to_owned(),
        kind,
        false,
        false,
        &metadata,
    )
}

fn changes(directory: &str, changes: Vec<ResolvedEntryChange>) -> DirectoryEntryChanges {
    DirectoryEntryChanges {
        directory: PathBuf::from(directory),
        changes,
    }
}

/// 根是 /home/me，跟到 /home/me/work/code，各层都读完。
fn followed_tree() -> FolderTree {
    let mut tree = FolderTree::new(PathBuf::from("/home/me"), false);
    let requests = tree.follow(Path::new("/home/me/work/code"));
    let directories: Vec<_> = requests
        .iter()
        .map(|request| request.directory.clone())
        .collect();
    assert_eq!(
        directories,
        [PathBuf::from("/home/me"), PathBuf::from("/home/me/work")],
        "the root first, then each level above the current folder"
    );
    answer(
        &mut tree,
        &requests[0],
        &["/home/me/music", "/home/me/work"],
    );
    answer(
        &mut tree,
        &requests[1],
        &["/home/me/work/code", "/home/me/work/notes"],
    );
    tree
}

#[test]
fn the_root_is_home_inside_home_and_slash_outside_it() {
    let home = Path::new("/home/me");
    assert_eq!(tree_root(Path::new("/home/me/a"), home, true), home);
    assert_eq!(tree_root(home, home, true), home);
    assert_eq!(tree_root(Path::new("/etc"), home, true), Path::new("/"));
    assert_eq!(
        tree_root(Path::new("/home/me/a"), home, false),
        Path::new("/")
    );
}

#[test]
fn following_a_folder_opens_the_levels_above_it_and_reveals_it_once() {
    let mut tree = followed_tree();

    assert_eq!(row_names(&tree), ["music", "work", "  code", "  notes"]);
    assert_eq!(tree.current(), Some(Path::new("/home/me/work/code")));
    assert_eq!(tree.take_reveal(), Some(2));
    assert_eq!(tree.take_reveal(), None);
}

#[test]
fn the_reveal_waits_for_the_levels_above_and_gives_up_when_they_are_read() {
    let mut tree = FolderTree::new(PathBuf::from("/home/me"), false);
    let requests = tree.follow(Path::new("/home/me/work/code"));
    answer(&mut tree, &requests[0], &["/home/me/work"]);
    assert_eq!(tree.take_reveal(), None, "work is still being read");
    // 读完了却没有它（比如是隐藏文件夹）：不再等。
    answer(&mut tree, &requests[1], &[]);
    assert_eq!(tree.take_reveal(), None);
    assert!(!tree.reveal_pending);
}

#[test]
fn collapsing_forgets_what_was_open_below_and_stops_watching_it() {
    let mut tree = followed_tree();
    assert!(tree
        .watched_directories()
        .any(|path| path == Path::new("/home/me/work")));

    assert_eq!(tree.toggle(Path::new("/home/me/work")), None);

    assert_eq!(row_names(&tree), ["music", "work"]);
    assert!(!tree
        .watched_directories()
        .any(|path| path == Path::new("/home/me/work")));
    let expand = tree.toggle(Path::new("/home/me/work")).expect("read again");
    assert_eq!(expand.directory, Path::new("/home/me/work"));
    assert_eq!(tree.rows()[1].expander, Expander::Loading);
}

#[test]
fn an_answer_to_an_older_request_is_dropped() {
    let mut tree = FolderTree::new(PathBuf::from("/home/me"), false);
    let first = tree.list_root().unwrap();
    let second = tree.relist_all(true).remove(0);
    answer(&mut tree, &first, &["/home/me/old"]);
    assert!(tree.rows().is_empty());
    answer(&mut tree, &second, &["/home/me/.hidden", "/home/me/new"]);
    assert_eq!(row_names(&tree), [".hidden", "new"]);
    assert!(second.include_hidden);
}

#[test]
fn only_changes_to_folders_read_the_level_again() {
    let mut tree = followed_tree();

    let file = changes(
        "/home/me/work",
        vec![ResolvedEntryChange::Added(entry(
            "/home/me/work/a.txt",
            FileKind::File,
        ))],
    );
    assert_eq!(tree.accept_changes(&file), None);

    let folder = changes(
        "/home/me/work",
        vec![ResolvedEntryChange::Added(entry(
            "/home/me/work/new",
            FileKind::Directory,
        ))],
    );
    let request = tree.accept_changes(&folder).expect("a folder came in");
    assert_eq!(request.directory, Path::new("/home/me/work"));

    let elsewhere = changes("/tmp", vec![ResolvedEntryChange::RescanRequired]);
    assert_eq!(tree.accept_changes(&elsewhere), None, "not in the tree");
}

#[test]
fn a_removed_folder_takes_what_was_open_inside_it_along() {
    let mut tree = followed_tree();
    let removed = changes(
        "/home/me",
        vec![ResolvedEntryChange::Removed {
            path: PathBuf::from("/home/me/work"),
        }],
    );

    assert!(tree.accept_changes(&removed).is_some());

    assert!(!tree
        .watched_directories()
        .any(|path| path.starts_with("/home/me/work")));
}

#[test]
fn a_row_outside_the_viewport_is_scrolled_to_the_middle() {
    let mut tree = FolderTree::new(PathBuf::from("/"), false);
    tree.viewport_height = FOLDER_ROW_HEIGHT * 10.0;
    assert_eq!(tree.scroll_target_for_row(3), None);
    let target = tree.scroll_target_for_row(40).unwrap();
    assert_eq!(target, FOLDER_ROW_HEIGHT * 40.0 - FOLDER_ROW_HEIGHT * 4.5);
}
