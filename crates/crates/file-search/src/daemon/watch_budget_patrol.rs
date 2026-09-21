use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::path::{Path, PathBuf};

use crate::database::{SearchDatabase, MAX_KNOWN_ENTRY_PAGE_ENTRIES};
use crate::error::SearchResult;

use super::bounded_paths::BoundedPathSet;

pub(super) const WATCH_BUDGET_PATROL_BATCH_SIZE: usize = 128;
const MAX_QUERY_PAGES_PER_BATCH: usize = 8;

#[derive(Debug)]
struct RootPatrolCursor {
    first_unwatched_path: Option<PathBuf>,
    after_path: Option<PathBuf>,
}

impl RootPatrolCursor {
    fn new(first_unwatched_path: PathBuf) -> Self {
        Self {
            first_unwatched_path: Some(first_unwatched_path),
            after_path: None,
        }
    }
}

pub(super) struct WatchBudgetPatrol {
    database: SearchDatabase,
    root_cursors: BTreeMap<PathBuf, RootPatrolCursor>,
    previous_root: Option<PathBuf>,
}

impl WatchBudgetPatrol {
    pub(super) fn open(database_path: &Path) -> SearchResult<Self> {
        Ok(Self {
            database: SearchDatabase::open_read_only(database_path)?,
            root_cursors: BTreeMap::new(),
            previous_root: None,
        })
    }

    pub(super) fn next_directories(
        &mut self,
        overflow_roots: &BTreeMap<PathBuf, PathBuf>,
        registered_directories: &BoundedPathSet,
    ) -> SearchResult<Vec<PathBuf>> {
        self.synchronize_roots(overflow_roots);
        let Some(root) = self.next_root(overflow_roots) else {
            return Ok(Vec::new());
        };
        let cursor = self
            .root_cursors
            .get_mut(&root)
            .expect("watch budget patrol root cursor missing");
        let mut directories = Vec::with_capacity(WATCH_BUDGET_PATROL_BATCH_SIZE);

        if let Some(first_unwatched_path) = cursor.first_unwatched_path.take() {
            cursor.after_path = Some(first_unwatched_path.clone());
            if !registered_directories.contains(&first_unwatched_path) {
                directories.push(first_unwatched_path);
            }
        }

        for _ in 0..MAX_QUERY_PAGES_PER_BATCH {
            if directories.len() >= WATCH_BUDGET_PATROL_BATCH_SIZE {
                break;
            }
            let query_limit = (WATCH_BUDGET_PATROL_BATCH_SIZE - directories.len())
                .min(MAX_KNOWN_ENTRY_PAGE_ENTRIES);
            let page = self.database.observable_directory_paths_for_root_page(
                &root,
                cursor.after_path.as_deref(),
                query_limit,
            )?;
            if page.is_empty() {
                cursor.after_path = None;
                break;
            }
            let reached_end = page.len() < query_limit;
            cursor.after_path = page.last().cloned();
            directories.extend(
                page.into_iter()
                    .filter(|path| !registered_directories.contains(path))
                    .take(WATCH_BUDGET_PATROL_BATCH_SIZE - directories.len()),
            );
            if reached_end {
                cursor.after_path = None;
                break;
            }
        }

        Ok(directories)
    }

    fn synchronize_roots(&mut self, overflow_roots: &BTreeMap<PathBuf, PathBuf>) {
        self.root_cursors
            .retain(|root, _| overflow_roots.contains_key(root));
        for (root, first_unwatched_path) in overflow_roots {
            if let Entry::Vacant(entry) = self.root_cursors.entry(root.clone()) {
                entry.insert(RootPatrolCursor::new(first_unwatched_path.clone()));
            }
        }
        if self
            .previous_root
            .as_ref()
            .is_some_and(|root| !overflow_roots.contains_key(root))
        {
            self.previous_root = None;
        }
    }

