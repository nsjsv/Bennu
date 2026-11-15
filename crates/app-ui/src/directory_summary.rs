use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use file_core::{is_hidden_name, DirectoryUsage, HiddenEntries, UsageMeasurement, UsageProgress};
use tokio_util::sync::CancellationToken;

const DIRECTORY_CONTENTS_PROGRESS_INTERVAL: usize = 128;
/// 测量时多久报一次进度：数字在跳，但不至于每帧都重画。
const MEASUREMENT_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

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
    /// 读不了、因而没算进来的子目录或条目数；非零时上面的数字偏小。
    pub(crate) unreadable_entries: u64,
}

impl DirectoryContentsSummary {
    pub(crate) fn total_item_count(&self) -> usize {
        self.file_count.saturating_add(self.directory_count)
    }
}

/// 文件夹递归总大小。读不了的子目录跳过不算时 `complete == false`，显示为"≥ 大小"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecursiveTotalSize {
    pub(crate) bytes: u64,
    pub(crate) complete: bool,
}

impl RecursiveTotalSize {
    pub(crate) fn display_text(self) -> String {
        let size = crate::formatting::format_file_size(self.bytes);
        if self.complete {
            size
        } else {
            format!("≥ {size}")
        }
    }
}

#[derive(Debug)]
pub(crate) enum DirectorySummaryError {
    Cancelled,
    Io(std::io::Error),
    Overflow(&'static str),
}

pub(crate) fn read_directory_contents_summary(
    path: &Path,
    cancellation: CancellationToken,
    mut progress: impl FnMut(DirectoryContentsSummary),
) -> Result<DirectoryContentsSummary, DirectorySummaryError> {
    let mut summary = DirectoryContentsSummary::default();
    let mut processed_entries = 0usize;
    let entries = std::fs::read_dir(path).map_err(DirectorySummaryError::Io)?;
    for entry in entries {
        if cancellation.is_cancelled() {
            return Err(DirectorySummaryError::Cancelled);
        }

        let Ok((entry, file_type, metadata)) = entry.and_then(|entry| {
            let file_type = entry.file_type()?;
            let metadata = entry.metadata()?;
            Ok((entry, file_type, metadata))
        }) else {
            summary.unreadable_entries = summary.unreadable_entries.saturating_add(1);
            continue;
        };
        if file_type.is_dir() {
            summary.directory_count = summary
                .directory_count
                .checked_add(1)
                .ok_or(DirectorySummaryError::Overflow("directory count"))?;
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

    progress(summary.clone());
    Ok(summary)
}

#[derive(Debug)]
pub(crate) enum MeasureFailure {
    Cancelled,
    /// 这个文件夹本身读不了。
    Unreadable(PathBuf, String),
}

/// 几个文件夹里面全部层的总用量加起来（多线程测量，几个同时测）；等的时候按节拍
/// 报当前的和，`cancellation` 一取消就停。
pub(crate) async fn measure_directories(
    paths: &[PathBuf],
    cancellation: &CancellationToken,
    mut progress: impl FnMut(DirectoryUsage),
) -> Result<DirectoryUsage, MeasureFailure> {
    let measurements = paths
        .iter()
        .map(|path| UsageMeasurement::start(path.clone(), HiddenEntries::Counted))
        .collect::<Vec<_>>();
    let mut ticks = tokio::time::interval(MEASUREMENT_PROGRESS_INTERVAL);
    ticks.tick().await;
    let all_finished = async {
        for measurement in &measurements {
            measurement.finished().await;
        }
    };
    tokio::pin!(all_finished);
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Err(MeasureFailure::Cancelled),
            () = &mut all_finished => break,
            _ = ticks.tick() => progress(sum_of(&measurements)),
        }
    }
    for (path, measurement) in paths.iter().zip(&measurements) {
        if let UsageProgress::Failed(error) = measurement.snapshot().progress {
            return Err(MeasureFailure::Unreadable(path.clone(), error));
        }
    }
    Ok(sum_of(&measurements))
}

fn sum_of(measurements: &[UsageMeasurement]) -> DirectoryUsage {
    measurements
        .iter()
        .map(|measurement| measurement.snapshot().usage)
        .fold(DirectoryUsage::default(), |total, usage| DirectoryUsage {
            files: total.files + usage.files,
            folders: total.folders + usage.folders,
            bytes: total.bytes.saturating_add(usage.bytes),
            disk_bytes: total.disk_bytes.saturating_add(usage.disk_bytes),
            unreadable: total.unreadable + usage.unreadable,
        })
}

impl From<DirectoryUsage> for RecursiveTotalSize {
    fn from(usage: DirectoryUsage) -> Self {
        Self {
            bytes: usage.bytes,
            complete: usage.unreadable == 0,
        }
    }
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
        let recursive = tokio_test_runtime()
            .block_on(measure_directories(
                &[temp_dir.path().to_path_buf()],
                &CancellationToken::new(),
                |_| {},
            ))
            .expect("recursive usage");

        assert_eq!(direct.file_count, 1);
        assert_eq!(direct.directory_count, 1);
        assert_eq!(recursive.files, 2);
        assert_eq!(recursive.folders, 1);
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
    fn recursive_total_size_counts_what_it_can_and_marks_itself_incomplete() {
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

        let usage = tokio_test_runtime()
            .block_on(measure_directories(
                &[root.to_path_buf()],
                &CancellationToken::new(),
                |_| {},
            ))
            .expect("recursive usage");

        std::fs::set_permissions(&blocked_dir, original_permissions).expect("unlock blocked dir");

        let total = RecursiveTotalSize::from(usage);
        assert_eq!(
            total,
            RecursiveTotalSize {
                bytes: expected,
                complete: false,
            }
        );
        assert!(total.display_text().starts_with("≥ "));
        assert_eq!(usage.unreadable, 1);
    }

    #[test]
    fn a_fully_readable_folder_reports_a_complete_size() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        std::fs::write(temp_dir.path().join("top.txt"), b"top").expect("write top file");

        let total = RecursiveTotalSize::from(
            tokio_test_runtime()
                .block_on(measure_directories(
                    &[temp_dir.path().to_path_buf()],
                    &CancellationToken::new(),
                    |_| {},
                ))
                .expect("recursive usage"),
        );

        assert!(total.complete);
        assert!(!total.display_text().starts_with('≥'));
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

        let outcome = tokio_test_runtime().block_on(measure_directories(
            std::slice::from_ref(&root),
            &CancellationToken::new(),
            |_| {},
        ));

        std::fs::set_permissions(&root, original_permissions).expect("unlock root dir");

        assert!(matches!(outcome, Err(MeasureFailure::Unreadable(path, _)) if path == root));
    }

    #[test]
    fn several_folders_are_measured_together_and_summed() {
        let first = tempfile::tempdir().expect("create temp dir");
        let second = tempfile::tempdir().expect("create temp dir");
        std::fs::write(first.path().join("a"), b"12").expect("write file");
        std::fs::write(second.path().join("b"), b"345").expect("write file");

        let usage = tokio_test_runtime()
            .block_on(measure_directories(
                &[first.path().to_path_buf(), second.path().to_path_buf()],
                &CancellationToken::new(),
                |_| {},
            ))
            .expect("recursive usage");

        assert_eq!((usage.files, usage.bytes), (2, 5));
    }

    fn tokio_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime")
    }
}
