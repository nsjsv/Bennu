use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::{
    ArchiveProgress, CopyProgress, FileObjectKind, SourceManifest, TransferJournalRecord,
};

use crate::{
    CurrentFileProgress, FileOperationEvent, FileOperationEventSender, FileOperationProgressUpdate,
    TransferEntrySnapshot, TransferEntrySnapshotState,
};

use super::super::QueuedTransferMode;

pub(super) struct TransferBatchProgress {
    records: Vec<TransferRecordProgress>,
    total_items: usize,
    /// `None` when some record has no manifest or the batch has no bytes: the
    /// bar then counts items only.
    total_bytes: Option<u64>,
    /// `CopyProgress` only names the source file; walking up its path to a
    /// record's source root says which record it belongs to.
    record_by_source: HashMap<PathBuf, usize>,
}

struct TransferRecordProgress {
    /// 这条记录算几项：清单里非目录条目的个数，至少 1（空文件夹、建不出
    /// 清单的记录各算 1 项）。目录没有字节事件，计入的话会在记录结算时整段跳一下。
    items: usize,
    state: TransferRecordState,
    /// 已搬完的文件数，结算前显示时不超过 `items`。
    copied_files: usize,
    /// 已开始、未完成的文件：源路径 → (已搬字节, 总字节)。目录并行 staging
    /// 和一段里的多条记录都会让它同时有好几个。
    in_flight: HashMap<PathBuf, (u64, u64)>,
    /// 清单里每个普通文件的大小；字节进度只认和清单一致的事件。
    regular_files: HashMap<PathBuf, u64>,
    copied_bytes: u64,
    file_bytes: HashMap<PathBuf, u64>,
    target: PathBuf,
    is_directory: bool,
    /// 清单里普通文件的总字节；没有清单时未知。
    display_bytes: Option<u64>,
    /// 是否进入占位流。复制全量 true（入队即占位）；移动只在真正发生字节
    /// 复制时置位，同盘 rename 永不产生占位。
    copy_observed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferRecordState {
    Queued,
    Active,
    Settled,
}

impl TransferBatchProgress {
    pub(super) fn new(records: &[TransferJournalRecord], mode: QueuedTransferMode) -> Self {
        let record_by_source = records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.request.source.clone(), index))
            .collect();
        let records = records
            .iter()
            .map(|record| record_progress(record, mode))
            .collect::<Vec<_>>();
        let total_bytes = records
            .iter()
            .map(|record| record.display_bytes)
            .try_fold(0_u64, |total, bytes| total.checked_add(bytes?))
            .filter(|total| *total > 0);
        Self {
            total_items: records.iter().map(|record| record.items).sum(),
            total_bytes,
            record_by_source,
            records,
        }
    }

    /// The records of a segment that just started running.
    pub(super) fn start_records(&mut self, indices: &[usize]) {
        for &index in indices {
            if self.records[index].state == TransferRecordState::Queued {
                self.records[index].state = TransferRecordState::Active;
            }
        }
    }

    /// 每一条 `CopyProgress` 都要经过这里（只有发往界面的消息才节流），
    /// 否则完成计数会随被丢掉的事件一起丢失。
    pub(super) fn observe_copy_progress(&mut self, progress: &CopyProgress) {
        let Some(index) = progress
            .from
            .ancestors()
            .find_map(|path| self.record_by_source.get(path).copied())
        else {
            return;
        };
        let record = &mut self.records[index];
        if record.state == TransferRecordState::Settled {
            return;
        }
        record.copy_observed = true;
        record.observe_file_lifecycle(progress);
        record.observe_copied_bytes(progress);
    }

    pub(super) fn complete_record(&mut self, index: usize) {
        let record = &mut self.records[index];
        record.state = TransferRecordState::Settled;
        record.in_flight.clear();
        record.file_bytes.clear();
    }

    pub(super) fn snapshot(&self) -> FileOperationProgressUpdate {
        let mut completed_items = 0;
        let mut completed_bytes = 0_u64;
        for record in &self.records {
            match record.state {
                TransferRecordState::Settled => {
                    // 符号链接这类没有字节事件的条目在这里补齐。
                    completed_items += record.items;
                    completed_bytes += record.display_bytes.unwrap_or(0);
                }
                // 清单缺失的记录只算 1 项，里面搬完的文件不能越过它的份额。
                _ => {
                    completed_items += record.copied_files.min(record.items);
                    completed_bytes += record.copied_bytes;
                }
            }
        }
        FileOperationProgressUpdate::Transfer {
            completed_items,
            total_items: self.total_items,
            bytes: self
                .total_bytes
                .map(|total_bytes| (completed_bytes.min(total_bytes), total_bytes)),
            current_file: self.current_file(),
        }
    }

    /// 并行搬运时取最大的那个：小文件一闪就完，条不会在它们之间乱跳。
    /// 同样大小按路径定序，避免 HashMap 遍历顺序让显示来回换。
    fn current_file(&self) -> Option<CurrentFileProgress> {
        self.records
            .iter()
            .flat_map(|record| record.in_flight.iter())
            .max_by(
                |(left_path, (_, left_total)), (right_path, (_, right_total))| {
                    left_total
                        .cmp(right_total)
                        .then_with(|| right_path.cmp(left_path))
                },
            )
            .map(
                |(path, &(completed_bytes, total_bytes))| CurrentFileProgress {
                    name: path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    completed_bytes,
                    total_bytes,
                },
            )
    }

    /// 全量占位快照，随进度消息整批替换 UI 侧 `transfer_progress`。移动任务
    /// 没开始复制的条目（同盘 rename）不进入快照，从源头保证不产生占位。
    pub(super) fn transfer_entry_snapshots(&self) -> Vec<TransferEntrySnapshot> {
        self.records
            .iter()
            .filter(|record| record.copy_observed)
            .map(|record| {
                let (state, completed_bytes) = match record.state {
                    TransferRecordState::Settled => {
                        (TransferEntrySnapshotState::Completed, record.display_bytes)
                    }
                    TransferRecordState::Active => (
                        TransferEntrySnapshotState::Active,
                        Some(record.copied_bytes),
                    ),
                    TransferRecordState::Queued => (TransferEntrySnapshotState::Queued, None),
                };
                TransferEntrySnapshot {
                    target: record.target.clone(),
                    is_directory: record.is_directory,
                    completed_bytes,
                    total_bytes: record.display_bytes,
                    state,
                }
            })
            .collect()
    }
}

