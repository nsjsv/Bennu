use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension, Transaction};

use crate::error::{SearchError, SearchResult};
use crate::path_encoding::{path_from_storage, storage_bytes};
use crate::{SearchPathDecision, SearchPathPolicy, VersionedSearchPathPreferences};

use super::{recursive_storage_range, SearchDatabase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchRootMount {
    pub(crate) root_path: PathBuf,
    pub(crate) mount_point: PathBuf,
    pub(crate) device: u64,
}

impl SearchDatabase {
    pub(crate) fn initialize_search_path_configuration(
        &self,
        initial: &VersionedSearchPathPreferences,
    ) -> SearchResult<VersionedSearchPathPreferences> {
        let preferences = serde_json::to_vec(&initial.preferences)?;
        self.connection.execute(
            "INSERT INTO search_path_configuration(singleton, revision, preferences)
             VALUES (1, ?1, ?2)
             ON CONFLICT(singleton) DO NOTHING",
            params![revision_to_i64(initial.revision)?, preferences],
        )?;
        self.read_search_path_configuration()?
            .ok_or_else(|| SearchError::InvalidDatabaseSchema {
                message: "search path configuration snapshot is missing".to_owned(),
            })
    }

    pub(crate) fn read_search_path_configuration(
        &self,
    ) -> SearchResult<Option<VersionedSearchPathPreferences>> {
        self.connection
            .query_row(
                "SELECT revision, preferences
                 FROM search_path_configuration
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
            .map(|(revision, preferences)| {
                Ok(VersionedSearchPathPreferences {
                    revision: stored_revision(revision)?,
                    preferences: serde_json::from_slice(&preferences)?,
                })
            })
            .transpose()
    }

    pub(crate) fn read_search_root_mounts(&self) -> SearchResult<Vec<SearchRootMount>> {
        let mut statement = self.connection.prepare(
            "SELECT root_path, mount_point, device
             FROM search_root_mounts ORDER BY root_path",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (root_path, mount_point, device) = row?;
            Ok(SearchRootMount {
                root_path: path_from_storage(root_path),
                mount_point: path_from_storage(mount_point),
                device: u64::try_from(device).map_err(|_| SearchError::InvalidDatabaseSchema {
                    message: format!("negative root mount device identity: {device}"),
                })?,
            })
        })
        .collect()
    }

    pub(crate) fn apply_search_path_transition(
        &self,
        effective: &VersionedSearchPathPreferences,
        policy: &SearchPathPolicy,
        mounts: &[SearchRootMount],
        unavailable_roots: &[PathBuf],
        affected_scopes: &[PathBuf],
    ) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        collect_path_transition_rows(&transaction, policy, unavailable_roots, affected_scopes)?;
        replace_search_path_snapshot(&transaction, effective)?;
        replace_search_root_mounts(&transaction, mounts)?;
        transaction.execute_batch(
            "UPDATE files
                SET observation_state = 'inaccessible'
                WHERE rowid IN (SELECT rowid FROM search_path_inaccessible_files);
             UPDATE directory_snapshots
                SET observation_state = 'inaccessible'
                WHERE path IN (SELECT path FROM search_path_inaccessible_directories);
             DELETE FROM file_search_snippets
                WHERE file_rowid IN (SELECT rowid FROM search_path_purge_files);
             DELETE FROM file_search_fts
                WHERE rowid IN (SELECT rowid FROM search_path_purge_files);
             DELETE FROM file_stage_state
                WHERE path IN (
                    SELECT files.path FROM files
                    JOIN search_path_purge_files ON search_path_purge_files.rowid = files.rowid
                );
             DELETE FROM files
                WHERE rowid IN (SELECT rowid FROM search_path_purge_files);
             DELETE FROM directory_snapshots
                WHERE path IN (SELECT path FROM search_path_purge_directories);
             DROP TABLE search_path_purge_files;
             DROP TABLE search_path_purge_directories;
             DROP TABLE search_path_inaccessible_files;
             DROP TABLE search_path_inaccessible_directories;",
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn replace_search_path_snapshot(
    transaction: &Transaction<'_>,
    versioned: &VersionedSearchPathPreferences,
) -> SearchResult<()> {
    let preferences = serde_json::to_vec(&versioned.preferences)?;
    transaction.execute(
        "INSERT INTO search_path_configuration(singleton, revision, preferences)
         VALUES (1, ?1, ?2)
         ON CONFLICT(singleton) DO UPDATE SET
             revision = excluded.revision,
             preferences = excluded.preferences",
        params![revision_to_i64(versioned.revision)?, preferences],
    )?;
    Ok(())
}

fn replace_search_root_mounts(
    transaction: &Transaction<'_>,
    mounts: &[SearchRootMount],
) -> SearchResult<()> {
    transaction.execute("DELETE FROM search_root_mounts", [])?;
    let mut statement = transaction.prepare(
        "INSERT INTO search_root_mounts(root_path, mount_point, device)
         VALUES (?1, ?2, ?3)",
    )?;
    for mount in mounts {
        statement.execute(params![
            storage_bytes(&mount.root_path),
            storage_bytes(&mount.mount_point),
            revision_to_i64(mount.device)?,
        ])?;
    }
    Ok(())
}

fn collect_path_transition_rows(
    transaction: &Transaction<'_>,
    policy: &SearchPathPolicy,
    unavailable_roots: &[PathBuf],
    affected_scopes: &[PathBuf],
) -> SearchResult<()> {
    transaction.execute_batch(
        "DROP TABLE IF EXISTS temp.search_path_purge_files;
         DROP TABLE IF EXISTS temp.search_path_purge_directories;
         DROP TABLE IF EXISTS temp.search_path_inaccessible_files;
         DROP TABLE IF EXISTS temp.search_path_inaccessible_directories;
         CREATE TEMP TABLE search_path_purge_files(rowid INTEGER PRIMARY KEY);
         CREATE TEMP TABLE search_path_purge_directories(path BLOB PRIMARY KEY) WITHOUT ROWID;
         CREATE TEMP TABLE search_path_inaccessible_files(rowid INTEGER PRIMARY KEY);
         CREATE TEMP TABLE search_path_inaccessible_directories(path BLOB PRIMARY KEY) WITHOUT ROWID;",
    )?;

    for scope in transition_scan_scopes(affected_scopes, unavailable_roots) {
        collect_file_transition_rows(transaction, policy, unavailable_roots, &scope)?;
        collect_directory_transition_rows(transaction, policy, unavailable_roots, &scope)?;
    }
    Ok(())
}

fn collect_file_transition_rows(
    transaction: &Transaction<'_>,
    policy: &SearchPathPolicy,
    unavailable_roots: &[PathBuf],
    scope: &Path,
) -> SearchResult<()> {
    let range = recursive_storage_range(scope);
    let mut after_path = Vec::<u8>::new();
    loop {
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT rowid, path FROM files
                 WHERE path > ?4
                   AND (path = ?1 OR (path >= ?2 AND path < ?3))
                 ORDER BY path LIMIT ?5",
            )?;
            let rows = statement
                .query_map(
                    params![
                        range.exact_path,
                        range.descendant_lower,
                        range.descendant_upper,
                        after_path,
                        512_i64,
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let Some((_, last_path)) = rows.last() else {
            break;
        };
        after_path = last_path.clone();
        for (rowid, path) in rows {
            match policy.decision(&path_from_storage(path)) {
                SearchPathDecision::Included { owning_root }
                    if unavailable_roots.iter().any(|root| root == owning_root) =>
                {
                    transaction.execute(
                        "INSERT OR IGNORE INTO search_path_inaccessible_files(rowid) VALUES (?1)",
                        [rowid],
                    )?;
                }
                SearchPathDecision::Included { .. } => {}
                SearchPathDecision::Excluded { .. } | SearchPathDecision::OutsideIndex => {
                    transaction.execute(
                        "INSERT OR IGNORE INTO search_path_purge_files(rowid) VALUES (?1)",
                        [rowid],
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn collect_directory_transition_rows(
    transaction: &Transaction<'_>,
    policy: &SearchPathPolicy,
    unavailable_roots: &[PathBuf],
    scope: &Path,
) -> SearchResult<()> {
    let range = recursive_storage_range(scope);
    let mut after_path = Vec::<u8>::new();
    loop {
        let paths = {
            let mut statement = transaction.prepare(
                "SELECT path FROM directory_snapshots
                 WHERE path > ?4
                   AND (path = ?1 OR (path >= ?2 AND path < ?3))
                 ORDER BY path LIMIT ?5",
            )?;
            let paths = statement
                .query_map(
                    params![
                        range.exact_path,
                        range.descendant_lower,
                        range.descendant_upper,
                        after_path,
                        512_i64,
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            paths
        };
        let Some(last_path) = paths.last() else {
            break;
        };
        after_path = last_path.clone();
        for path in paths {
            match policy.decision(&path_from_storage(path.clone())) {
                SearchPathDecision::Included { owning_root } => {
                    transaction.execute(
                        "UPDATE directory_snapshots SET root_path = ?1 WHERE path = ?2",
                        params![storage_bytes(owning_root), path],
                    )?;
                    if unavailable_roots.iter().any(|root| root == owning_root) {
                        transaction.execute(
                            "INSERT OR IGNORE INTO search_path_inaccessible_directories(path) VALUES (?1)",
                            [path],
                        )?;
                    }
                }
                SearchPathDecision::Excluded { .. } | SearchPathDecision::OutsideIndex => {
                    transaction.execute(
                        "INSERT OR IGNORE INTO search_path_purge_directories(path) VALUES (?1)",
                        [path],
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn transition_scan_scopes(
    affected_scopes: &[PathBuf],
    unavailable_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut scopes = affected_scopes
        .iter()
        .chain(unavailable_roots)
        .cloned()
        .collect::<Vec<_>>();
    scopes.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    let mut collapsed = Vec::<PathBuf>::new();
    for scope in scopes {
        if !collapsed.iter().any(|ancestor| scope.starts_with(ancestor)) {
            collapsed.push(scope);
        }
    }
    collapsed
}

fn stored_revision(revision: i64) -> SearchResult<u64> {
    u64::try_from(revision).map_err(|_| SearchError::InvalidDatabaseSchema {
        message: format!("negative search path configuration revision: {revision}"),
    })
}

fn revision_to_i64(revision: u64) -> SearchResult<i64> {
    i64::try_from(revision).map_err(|_| {
        SearchError::InvalidConfiguration(format!(
            "search path configuration revision exceeds SQLite range: {revision}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use super::*;
    use crate::SearchPathPreferences;

    #[test]
    fn initialization_keeps_an_existing_effective_snapshot() {
        let directory = tempdir().unwrap();
        let database = SearchDatabase::open(&directory.path().join("search.sqlite")).unwrap();
        let first = VersionedSearchPathPreferences {
            revision: 3,
            preferences: SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/data")],
                exclusions: Vec::new(),
            },
        };
        let later_default = VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        };

        assert_eq!(
            database
                .initialize_search_path_configuration(&first)
                .unwrap(),
            first
        );
        assert_eq!(
            database
                .initialize_search_path_configuration(&later_default)
                .unwrap(),
            first
        );
    }

    #[test]
    #[ignore = "100k-row release performance gate"]
    fn narrow_policy_transition_stays_below_three_seconds_on_a_100k_index() {
        let directory = tempdir().unwrap();
        let mut database = SearchDatabase::open(&directory.path().join("search.sqlite")).unwrap();
        let initial = VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        };
        database
            .initialize_search_path_configuration(&initial)
            .unwrap();
        let transaction = database.connection.transaction().unwrap();
        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO files(
                         path, parent_path, display_name, kind, size, content_status
                     ) VALUES (?1, ?2, ?3, 'file', 1, 'not_indexed')",
                )
                .unwrap();
            for index in 0..100_000 {
                let parent = if index < 100 {
                    Path::new("/home/private")
                } else {
                    Path::new("/home/bulk")
                };
                let name = format!("entry-{index:06}.txt");
                insert
                    .execute(params![
                        storage_bytes(&parent.join(&name)),
                        storage_bytes(parent),
                        name
                    ])
                    .unwrap();
            }
        }
        transaction.commit().unwrap();

        let preferences = SearchPathPreferences {
            custom_roots: Vec::new(),
            exclusions: vec![PathBuf::from("/home/private")],
        };
        let policy = SearchPathPolicy::new(PathBuf::from("/home"), preferences.clone()).unwrap();
        let started = Instant::now();
        database
            .apply_search_path_transition(
                &VersionedSearchPathPreferences {
                    revision: 1,
                    preferences,
                },
                &policy,
                &[SearchRootMount {
                    root_path: PathBuf::from("/home"),
                    mount_point: PathBuf::from("/"),
                    device: 1,
                }],
                &[],
                &[PathBuf::from("/home/private")],
            )
            .unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "transition took {elapsed:?}"
        );
        assert_eq!(
            database
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            99_900
        );
    }

    #[test]
    fn transition_purges_only_paths_outside_the_new_deepest_policy_and_rolls_back_atomically() {
        let directory = tempdir().unwrap();
        let database = SearchDatabase::open(&directory.path().join("search.sqlite")).unwrap();
        let initial = VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        };
        database
            .initialize_search_path_configuration(&initial)
            .unwrap();

        let paths = [
            "/home/keep.txt",
            "/home/private/drop.txt",
            "/home/private/reinclude/keep.txt",
            "/outside/not-in-diff.txt",
        ];
        for (index, path) in paths.iter().enumerate() {
            let path = PathBuf::from(path);
            let rowid = database
                .connection
                .query_row(
                    "INSERT INTO files(
                         path, parent_path, display_name, kind, size, content_status
                     ) VALUES (?1, ?2, ?3, 'file', 1, 'not_requested')
                     RETURNING rowid",
                    params![
                        storage_bytes(&path),
                        storage_bytes(path.parent().unwrap()),
                        format!("file-{index}.txt"),
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            database
                .connection
                .execute(
                    "INSERT INTO file_search_fts(rowid, name, content)
                     VALUES (?1, ?2, ?3)",
                    params![rowid, format!("file-{index}.txt"), "content"],
                )
                .unwrap();
            database
                .connection
                .execute(
                    "INSERT INTO file_search_snippets(file_rowid, preview)
                     VALUES (?1, 'content')",
                    [rowid],
                )
                .unwrap();
            database
                .connection
                .execute(
                    "INSERT INTO file_stage_state(path, metadata_stage_state, content_stage_state)
                     VALUES (?1, 'complete', 'complete')",
                    [storage_bytes(&path)],
                )
                .unwrap();
        }
        let private = PathBuf::from("/home/private");
        let reinclude = PathBuf::from("/home/private/reinclude");
        for directory_path in [&private, &reinclude] {
            database
                .connection
                .execute(
                    "INSERT INTO directory_snapshots(
                         path, parent_path, root_path, device, inode, mtime_ns, ctime_ns
                     ) VALUES (?1, ?2, ?3, 1, 1, 1, 1)",
                    params![
                        storage_bytes(directory_path),
                        storage_bytes(directory_path.parent().unwrap()),
                        storage_bytes(Path::new("/home")),
                    ],
                )
                .unwrap();
        }

        let new_preferences = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/home/private/reinclude")],
            exclusions: vec![PathBuf::from("/home/private")],
        };
        let policy =
            SearchPathPolicy::new(PathBuf::from("/home"), new_preferences.clone()).unwrap();
        let effective = VersionedSearchPathPreferences {
            revision: 1,
            preferences: new_preferences,
        };
        database
            .apply_search_path_transition(
                &effective,
                &policy,
                &[SearchRootMount {
                    root_path: PathBuf::from("/home"),
                    mount_point: PathBuf::from("/"),
                    device: 1,
                }],
                &[],
                &[PathBuf::from("/home/private")],
            )
            .unwrap();

        assert!(database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [storage_bytes(Path::new("/home/keep.txt"))],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert!(!database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [storage_bytes(Path::new("/home/private/drop.txt"))],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert!(database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [storage_bytes(Path::new("/home/private/reinclude/keep.txt"))],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert!(database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [storage_bytes(Path::new("/outside/not-in-diff.txt"))],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        assert!(!database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM directory_snapshots WHERE path = ?1)",
                [storage_bytes(&private)],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
        let stored_owner = database
            .connection
            .query_row(
                "SELECT root_path FROM directory_snapshots WHERE path = ?1",
                [storage_bytes(&reinclude)],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        assert_eq!(path_from_storage(stored_owner), reinclude);
        assert_eq!(database.read_search_root_mounts().unwrap().len(), 1);

        let duplicate_mounts = vec![
            SearchRootMount {
                root_path: PathBuf::from("/new"),
                mount_point: PathBuf::from("/"),
                device: 2,
            },
            SearchRootMount {
                root_path: PathBuf::from("/new"),
                mount_point: PathBuf::from("/"),
                device: 2,
            },
        ];
        assert!(database
            .apply_search_path_transition(
                &VersionedSearchPathPreferences {
                    revision: 2,
                    preferences: SearchPathPreferences::default(),
                },
                &SearchPathPolicy::new(PathBuf::from("/home"), SearchPathPreferences::default())
                    .unwrap(),
                &duplicate_mounts,
                &[],
                &[],
            )
            .is_err());
        assert_eq!(
            database
                .read_search_path_configuration()
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        assert!(database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [storage_bytes(Path::new("/home/keep.txt"))],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }
}
