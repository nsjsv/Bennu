use std::cmp::Ordering;
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use crate::{
    DirectoryEntry, DirectoryMetadataState, DiscoveredDirectoryEntry, FileKind, ScanOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    Size,
    Kind,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub fn apply_entry_options(entries: &mut Vec<DirectoryEntry>, options: &ScanOptions) {
    filter_hidden(entries, options.include_hidden);
    sort_entries(entries, options);
}

pub fn filter_hidden(entries: &mut Vec<DirectoryEntry>, include_hidden: bool) {
    if !include_hidden {
        entries.retain(|entry| !entry.is_hidden);
    }
}

pub fn sort_entries(entries: &mut [DirectoryEntry], options: &ScanOptions) {
    entries.sort_unstable_by(|left, right| compare_entries(left, right, options));
}

pub fn discovered_sort_is_ready(
    entries: &[DiscoveredDirectoryEntry],
    options: &ScanOptions,
) -> bool {
    match options.sort_field {
        SortField::Name | SortField::Kind => true,
        SortField::Size | SortField::Modified => entries
            .iter()
            .all(|entry| !matches!(entry.filesystem_metadata(), DirectoryMetadataState::Pending)),
    }
}

pub fn sort_discovered_entry_indices(
    entries: &[DiscoveredDirectoryEntry],
    options: &ScanOptions,
) -> Vec<usize> {
    let mut indices = (0..entries.len()).collect::<Vec<_>>();
    indices.sort_unstable_by(|left, right| {
        compare_discovered_entries(&entries[*left], &entries[*right], options)
    });
    indices
}

fn compare_discovered_entries(
    left: &DiscoveredDirectoryEntry,
    right: &DiscoveredDirectoryEntry,
    options: &ScanOptions,
) -> Ordering {
    if let Some(ordering) = directory_position(left.kind(), right.kind(), options) {
        return ordering;
    }

    let ordering = match options.sort_field {
        SortField::Kind => kind_rank(left.kind())
            .cmp(&kind_rank(right.kind()))
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Name => compare_names(left.name(), right.name()),
        SortField::Size => {
            return compare_discovered_filesystem_metadata(left, right, options, |metadata| {
                metadata.len
            });
        }
        SortField::Modified => {
            return compare_discovered_filesystem_metadata(left, right, options, |metadata| {
                metadata.modified
            });
        }
    };
    apply_sort_direction(ordering, options.sort_direction)
}

fn compare_discovered_filesystem_metadata<T: Ord>(
    left: &DiscoveredDirectoryEntry,
    right: &DiscoveredDirectoryEntry,
    options: &ScanOptions,
    value: impl Fn(&crate::DirectoryFilesystemMetadata) -> T,
) -> Ordering {
    match (left.filesystem_metadata(), right.filesystem_metadata()) {
        (
            DirectoryMetadataState::Complete(left_metadata),
            DirectoryMetadataState::Complete(right_metadata),
        ) => apply_sort_direction(
            value(left_metadata)
                .cmp(&value(right_metadata))
                .then_with(|| compare_names(left.name(), right.name())),
            options.sort_direction,
        ),
        (DirectoryMetadataState::Complete(_), _) => Ordering::Less,
        (_, DirectoryMetadataState::Complete(_)) => Ordering::Greater,
        (DirectoryMetadataState::Unavailable(_), DirectoryMetadataState::Pending) => Ordering::Less,
        (DirectoryMetadataState::Pending, DirectoryMetadataState::Unavailable(_)) => {
            Ordering::Greater
        }
        _ => apply_sort_direction(
            compare_names(left.name(), right.name()),
            options.sort_direction,
        ),
    }
}

pub fn compare_entries(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    options: &ScanOptions,
) -> Ordering {
    if let Some(ordering) = directory_position(left.kind, right.kind, options) {
        return ordering;
    }

    let ordering = match options.sort_field {
        SortField::Name => compare_names(left.name(), right.name()),
        SortField::Size => left
            .metadata
            .len
            .cmp(&right.metadata.len)
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Kind => kind_rank(left.kind)
            .cmp(&kind_rank(right.kind))
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Modified => left
            .metadata
            .modified
            .cmp(&right.metadata.modified)
            .then_with(|| compare_names(left.name(), right.name())),
    };

    apply_sort_direction(ordering, options.sort_direction)
}

fn directory_position(left: FileKind, right: FileKind, options: &ScanOptions) -> Option<Ordering> {
    if !options.directories_first {
        return None;
    }
    match (left == FileKind::Directory, right == FileKind::Directory) {
        (true, false) => Some(Ordering::Less),
        (false, true) => Some(Ordering::Greater),
        _ => None,
    }
}

fn apply_sort_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn kind_rank(kind: FileKind) -> u8 {
    match kind {
        FileKind::Directory => 0,
        FileKind::File => 1,
        FileKind::Symlink => 2,
        FileKind::Other => 3,
    }
}

#[cfg(unix)]
fn compare_names(left: &OsStr, right: &OsStr) -> Ordering {
    left.as_bytes().cmp(right.as_bytes())
}

#[cfg(not(unix))]
fn compare_names(left: &OsStr, right: &OsStr) -> Ordering {
    left.to_string_lossy().cmp(&right.to_string_lossy())
}