    fn next_root(&mut self, overflow_roots: &BTreeMap<PathBuf, PathBuf>) -> Option<PathBuf> {
        let next = self
            .previous_root
            .as_ref()
            .and_then(|previous| {
                overflow_roots
                    .range((Excluded(previous.clone()), Unbounded))
                    .next()
            })
            .or_else(|| overflow_roots.first_key_value())
            .map(|(root, _)| root.clone());
        self.previous_root = next.clone();
        next
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use crate::database::{
        DirectorySignature, DirectorySnapshot, EntryObservationState, SearchDatabase,
    };

    use super::*;

    fn snapshot(path: PathBuf, root: &Path, inode: u64) -> DirectorySnapshot {
        DirectorySnapshot {
            parent_path: path.parent().unwrap_or(root).to_path_buf(),
            path,
            root_path: root.to_path_buf(),
            signature: DirectorySignature {
                device: 1,
                inode,
                mtime_ns: 1,
                ctime_ns: 1,
            },
            observation_state: EntryObservationState::Observable,
        }
    }

    #[test]
    fn watched_prefix_scanning_is_bounded_and_keeps_cursor_progress() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("search.sqlite");
        let root = directory.path().join("root");
        let database = SearchDatabase::open(&database_path).unwrap();
        for position in 0..=1_025 {
            database
                .upsert_directory_snapshot(&snapshot(
                    root.join(format!("d{position:04}")),
                    &root,
                    position + 1,
                ))
                .unwrap();
        }
        drop(database);

        let mut registered = BoundedPathSet::new(1_025, usize::MAX);
        for position in 0..=1_024 {
            registered
                .insert(root.join(format!("d{position:04}")))
                .unwrap();
        }
        let overflow_roots = BTreeMap::from([(root.clone(), root.join("d0000"))]);
        let mut patrol = WatchBudgetPatrol::open(&database_path).unwrap();

        assert!(patrol
            .next_directories(&overflow_roots, &registered)
            .unwrap()
            .is_empty());
        assert_eq!(
            patrol
                .next_directories(&overflow_roots, &registered)
                .unwrap(),
            vec![root.join("d1025")]
        );
    }

    #[test]
    fn watch_budget_patrol_pages_only_unwatched_directories_for_the_overflow_root() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("search.sqlite");
        let first_root = directory.path().join("first-root");
        let second_root = directory.path().join("second-root");
        let first_root_directory = first_root.join("d000");
        let first_unwatched = first_root.join("d001");
        let database = SearchDatabase::open(&database_path).unwrap();
        database
            .upsert_directory_snapshot(&snapshot(first_root.clone(), &first_root, 1))
            .unwrap();
        for position in 0..300 {
            database
                .upsert_directory_snapshot(&snapshot(
                    first_root.join(format!("d{position:03}")),
                    &first_root,
                    position + 2,
                ))
                .unwrap();
        }
        database
            .upsert_directory_snapshot(&snapshot(second_root.clone(), &second_root, 500))
            .unwrap();
        database
            .upsert_directory_snapshot(&snapshot(second_root.join("excluded"), &second_root, 501))
            .unwrap();
        drop(database);

        let mut registered = BoundedPathSet::new(2, usize::MAX);
        registered.insert(first_root.clone()).unwrap();
        registered.insert(first_root_directory).unwrap();
        let overflow_roots = BTreeMap::from([(first_root.clone(), first_unwatched.clone())]);
        let mut patrol = WatchBudgetPatrol::open(&database_path).unwrap();

        let first_page = patrol
            .next_directories(&overflow_roots, &registered)
            .unwrap();
        let second_page = patrol
            .next_directories(&overflow_roots, &registered)
            .unwrap();
        let final_page = patrol
            .next_directories(&overflow_roots, &registered)
            .unwrap();
        let wrapped_page = patrol
            .next_directories(&overflow_roots, &registered)
            .unwrap();

        assert_eq!(first_page.len(), WATCH_BUDGET_PATROL_BATCH_SIZE);
        assert_eq!(first_page.first(), Some(&first_unwatched));
        assert!(first_page.iter().all(|path| path.starts_with(&first_root)));
        assert_eq!(second_page.len(), WATCH_BUDGET_PATROL_BATCH_SIZE);
        assert!(second_page.iter().all(|path| path.starts_with(&first_root)));
        assert!(first_page.iter().all(|path| !second_page.contains(path)));
        assert_eq!(final_page.len(), 43);
        assert!(final_page.iter().all(|path| path.starts_with(&first_root)));
        assert_eq!(wrapped_page, first_page);
    }
}
