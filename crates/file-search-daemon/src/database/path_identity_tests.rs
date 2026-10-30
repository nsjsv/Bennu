use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::extractor::ExtractionStatus;
use crate::model::{SearchFileKind, SearchMatchMode, SearchQuery, SearchScope, SearchTextScope};

use super::{
    DirectorySignature, DirectorySnapshot, EntryObservationState, EntryStageProgress,
    IndexedEntryStageState, IndexedFile, SearchDatabase,
};

fn indexed_file(path: PathBuf, content: &str) -> IndexedFile {
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        path,
        display_name: "note.txt".to_owned(),
        kind: SearchFileKind::File,
        size: content.len() as u64,
        modified_ms: Some(1),
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: Some(content.to_owned()),
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(1),
        mtime_ns: Some(1),
        ctime_ns: Some(1),
    }
}

#[test]
fn non_utf8_paths_with_the_same_lossy_text_keep_independent_identity() {
    let database = SearchDatabase::in_memory().unwrap();
    let first_root = PathBuf::from(OsString::from_vec(b"/tmp/\x80".to_vec()));
    let second_root = PathBuf::from(OsString::from_vec(b"/tmp/\x81".to_vec()));
    assert_eq!(first_root.to_string_lossy(), second_root.to_string_lossy());

    let first_path = first_root.join("note.txt");
    let second_path = second_root.join("note.txt");
    database
        .upsert_file(&indexed_file(first_path.clone(), "shared needle"))
        .unwrap();
    database
        .upsert_file(&indexed_file(second_path.clone(), "shared needle"))
        .unwrap();
    for (root, inode) in [(&first_root, 10), (&second_root, 11)] {
        database
            .upsert_directory_snapshot(&DirectorySnapshot {
                path: root.clone(),
                parent_path: PathBuf::from("/tmp"),
                root_path: root.clone(),
                signature: DirectorySignature {
                    device: 1,
                    inode,
                    mtime_ns: 1,
                    ctime_ns: 1,
                },
                observation_state: EntryObservationState::Observable,
            })
            .unwrap();
    }

    let indexed_paths = database
        .search(&SearchQuery::global(1, "needle"))
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        indexed_paths,
        BTreeSet::from([first_path.clone(), second_path.clone()])
    );
    assert!(database.entry_stage_state(&first_path).unwrap().is_some());
    assert!(database.entry_stage_state(&second_path).unwrap().is_some());

    assert!(database.mark_scope_inaccessible(&first_root).unwrap());
    assert_eq!(
        database
            .search(&SearchQuery::global(2, "needle"))
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.path)
            .collect::<Vec<_>>(),
        vec![second_path.clone()]
    );

    database.delete_scope(&first_root).unwrap();

    assert!(database.entry_stage_state(&first_path).unwrap().is_none());
    assert!(database.entry_stage_state(&second_path).unwrap().is_some());
    assert!(database.directory_snapshot(&first_root).unwrap().is_none());
    assert!(database.directory_snapshot(&second_root).unwrap().is_some());
    assert_eq!(
        database
            .search(&SearchQuery::global(3, "needle"))
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.path)
            .collect::<Vec<_>>(),
        vec![second_path]
    );
}

#[test]
fn recursive_non_utf8_scope_uses_raw_component_boundaries() {
    let database = SearchDatabase::in_memory().unwrap();
    let scope = PathBuf::from(OsString::from_vec(b"/tmp/\x80".to_vec()));
    let direct_child = scope.join("direct.txt");
    let nested_child = scope.join("nested").join("deep.txt");
    let prefix_sibling = PathBuf::from(OsString::from_vec(b"/tmp/\x80-other/note.txt".to_vec()));
    let lossy_sibling = PathBuf::from(OsString::from_vec(b"/tmp/\x81/note.txt".to_vec()));
    for path in [
        &direct_child,
        &nested_child,
        &prefix_sibling,
        &lossy_sibling,
    ] {
        database
            .upsert_file(&indexed_file(path.clone(), ""))
            .unwrap();
    }
    let nested_directory = scope.join("nested");
    let prefix_directory = prefix_sibling.parent().unwrap().to_path_buf();
    let lossy_directory = lossy_sibling.parent().unwrap().to_path_buf();
    for (index, (path, parent_path, root_path)) in [
        (scope.clone(), PathBuf::from("/tmp"), scope.clone()),
        (nested_directory.clone(), scope.clone(), scope.clone()),
        (
            prefix_directory.clone(),
            PathBuf::from("/tmp"),
            prefix_directory,
        ),
        (
            lossy_directory.clone(),
            PathBuf::from("/tmp"),
            lossy_directory,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        database
            .upsert_directory_snapshot(&DirectorySnapshot {
                path,
                parent_path,
                root_path,
                signature: DirectorySignature {
                    device: 1,
                    inode: index as u64 + 1,
                    mtime_ns: 1,
                    ctime_ns: 1,
                },
                observation_state: EntryObservationState::Observable,
            })
            .unwrap();
    }
    let query = SearchQuery {
        query_id: 1,
        terms: String::new(),
        text_scope: SearchTextScope::NameAndContent,
        match_mode: SearchMatchMode::Plain,
        scope: SearchScope::Directory(scope.clone()),
        recursive: true,
        filters: Default::default(),
        limit: 100,
        cursor: None,
    };

    let paths = database
        .search(&query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        paths,
        BTreeSet::from([direct_child.clone(), nested_child.clone()])
    );
    assert_eq!(
        database
            .known_files_page(&scope, None, 128)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([direct_child.clone(), nested_child])
    );
    assert_eq!(
        database
            .directory_snapshots_page(&scope, None, 128)
            .unwrap()
            .into_iter()
            .map(|snapshot| snapshot.path)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([scope.clone(), nested_directory.clone()])
    );
    assert_eq!(
        database
            .direct_children_page(&scope, None, 128)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([direct_child, nested_directory])
    );
}
