use std::cmp::Ordering;
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use crate::{
    DirectoryEntry, DirectoryMetadataState, DiscoveredDirectoryEntry, FileKind, ScanOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortField {
    Name,
    Size,
    Kind,
    Modified,
    /// 状态改变时间（ctime）。
    Changed,
}

impl SortField {
    /// 按它排要先读每个条目的文件信息（stat）；名字和类型在列目录时就有。
    pub fn needs_filesystem_metadata(self) -> bool {
        match self {
            Self::Name | Self::Kind => false,
            Self::Size | Self::Modified | Self::Changed => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        entries.retain(|entry| !entry.is_hidden());
    }
}

pub fn sort_entries(entries: &mut [DirectoryEntry], options: &ScanOptions) {
    entries.sort_unstable_by(|left, right| compare_entries(left, right, options));
}

pub fn discovered_sort_is_ready(
    entries: &[DiscoveredDirectoryEntry],
    options: &ScanOptions,
) -> bool {
    !options.sort_field.needs_filesystem_metadata()
        || entries
            .iter()
            .all(|entry| !matches!(entry.filesystem_metadata(), DirectoryMetadataState::Pending))
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
        SortField::Changed => {
            return compare_discovered_filesystem_metadata(left, right, options, |metadata| {
                metadata.changed
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
    if let Some(ordering) = directory_position(left.kind(), right.kind(), options) {
        return ordering;
    }

    let ordering = match options.sort_field {
        SortField::Name => compare_names(left.name(), right.name()),
        SortField::Size => left
            .size()
            .cmp(&right.size())
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Kind => kind_rank(left.kind())
            .cmp(&kind_rank(right.kind()))
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Modified => left
            .modified()
            .cmp(&right.modified())
            .then_with(|| compare_names(left.name(), right.name())),
        SortField::Changed => left
            .changed()
            .cmp(&right.changed())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::EntryMetadata;

    fn file_changed_at(name: &str, changed_seconds: u64) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from("/folder").join(name),
            FileKind::File,
            EntryMetadata {
                // 修改时间和状态改变时间反着来：排序必须看对字段。
                modified: Some(
                    SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - changed_seconds),
                ),
                changed: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(changed_seconds)),
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    #[test]
    fn sorting_by_status_change_time_uses_ctime_not_the_modified_time() {
        let mut entries = vec![
            file_changed_at("b", 30),
            file_changed_at("a", 10),
            file_changed_at("c", 20),
        ];
        let options = ScanOptions {
            sort_field: SortField::Changed,
            ..ScanOptions::default()
        };

        sort_entries(&mut entries, &options);

        let names: Vec<_> = entries
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect();
        assert_eq!(names, ["a", "c", "b"]);
    }

    #[test]
    fn only_name_and_kind_sort_without_reading_file_information() {
        assert!(!SortField::Name.needs_filesystem_metadata());
        assert!(!SortField::Kind.needs_filesystem_metadata());
        for field in [SortField::Size, SortField::Modified, SortField::Changed] {
            assert!(field.needs_filesystem_metadata(), "{field:?}");
        }
    }

    /// 改修改时间本身就是一次状态改变：ctime 落在「现在」，和改成 2001 年的修改时间不是一回事。
    #[test]
    fn status_change_time_follows_metadata_changes_not_the_modified_time() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("file");
        std::fs::write(&path, b"x").expect("write file");
        let long_ago = filetime::FileTime::from_unix_time(978_307_200, 0);
        filetime::set_file_mtime(&path, long_ago).expect("set modified time");
        let before = SystemTime::now() - Duration::from_secs(60);

        let metadata = std::fs::metadata(&path).expect("metadata");
        let changed = crate::status_changed(&metadata).expect("ctime");

        assert_eq!(
            metadata.modified().expect("modified"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(978_307_200)
        );
        assert!(changed > before, "{changed:?}");
    }
}