impl TransferRecordProgress {
    /// 搬运方对每个文件先发 `0/total` 开始事件，第一条 `done == total` 即完成
    /// （空文件的 `0/0` 两者合一）。
    fn observe_file_lifecycle(&mut self, progress: &CopyProgress) {
        let finished = progress.bytes_done >= progress.bytes_total;
        if let Some((completed_bytes, _)) = self.in_flight.get_mut(&progress.from) {
            *completed_bytes = (*completed_bytes).max(progress.bytes_done);
            if !finished {
                return;
            }
            self.in_flight.remove(&progress.from);
        } else if progress.bytes_done != 0 {
            // 已完成文件的迟到事件。
            return;
        } else if !finished {
            self.in_flight
                .insert(progress.from.clone(), (0, progress.bytes_total));
            return;
        }
        self.copied_files += 1;
    }

    fn observe_copied_bytes(&mut self, progress: &CopyProgress) {
        let Some(&expected_bytes) = self.regular_files.get(progress.from.as_path()) else {
            return;
        };
        if progress.bytes_total != expected_bytes {
            return;
        }
        let completed_file_bytes = progress.bytes_done.min(expected_bytes);
        let previous_file_bytes = self
            .file_bytes
            .get(progress.from.as_path())
            .copied()
            .unwrap_or(0);
        if completed_file_bytes <= previous_file_bytes {
            return;
        }
        self.file_bytes
            .insert(progress.from.clone(), completed_file_bytes);
        self.copied_bytes += completed_file_bytes - previous_file_bytes;
    }
}

/// 一条记录的工作量：算几个文件（清单里非目录条目，至少 1）、多少字节
/// （普通文件之和）。进度的总数和分段的上限都按它算。
pub(super) fn record_workload(record: &TransferJournalRecord) -> (usize, u64) {
    let Some(manifest) = record.manifest.as_ref() else {
        return (1, 0);
    };
    let (mut files, mut bytes) = (0_usize, 0_u64);
    for entry in &manifest.entries {
        match entry.identity.object_kind {
            FileObjectKind::Directory => {}
            FileObjectKind::RegularFile => {
                files += 1;
                bytes = bytes.saturating_add(entry.identity.size);
            }
            FileObjectKind::SymbolicLink => files += 1,
        }
    }
    (files.max(1), bytes)
}

