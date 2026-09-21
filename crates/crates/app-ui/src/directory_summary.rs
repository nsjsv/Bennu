use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use file_core::is_hidden_name;
use tokio_util::sync::CancellationToken;

const DIRECTORY_CONTENTS_PROGRESS_INTERVAL: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DirectoryContentsSummary {
    pub(crate) file_count: usize,
    pub(crate) directory_count: usize,
    pub(crate) total_size_bytes: u64,
    pub(crate) total_disk_size_bytes: u64,
    // 直属文件大小记账：缓存只存文件系统原样事实，隐藏部分单独记录，
    // 可见口径（扣除隐藏）由 UI 按 show_hidden_files 推导。
    pub(crate) files_total_size_bytes: u64,
    pub(crate) hidden_files_total_size_bytes: u64,
    // 隐藏条目数事实（隐藏文件 + 隐藏文件夹），与隐藏大小同源产出，
    // 供底栏幽灵开关显示；缓存同样不掺配置。
    pub(crate) hidden_entry_count: usize,
}

impl DirectoryContentsSummary {
    pub(crate) fn total_item_count(&self) -> usize {
        self.file_count.saturating_add(self.directory_count)
    }
}

#[derive(Debug)]
pub(crate) enum DirectorySummaryError {
    Cancelled,
    Io(std::io::Error),
    Overflow(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectorySummaryScope {
    DirectChildren,
    Descendants,
}

pub(crate) fn read_directory_contents_summary(
    path: &Path,
    cancellation: CancellationToken,
    progress: impl FnMut(DirectoryContentsSummary),
) -> Result<DirectoryContentsSummary, DirectorySummaryError> {
    read_directory_summary(
        path,
        cancellation,
        DirectorySummaryScope::DirectChildren,
        progress,
    )
}

pub(crate) fn read_directory_tree_summary(
    path: &Path,
    cancellation: CancellationToken,
    progress: impl FnMut(DirectoryContentsSummary),
) -> Result<DirectoryContentsSummary, DirectorySummaryError> {
    read_directory_summary(
        path,
        cancellation,
        DirectorySummaryScope::Descendants,
        progress,
    )
}

fn read_directory_summary(
    path: &Path,
    cancellation: CancellationToken,
    scope: DirectorySummaryScope,
    mut progress: impl FnMut(DirectoryContentsSummary),
) -> Result<DirectoryContentsSummary, DirectorySummaryError> {
    let mut summary = DirectoryContentsSummary::default();
    let mut pending = vec![PathBuf::from(path)];
    let mut processed_entries = 0usize;

    while let Some(directory) = pending.pop() {
        if cancellation.is_cancelled() {
            return Err(DirectorySummaryError::Cancelled);
        }

        for entry in std::fs::read_dir(directory).map_err(DirectorySummaryError::Io)? {
            if cancellation.is_cancelled() {
                return Err(DirectorySummaryError::Cancelled);
            }

            let entry = entry.map_err(DirectorySummaryError::Io)?;
            let file_type = entry.file_type().map_err(DirectorySummaryError::Io)?;
            let metadata = entry.metadata().map_err(DirectorySummaryError::Io)?;
            if file_type.is_dir() {
                summary.directory_count = summary
                    .directory_count
                    .checked_add(1)
                    .ok_or(DirectorySummaryError::Overflow("directory count"))?;
                if scope == DirectorySummaryScope::Descendants {
                    pending.push(entry.path());
                }
            } else {
                summary.file_count = summary
                    .file_count
                    .checked_add(1)
                    .ok_or(DirectorySummaryError::Overflow("file count"))?;
                summary.files_total_size_bytes = summary
                    .files_total_size_bytes
                    .checked_add(metadata.len())
                    .ok_or(DirectorySummaryError::Overflow("file size"))?;
                if is_hidden_name(&entry.file_name()) {
                    summary.hidden_files_total_size_bytes = summary
                        .hidden_files_total_size_bytes
                        .checked_add(metadata.len())
                        .ok_or(DirectorySummaryError::Overflow("hidden file size"))?;
                }
            }
            // 幽灵开关的数量口径：隐藏文件夹与隐藏文件都算一个条目。
            if is_hidden_name(&entry.file_name()) {
                summary.hidden_entry_count = summary
                    .hidden_entry_count
                    .checked_add(1)
                    .ok_or(DirectorySummaryError::Overflow("hidden entry count"))?;
            }
            summary.total_size_bytes = summary
                .total_size_bytes
                .checked_add(metadata.len())
                .ok_or(DirectorySummaryError::Overflow("logical size"))?;
            summary.total_disk_size_bytes = summary
                .total_disk_size_bytes
                .checked_add(metadata_disk_size(&metadata))
                .ok_or(DirectorySummaryError::Overflow("disk size"))?;
            processed_entries = processed_entries
                .checked_add(1)
                .ok_or(DirectorySummaryError::Overflow("processed entry count"))?;
            if processed_entries % DIRECTORY_CONTENTS_PROGRESS_INTERVAL == 0 {
                progress(summary.clone());
            }
        }
    }

    progress(summary.clone());
    Ok(summary)
}

pub(crate) fn read_directory_recursive_total_size(
    path: &Path,
    cancellation: CancellationToken,
) -> Result<u64, DirectorySummaryError> {
    let mut pending = vec![PathBuf::from(path)];
    let mut total_size_bytes = 0u64;

    while let Some(directory) = pending.pop() {
        if cancellation.is_cancelled() {
            return Err(DirectorySummaryError::Cancelled);
        }

        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if directory.as_path() == path => {
                return Err(DirectorySummaryError::Io(error));
            }
            Err(_) => continue,
        };

        for entry in entries {
            if cancellation.is_cancelled() {
                return Err(DirectorySummaryError::Cancelled);
            }

            let Ok(entry) = entry else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            total_size_bytes = total_size_bytes.saturating_add(metadata.len());
            if file_type.is_dir() {
                pending.push(entry.path());
            }
        }
    }

    Ok(total_size_bytes)
}

#[cfg(unix)]
pub(crate) fn metadata_disk_size(metadata: &std::fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
pub(crate) fn metadata_disk_size(metadata: &std::fs::Metadata) -> u64 {
    metadata.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_recursive_summaries_keep_distinct_scopes() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let child = temp_dir.path().join("child");
        std::fs::create_dir(&child).expect("create child");
        std::fs::write(temp_dir.path().join("top.txt"), b"top").expect("write top file");
        std::fs::write(child.join("nested.txt"), b"nested").expect("write nested file");

        let direct =
            read_directory_contents_summary(temp_dir.path(), CancellationToken::new(), |_| {})
                .expect("direct summary");
        let recursive =
            read_directory_tree_summary(temp_dir.path(), CancellationToken::new(), |_| {})
                .expect("recursive summary");

        assert_eq!(direct.file_count, 1);
        assert_eq!(direct.directory_count, 1);
        assert_eq!(recursive.file_count, 2);
        assert_eq!(recursive.directory_count, 1);
    }

    #[test]
    fn file_size_accounting_records_hidden_share_separately() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let child = temp_dir.path().join("child");
        std::fs::create_dir(&child).expect("create child");
        std::fs::write(temp_dir.path().join("top.txt"), b"top").expect("write top file");
        std::fs::write(temp_dir.path().join(".hidden"), b"hid").expect("write hidden file");
        std::fs::write(child.join("nested.txt"), b"nested").expect("write nested file");

        let direct =
            read_directory_contents_summary(temp_dir.path(), CancellationToken::new(), |_| {})
                .expect("direct summary");

        // 只记直属文件，不进子文件夹；目录自身的 inode 大小也不计入文件大小。
        let top_len = std::fs::metadata(temp_dir.path().join("top.txt"))
            .expect("top metadata")
            .len();
        let hidden_len = std::fs::metadata(temp_dir.path().join(".hidden"))
            .expect("hidden metadata")
            .len();
        assert_eq!(direct.files_total_size_bytes, top_len + hidden_len);
        assert_eq!(direct.hidden_files_total_size_bytes, hidden_len);
    }

