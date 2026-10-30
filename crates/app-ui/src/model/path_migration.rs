use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_core::{DirectoryDiscovery, DirectoryEntry};

use super::{DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryStatus};
use file_operations::{path_after_completed_migrations, CompletedPathMigration};

pub(super) fn migrate_directory_entries(
    entries: &mut [DirectoryEntry],
    migrations: &[CompletedPathMigration],
) {
    for entry in entries {
        migrate_directory_entry(entry, migrations);
    }
}

pub(super) fn migrate_directory_entry(
    entry: &mut DirectoryEntry,
    migrations: &[CompletedPathMigration],
) {
    let migrated_path = path_after_completed_migrations(&entry.path, migrations);
    if migrated_path == entry.path {
        return;
    }
    entry.name = migrated_path
        .file_name()
        .unwrap_or_else(|| migrated_path.as_os_str())
        .to_os_string();
    entry.path = migrated_path;
}

pub(super) fn migrate_optional_path(
    path: &mut Option<PathBuf>,
    migrations: &[CompletedPathMigration],
) {
    if let Some(path) = path {
        *path = path_after_completed_migrations(path, migrations);
    }
}

pub(super) fn retain_optional_path_outside_subtree(path: &mut Option<PathBuf>, subtree: &Path) {
    if path
        .as_ref()
        .is_some_and(|candidate| candidate.starts_with(subtree))
    {
        *path = None;
    }
}

pub(super) fn migrate_path_list(paths: &mut [PathBuf], migrations: &[CompletedPathMigration]) {
    for path in paths {
        *path = path_after_completed_migrations(path, migrations);
    }
}

pub(super) fn migrate_path_set(
    paths: &mut HashSet<PathBuf>,
    migrations: &[CompletedPathMigration],
) {
    *paths = paths
        .drain()
        .map(|path| path_after_completed_migrations(&path, migrations))
        .collect();
}

pub(super) fn migrate_path_map_keys<Value>(
    paths: &mut HashMap<PathBuf, Value>,
    migrations: &[CompletedPathMigration],
) {
    *paths = paths
        .drain()
        .map(|(path, value)| (path_after_completed_migrations(&path, migrations), value))
        .collect();
}

pub(super) fn detach_directory_discovery(
    entries: &mut [DirectoryEntry],
    discovery: &mut Option<DirectoryDiscovery>,
    order_phase: &mut DirectoryOrderPhase,
) {
    if let Some(discovery) = discovery.take() {
        for entry in entries {
            let Some(index) = entry.discovery_index.take() else {
                continue;
            };
            let Some(discovered) = discovery.entries.get(index) else {
                continue;
            };
            let materialized = discovered.display_entry();
            entry.metadata = materialized.metadata;
            entry.is_broken_symlink = materialized.is_broken_symlink;
        }
    }
    if let DirectoryOrderPhase::WaitingForMetadata {
        field, direction, ..
    } = *order_phase
    {
        *order_phase = DirectoryOrderPhase::Ready { field, direction };
    }
}

pub(super) fn migrate_expanded_directories(
    directories: &mut HashMap<PathBuf, ExpandedDirectory>,
    migrations: &[CompletedPathMigration],
) {
    *directories = directories
        .drain()
        .filter_map(|(path, mut directory)| {
            if let Some(cancellation) = directory.load_cancel.take() {
                cancellation.cancel();
            }
            if matches!(directory.status, ExpandedDirectoryStatus::Loading) {
                return None;
            }
            detach_directory_discovery(
                &mut directory.entries,
                &mut directory.directory_discovery,
                &mut directory.directory_order_phase,
            );
            migrate_directory_entries(&mut directory.entries, migrations);
            Some((
                path_after_completed_migrations(&path, migrations),
                directory,
            ))
        })
        .collect();
}

pub(super) fn cancel_and_clear_expanded_directories(
    directories: &mut HashMap<PathBuf, ExpandedDirectory>,
) {
    for directory in directories.values_mut() {
        if let Some(cancellation) = directory.load_cancel.take() {
            cancellation.cancel();
        }
    }
    directories.clear();
}