fn record_progress(
    record: &TransferJournalRecord,
    mode: QueuedTransferMode,
) -> TransferRecordProgress {
    let manifest = record.manifest.as_ref();
    let (items, bytes) = record_workload(record);
    let regular_files = manifest
        .map(|manifest| {
            manifest
                .entries
                .iter()
                .filter(|entry| entry.identity.object_kind == FileObjectKind::RegularFile)
                .map(|entry| {
                    (
                        manifest_entry_path(manifest, &entry.relative_path),
                        entry.identity.size,
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let display_bytes = manifest.map(|_| bytes);
    TransferRecordProgress {
        items,
        state: TransferRecordState::Queued,
        copied_files: 0,
        in_flight: HashMap::new(),
        regular_files,
        copied_bytes: 0,
        file_bytes: HashMap::new(),
        target: record.request.requested_target.clone(),
        // 清单首条即源根自身:目录源的根条目 kind 为 Directory。
        is_directory: manifest
            .and_then(|manifest| manifest.entries.first())
            .is_some_and(|root| root.identity.object_kind == FileObjectKind::Directory),
        display_bytes,
        copy_observed: matches!(mode, QueuedTransferMode::Copy),
    }
}

pub(crate) async fn send_archive_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    progress: ArchiveProgress,
) {
    send_file_operation_progress(output, task_id, archive_progress_update(progress)).await;
}

pub(crate) async fn send_file_operation_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    progress: FileOperationProgressUpdate,
) {
    let _ = output
        .send(FileOperationEvent::Progressed {
            task_id,
            update: progress,
            entries: Vec::new(),
        })
        .await;
}

/// 传输批次的进度消息:聚合更新与逐条目占位快照同行发送,快照全量
/// 替换 UI 侧任务状态,占位行由此保持单一事实源。
pub(super) async fn send_transfer_batch_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    batch_progress: &TransferBatchProgress,
    progress: FileOperationProgressUpdate,
) {
    let entries = batch_progress.transfer_entry_snapshots();
    let _ = output
        .send(FileOperationEvent::Progressed {
            task_id,
            update: progress,
            entries,
        })
        .await;
}

/// 度量由产出方选定：按条目算的发 `Transfer`（可带当前文件），按字节算的
/// 发 `Bytes`。分母为 0 时进度模型自己不出比例，这里不必再分支。
fn archive_progress_update(progress: ArchiveProgress) -> FileOperationProgressUpdate {
    match progress {
        ArchiveProgress::Entries {
            completed_entries,
            total_entries,
            bytes,
            current_entry,
        } => FileOperationProgressUpdate::Transfer {
            completed_items: completed_entries,
            total_items: total_entries,
            bytes,
            current_file: current_entry.map(|entry| CurrentFileProgress {
                name: entry.name,
                completed_bytes: entry.completed_bytes,
                total_bytes: entry.total_bytes,
            }),
        },
        ArchiveProgress::Bytes {
            completed_bytes,
            total_bytes,
            completed_entries,
            total_entries,
        } => FileOperationProgressUpdate::Bytes {
            completed_bytes,
            total_bytes,
            completed_items: completed_entries,
            total_items: total_entries,
        },
    }
}

fn manifest_entry_path(manifest: &SourceManifest, relative_path: &Path) -> PathBuf {
    if relative_path.as_os_str().is_empty() {
        manifest.root.clone()
    } else {
        manifest.root.join(relative_path)
    }
}

#[cfg(test)]
mod tests {
    use file_core::{
        FileIdentity, FileOperationVerification, RecoverableTransferOperation,
        RecoverableTransferRequest, SourceManifestEntry, TransferCheckpoint,
        TransferConflictStrategy, TransferWorkKey,
    };

    use super::*;

    fn identity(size: u64) -> FileIdentity {
        FileIdentity {
            device: 1,
            inode: size + 1,
            object_kind: FileObjectKind::RegularFile,
            size,
            modified_seconds: 2,
            modified_nanoseconds: 3,
            changed_seconds: 4,
            changed_nanoseconds: 5,
            symbolic_link_target: None,
        }
    }

    fn record(index: u64, root: &str, entries: &[(&str, u64)]) -> TransferJournalRecord {
        let source = PathBuf::from(root);
        TransferJournalRecord {
            task_id: 1,
            key: TransferWorkKey::top_level(index),
            request: RecoverableTransferRequest {
                source: source.clone(),
                requested_target: PathBuf::from(format!("{root}-target")),
                operation: RecoverableTransferOperation::Copy,
                conflict_strategy: TransferConflictStrategy::Fail,
                verification: FileOperationVerification::BasicMetadata,
            },
            checkpoint: TransferCheckpoint::AwaitingManifest,
            revision: 1,
            manifest: Some(SourceManifest {
                root: source,
                entries: entries
                    .iter()
                    .map(|(path, size)| SourceManifestEntry {
                        relative_path: PathBuf::from(path),
                        identity: identity(*size),
                    })
                    .collect(),
            }),
            replacement_manifest: None,
        }
    }

    fn settle(progress: &mut TransferBatchProgress, index: usize) -> FileOperationProgressUpdate {
        progress.complete_record(index);
        progress.snapshot()
    }

    fn copy_progress(path: &str, bytes_done: u64, bytes_total: u64) -> CopyProgress {
        CopyProgress {
            from: PathBuf::from(path),
            to: PathBuf::from("/target"),
            bytes_done,
            bytes_total,
        }
    }

    fn bytes(update: FileOperationProgressUpdate) -> (u64, u64, usize, usize) {
        let FileOperationProgressUpdate::Transfer {
            completed_items,
            total_items,
            bytes: Some((completed_bytes, total_bytes)),
            ..
        } = update
        else {
            panic!("expected transfer progress with bytes");
        };
        (completed_bytes, total_bytes, completed_items, total_items)
    }

    fn items(update: FileOperationProgressUpdate) -> (usize, usize) {
        let FileOperationProgressUpdate::Transfer {
            completed_items,
            total_items,
            ..
        } = update
        else {
            panic!("expected transfer progress");
        };
        (completed_items, total_items)
    }

    fn current_file(update: FileOperationProgressUpdate) -> Option<CurrentFileProgress> {
        let FileOperationProgressUpdate::Transfer { current_file, .. } = update else {
            panic!("expected transfer progress");
        };
        current_file
    }

    #[test]
    fn small_first_file_does_not_make_directory_look_complete() {
        let records = vec![record(0, "/source", &[("small", 10), ("large", 990)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(&copy_progress("/source/small", 0, 10));
        progress.observe_copy_progress(&copy_progress("/source/small", 10, 10));

        assert_eq!(bytes(progress.snapshot()), (10, 1_000, 1, 2));
    }

    #[test]
    fn top_level_records_share_one_byte_denominator() {
        let records = vec![
            record(0, "/small", &[("", 10)]),
            record(1, "/large", &[("", 990)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(settle(&mut progress, 0)), (10, 1_000, 1, 2));
        progress.observe_copy_progress(&copy_progress("/large", 495, 990));
        assert_eq!(bytes(progress.snapshot()), (505, 1_000, 1, 2));
    }

    #[test]
    fn leaf_progress_is_monotonic_and_must_match_manifest_size() {
        let records = vec![record(0, "/source", &[("file", 100)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(&copy_progress("/source/file", 60, 100));
        assert_eq!(bytes(progress.snapshot()), (60, 100, 0, 1));
        progress.observe_copy_progress(&copy_progress("/source/file", 20, 100));
        progress.observe_copy_progress(&copy_progress("/source/file", 90, 200));
        assert_eq!(bytes(progress.snapshot()), (60, 100, 0, 1));
    }

    #[test]
    fn completed_record_fills_logical_bytes_without_copy_events() {
        let records = vec![record(0, "/source", &[("", 500)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(settle(&mut progress, 0)), (500, 500, 1, 1));
    }

    /// 2049 个空文件那次：字节分母只有 41 B，进度条一直是 0。项数必须
    /// 随空文件的开始事件（0/0 同时是完成）前进。
    #[test]
    fn empty_files_advance_the_item_count() {
        let records = vec![record(
            0,
            "/source",
            &[("a", 0), ("b", 0), ("c", 0), ("note.txt", 41)],
        )];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(&copy_progress("/source/a", 0, 0));
        progress.observe_copy_progress(&copy_progress("/source/b", 0, 0));

        assert_eq!(bytes(progress.snapshot()), (0, 41, 2, 4));
    }

    #[test]
    fn late_events_of_a_finished_file_are_not_counted_twice() {
        let records = vec![record(0, "/source", &[("file", 100), ("other", 5)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(&copy_progress("/source/file", 0, 100));
        progress.observe_copy_progress(&copy_progress("/source/file", 100, 100));
        progress.observe_copy_progress(&copy_progress("/source/file", 100, 100));

        assert_eq!(items(progress.snapshot()), (1, 2));
    }

    #[test]
    fn current_file_is_the_largest_one_in_flight() {
        let records = vec![record(
            0,
            "/source",
            &[("small", 10), ("movie.mkv", 3_000), ("medium", 500)],
        )];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(&copy_progress("/source/small", 0, 10));
        progress.observe_copy_progress(&copy_progress("/source/movie.mkv", 0, 3_000));
        progress.observe_copy_progress(&copy_progress("/source/medium", 0, 500));
        progress.observe_copy_progress(&copy_progress("/source/movie.mkv", 1_200, 3_000));
        progress.observe_copy_progress(&copy_progress("/source/small", 10, 10));

        assert_eq!(
            current_file(progress.snapshot()),
            Some(CurrentFileProgress {
                name: "movie.mkv".to_owned(),
                completed_bytes: 1_200,
                total_bytes: 3_000,
            })
        );

        progress.observe_copy_progress(&copy_progress("/source/movie.mkv", 3_000, 3_000));
        assert_eq!(
            current_file(progress.snapshot()).map(|file| file.name),
            Some("medium".to_owned())
        );
        assert!(current_file(settle(&mut progress, 0)).is_none());
    }

    #[test]
    fn record_completion_counts_entries_without_byte_events() {
        let mut directory = record(0, "/source", &[("file", 10), ("link", 0)]);
        let manifest = directory.manifest.as_mut().unwrap();
        manifest.entries[1].identity.object_kind = FileObjectKind::SymbolicLink;
        manifest.entries.insert(
            0,
            SourceManifestEntry {
                relative_path: PathBuf::new(),
                identity: FileIdentity {
                    object_kind: FileObjectKind::Directory,
                    ..identity(0)
                },
            },
        );
        let records = vec![directory, record(1, "/next", &[("", 5)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        // 目录本身不算项：文件 + 符号链接 = 2，加下一条记录 1。
        assert_eq!(items(progress.snapshot()), (0, 3));
        progress.observe_copy_progress(&copy_progress("/source/file", 0, 10));
        progress.observe_copy_progress(&copy_progress("/source/file", 10, 10));
        assert_eq!(items(progress.snapshot()), (1, 3));
        assert_eq!(items(settle(&mut progress, 0)), (2, 3));
    }

    #[test]
    fn record_without_manifest_counts_as_one_item_however_many_files_it_copies() {
        let mut missing = record(0, "/source", &[]);
        missing.manifest = None;
        let records = vec![missing, record(1, "/next", &[("", 5)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        for name in ["a", "b", "c"] {
            let path = format!("/source/{name}");
            progress.observe_copy_progress(&copy_progress(&path, 0, 0));
        }

        assert_eq!(items(progress.snapshot()), (1, 2));
    }

    /// 度量由产出方选定，这里只照着搬：按条目的带当前文件发 Transfer，
    /// 按字节的（7z 百分比、tar 已读字节）发 Bytes，总数未知照样透传。
    #[test]
    fn archive_progress_keeps_the_measure_its_producer_chose() {
        assert!(matches!(
            archive_progress_update(ArchiveProgress::Entries {
                completed_entries: 1,
                total_entries: 3,
                bytes: Some((40, 100)),
                current_entry: Some(file_core::ArchiveEntryProgress {
                    name: "a.bin".to_owned(),
                    completed_bytes: 5,
                    total_bytes: 50,
                }),
            }),
            FileOperationProgressUpdate::Transfer {
                completed_items: 1,
                total_items: 3,
                bytes: Some((40, 100)),
                current_file: Some(CurrentFileProgress {
                    completed_bytes: 5,
                    total_bytes: 50,
                    ..
                }),
            }
        ));
        assert!(matches!(
            archive_progress_update(ArchiveProgress::Bytes {
                completed_bytes: 40,
                total_bytes: 100,
                completed_entries: 2,
                total_entries: None,
            }),
            FileOperationProgressUpdate::Bytes {
                completed_bytes: 40,
                total_bytes: 100,
                completed_items: 2,
                total_items: None,
            }
        ));
    }

    #[test]
    fn missing_manifest_and_zero_byte_batch_report_items_without_bytes() {
        let mut missing = record(0, "/source", &[("", 10)]);
        missing.manifest = None;
        let mut missing_progress = TransferBatchProgress::new(&[missing], QueuedTransferMode::Copy);
        let zero_records = vec![record(0, "/empty", &[("", 0)])];
        let mut zero_progress = TransferBatchProgress::new(&zero_records, QueuedTransferMode::Copy);

        for update in [
            settle(&mut missing_progress, 0),
            settle(&mut zero_progress, 0),
        ] {
            assert!(matches!(
                update,
                FileOperationProgressUpdate::Transfer {
                    completed_items: 1,
                    total_items: 1,
                    bytes: None,
                    ..
                }
            ));
        }
    }

    #[test]
    fn copy_mode_snapshots_cover_every_record_through_the_batch() {
        let records = vec![
            record(0, "/small", &[("", 10)]),
            record(1, "/large", &[("", 990)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);
        progress.start_records(&[0]);

        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots.len(), 2);
        // 段起点:段里的记录 Active(零字节,环画空弧),后面段的记录排队。
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(0));
        assert_eq!(snapshots[0].total_bytes, Some(10));
        assert!(!snapshots[0].is_directory);
        assert_eq!(snapshots[1].state, TransferEntrySnapshotState::Queued);
        assert_eq!(snapshots[1].completed_bytes, None);

        progress.observe_copy_progress(&copy_progress("/small", 4, 10));
        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(4));

        progress.complete_record(0);
        progress.start_records(&[1]);
        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Completed);
        assert_eq!(snapshots[0].total_bytes, Some(10));
        assert_eq!(snapshots[1].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[1].completed_bytes, Some(0));
    }

    #[test]
    fn move_mode_snapshots_stay_empty_until_bytes_actually_copy() {
        let records = vec![
            record(0, "/small", &[("", 10)]),
            record(1, "/large", &[("", 990)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Move);
        progress.start_records(&[0, 1]);

        // rename 结算推进完成计数,但未发生字节复制:不产生任何占位。
        progress.complete_record(0);
        assert!(progress.transfer_entry_snapshots().is_empty());

        // 分歧退回复制后,只有真正复制过的条目进入占位流。
        progress.observe_copy_progress(&copy_progress("/large", 495, 990));
        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].target, PathBuf::from("/large-target"));
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(495));
    }

    /// 一段里的记录同时在跑，谁先做完谁先结算，计数和字节都不能乱。
    #[test]
    fn records_of_one_segment_settle_in_any_order() {
        let records = vec![
            record(0, "/first", &[("", 10)]),
            record(1, "/second", &[("", 90)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);
        progress.start_records(&[0, 1]);

        progress.observe_copy_progress(&copy_progress("/first", 0, 10));
        progress.observe_copy_progress(&copy_progress("/second", 0, 90));
        progress.observe_copy_progress(&copy_progress("/second", 90, 90));
        assert_eq!(bytes(progress.snapshot()), (90, 100, 1, 2));
        assert_eq!(bytes(settle(&mut progress, 1)), (90, 100, 1, 2));
        assert_eq!(
            current_file(progress.snapshot()).map(|file| file.name),
            Some("first".to_owned())
        );
        assert_eq!(bytes(settle(&mut progress, 0)), (100, 100, 2, 2));
        let states = progress
            .transfer_entry_snapshots()
            .into_iter()
            .map(|snapshot| snapshot.state)
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                TransferEntrySnapshotState::Completed,
                TransferEntrySnapshotState::Completed
            ]
        );
    }

    #[test]
    fn directory_source_snapshot_flows_from_manifest_root_kind() {
        let mut directory_record = record(0, "/source", &[("inner.txt", 10)]);
        directory_record.manifest.as_mut().unwrap().entries.insert(
            0,
            SourceManifestEntry {
                relative_path: PathBuf::new(),
                identity: FileIdentity {
                    object_kind: FileObjectKind::Directory,
                    ..identity(0)
                },
            },
        );
        let progress = TransferBatchProgress::new(&[directory_record], QueuedTransferMode::Copy);

        let snapshots = progress.transfer_entry_snapshots();
        assert!(snapshots[0].is_directory);
        assert_eq!(snapshots[0].total_bytes, Some(10));
    }
}