    #[test]
    fn hidden_entry_count_covers_hidden_files_and_directories() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(temp_dir.path().join(".hidden-dir")).expect("create hidden dir");
        std::fs::write(temp_dir.path().join(".hidden-file"), b"hid").expect("write hidden file");
        std::fs::write(temp_dir.path().join("visible.txt"), b"top").expect("write visible file");
        // 隐藏目录的内部条目不属于直属口径，不得计入。
        std::fs::write(
            temp_dir.path().join(".hidden-dir").join("nested.txt"),
            b"nested",
        )
        .expect("write nested file");

        let direct =
            read_directory_contents_summary(temp_dir.path(), CancellationToken::new(), |_| {})
                .expect("direct summary");

        assert_eq!(direct.hidden_entry_count, 2);
    }

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    #[test]
    fn recursive_total_size_skips_unreadable_descendants() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let root = temp_dir.path();
        let readable_dir = root.join("readable");
        let blocked_dir = root.join("blocked");
        let top_file = root.join("top.txt");
        let nested_file = readable_dir.join("nested.txt");
        let blocked_file = blocked_dir.join("secret.txt");

        std::fs::create_dir(&readable_dir).expect("create readable dir");
        std::fs::create_dir(&blocked_dir).expect("create blocked dir");
        std::fs::write(&top_file, b"top-level").expect("write top file");
        std::fs::write(&nested_file, b"nested").expect("write nested file");
        std::fs::write(&blocked_file, b"secret").expect("write blocked file");

        let expected = std::fs::metadata(&readable_dir)
            .expect("readable dir metadata")
            .len()
            + std::fs::metadata(&blocked_dir)
                .expect("blocked dir metadata")
                .len()
            + std::fs::metadata(&top_file)
                .expect("top file metadata")
                .len()
            + std::fs::metadata(&nested_file)
                .expect("nested file metadata")
                .len();

        let original_permissions = std::fs::metadata(&blocked_dir)
            .expect("blocked dir metadata")
            .permissions();
        let mut blocked_permissions = original_permissions.clone();
        blocked_permissions.set_mode(0o000);
        std::fs::set_permissions(&blocked_dir, blocked_permissions).expect("lock blocked dir");
        if std::fs::read_dir(&blocked_dir).is_ok() {
            std::fs::set_permissions(&blocked_dir, original_permissions)
                .expect("unlock blocked dir");
            return;
        }

        let total = read_directory_recursive_total_size(root, CancellationToken::new())
            .expect("recursive total size");

        std::fs::set_permissions(&blocked_dir, original_permissions).expect("unlock blocked dir");

        assert_eq!(total, expected);
    }

    #[cfg(unix)]
    #[test]
    fn recursive_total_size_errors_when_root_directory_is_unreadable() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let root = temp_dir.path().join("root");
        std::fs::create_dir(&root).expect("create root dir");
        std::fs::write(root.join("file.txt"), b"payload").expect("write file");

        let original_permissions = std::fs::metadata(&root)
            .expect("root metadata")
            .permissions();
        let mut blocked_permissions = original_permissions.clone();
        blocked_permissions.set_mode(0o000);
        std::fs::set_permissions(&root, blocked_permissions).expect("lock root dir");
        if std::fs::read_dir(&root).is_ok() {
            std::fs::set_permissions(&root, original_permissions).expect("unlock root dir");
            return;
        }

        let outcome = read_directory_recursive_total_size(&root, CancellationToken::new());

        std::fs::set_permissions(&root, original_permissions).expect("unlock root dir");

        assert!(matches!(outcome, Err(DirectorySummaryError::Io(_))));
    }
}
