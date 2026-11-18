use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use file_core::{DirectoryUsage, HiddenEntries, UsageMeasurement, UsageProgress};
use tokio_util::sync::CancellationToken;

/// 测量时多久报一次进度：数字在跳，但不至于每帧都重画。
const MEASUREMENT_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

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

/// 文件夹里直属的条目数（隐藏的也算）：列表里文件夹那一行只显示「几项」，只读目录、不 stat。
pub(crate) fn count_directory_children(path: &Path) -> std::io::Result<usize> {
    Ok(std::fs::read_dir(path)?.filter(Result::is_ok).count())
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

        let direct = count_directory_children(temp_dir.path()).expect("direct count");
        let recursive = tokio_test_runtime()
            .block_on(measure_directories(
                &[temp_dir.path().to_path_buf()],
                &CancellationToken::new(),
                |_| {},
            ))
            .expect("recursive usage");

        assert_eq!(direct, 2);
        assert_eq!(recursive.files, 2);
        assert_eq!(recursive.folders, 1);
    }

    /// 隐藏的也算：「几项」与文件夹里实际有的一致；子文件夹里面的不算。
    #[test]
    fn direct_count_includes_hidden_entries_but_not_nested_ones() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(temp_dir.path().join(".hidden-dir")).expect("create hidden dir");
        std::fs::write(temp_dir.path().join(".hidden-file"), b"hid").expect("write hidden file");
        std::fs::write(temp_dir.path().join("visible.txt"), b"top").expect("write visible file");
        std::fs::write(
            temp_dir.path().join(".hidden-dir").join("nested.txt"),
            b"nested",
        )
        .expect("write nested file");

        assert_eq!(count_directory_children(temp_dir.path()).expect("count"), 3);
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
