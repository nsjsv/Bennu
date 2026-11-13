use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::{
    ArchiveCreationProgress, ArchiveExtractionProgress, CopyProgress, FileObjectKind,
    SourceManifest, TransferJournalRecord,
};

use crate::{
    CurrentFileProgress, FileOperationEvent, FileOperationEventSender, FileOperationProgressUpdate,
    TransferEntrySnapshot, TransferEntrySnapshotState,
};

use super::super::QueuedTransferMode;

pub(super) struct TransferBatchProgress {
    /// 每条顶层记录算几项：清单里非目录条目的个数，至少 1（空文件夹、
    /// 建不出清单的记录各算 1 项）。目录没有字节事件，计入的话会在记录
    /// 结算时整段跳一下。
    record_items: Vec<usize>,
    total_items: usize,
    completed_records: usize,
    /// 已结算记录的项数之和。
    settled_items: usize,
    /// 当前记录里已搬完的普通文件数。
    current_record_files: usize,
    /// 当前记录里已开始、未完成的文件：源路径 → (已搬字节, 总字节)。
    /// 目录并行 staging 时同时有多个。
    in_flight: HashMap<PathBuf, (u64, u64)>,
    byte_workload: Option<TransferBatchByteWorkload>,
    /// 逐条目展示元数据:占位快照的常量部分(目标/目录性/字节总量)。
    records: Vec<TransferRecordDisplayMeta>,
    /// 该条目是否进入占位流。Copy/Duplicate 全量true(入队即占位);
    /// Move 只在真正发生字节复制时置位,同盘 rename 路径永不产生占位。
    copy_observed: Vec<bool>,
}

struct TransferRecordDisplayMeta {
    target: PathBuf,
    is_directory: bool,
    total_bytes: Option<u64>,
}

struct TransferBatchByteWorkload {
    total_bytes: u64,
    resolved_bytes: u64,
    current_record_bytes: u64,
    current_file_bytes: HashMap<PathBuf, u64>,
    records: Vec<TransferRecordByteWorkload>,
}

struct TransferRecordByteWorkload {
    logical_bytes: u64,
    regular_files: HashMap<PathBuf, u64>,
}

impl TransferBatchProgress {
    pub(super) fn new(records: &[TransferJournalRecord], mode: QueuedTransferMode) -> Self {
        let record_items = records.iter().map(record_item_count).collect::<Vec<_>>();
        Self {
            total_items: record_items.iter().sum(),
            record_items,
            completed_records: 0,
            settled_items: 0,
            current_record_files: 0,
            in_flight: HashMap::new(),
            byte_workload: batch_byte_workload(records),
            records: records.iter().map(record_display_meta).collect(),
            copy_observed: records
                .iter()
                .map(|_| matches!(mode, QueuedTransferMode::Copy))
                .collect(),
        }
    }

    /// 每一条 `CopyProgress` 都要经过这里（只有发往界面的消息才节流），
    /// 否则完成计数会随被丢掉的事件一起丢失。
    pub(super) fn observe_copy_progress(&mut self, record_index: usize, progress: &CopyProgress) {
        debug_assert_eq!(record_index, self.completed_records);
        self.copy_observed[record_index] = true;
        self.observe_file_lifecycle(progress);
        self.observe_copied_bytes(record_index, progress);
    }

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
        self.current_record_files += 1;
    }

    fn observe_copied_bytes(&mut self, record_index: usize, progress: &CopyProgress) {
        let Some(byte_workload) = self.byte_workload.as_mut() else {
            return;
        };
        let Some(record) = byte_workload.records.get(record_index) else {
            return;
        };
        let Some(&expected_bytes) = record.regular_files.get(progress.from.as_path()) else {
            return;
        };
        if progress.bytes_total != expected_bytes {
            return;
        }

        let completed_file_bytes = progress.bytes_done.min(expected_bytes);
        let previous_file_bytes = byte_workload
            .current_file_bytes
            .get(progress.from.as_path())
            .copied()
            .unwrap_or(0);
        if completed_file_bytes <= previous_file_bytes {
            return;
        }
        byte_workload
            .current_file_bytes
            .insert(progress.from.clone(), completed_file_bytes);
        byte_workload.current_record_bytes += completed_file_bytes - previous_file_bytes;
    }

    pub(super) fn complete_record(&mut self, record_index: usize) -> FileOperationProgressUpdate {
        debug_assert_eq!(record_index, self.completed_records);
        if let Some(byte_workload) = self.byte_workload.as_mut() {
            byte_workload.resolved_bytes += byte_workload.records[record_index].logical_bytes;
            byte_workload.current_record_bytes = 0;
            byte_workload.current_file_bytes.clear();
        }
        // 符号链接这类没有字节事件的条目在这里补齐。
        self.settled_items += self.record_items[record_index];
        self.completed_records += 1;
        self.current_record_files = 0;
        self.in_flight.clear();
        self.snapshot()
    }

    pub(super) fn snapshot(&self) -> FileOperationProgressUpdate {
        // 清单缺失的记录只算 1 项，里面搬完的文件不能越过它的份额。
        let current_record_items = self
            .record_items
            .get(self.completed_records)
            .copied()
            .unwrap_or_default();
        FileOperationProgressUpdate::Transfer {
            completed_items: self.settled_items
                + self.current_record_files.min(current_record_items),
            total_items: self.total_items,
            bytes: self.byte_workload.as_ref().map(|byte_workload| {
                (
                    byte_workload.resolved_bytes + byte_workload.current_record_bytes,
                    byte_workload.total_bytes,
                )
            }),
            current_file: self.current_file(),
        }
    }

    /// 并行搬运时取最大的那个：小文件一闪就完，条不会在它们之间乱跳。
    /// 同样大小按路径定序，避免 HashMap 遍历顺序让显示来回换。
    fn current_file(&self) -> Option<CurrentFileProgress> {
        self.in_flight
            .iter()
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

    /// 全量占位快照:按位置推导每条目的排队/进行/完成状态,随进度消息
    /// 整批替换 UI 侧 `transfer_progress`。Move 任务未开始复制的条目
    /// (同盘 rename)不进入快照,从源头保证不产生占位。
    pub(super) fn transfer_entry_snapshots(&self) -> Vec<TransferEntrySnapshot> {
        self.records
            .iter()
            .zip(&self.copy_observed)
            .enumerate()
            .filter(|(_, (_, copy_observed))| **copy_observed)
            .map(|(record_index, (meta, _))| {
                let (state, completed_bytes) = if record_index < self.completed_records {
                    (TransferEntrySnapshotState::Completed, meta.total_bytes)
                } else if record_index == self.completed_records {
                    let current_bytes = self
                        .byte_workload
                        .as_ref()
                        .map(|workload| workload.current_record_bytes);
                    (
                        TransferEntrySnapshotState::Active,
                        Some(current_bytes.unwrap_or_default()),
                    )
                } else {
                    (TransferEntrySnapshotState::Queued, None)
                };
                TransferEntrySnapshot {
                    target: meta.target.clone(),
                    is_directory: meta.is_directory,
                    completed_bytes,
                    total_bytes: meta.total_bytes,
                    state,
                }
            })
            .collect()
    }
}

fn record_display_meta(record: &TransferJournalRecord) -> TransferRecordDisplayMeta {
    TransferRecordDisplayMeta {
        target: record.request.requested_target.clone(),
        // 清单首条即源根自身:目录源的根条目 kind 为 Directory。
        is_directory: record
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.entries.first())
            .is_some_and(|root| root.identity.object_kind == FileObjectKind::Directory),
        total_bytes: record
            .manifest
            .as_ref()
            .map(|manifest| manifest_regular_bytes(manifest)),
    }
}

fn manifest_regular_bytes(manifest: &SourceManifest) -> u64 {
    manifest
        .entries
        .iter()
        .filter(|entry| entry.identity.object_kind == FileObjectKind::RegularFile)
        .fold(0_u64, |total, entry| {
            total.saturating_add(entry.identity.size)
        })
}

fn record_item_count(record: &TransferJournalRecord) -> usize {
    record
        .manifest
        .as_ref()
        .map_or(0, |manifest| {
            manifest
                .entries
                .iter()
                .filter(|entry| entry.identity.object_kind != FileObjectKind::Directory)
                .count()
        })
        .max(1)
}

pub(crate) async fn send_archive_creation_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    progress: ArchiveCreationProgress,
) {
    send_file_operation_progress(
        output,
        task_id,
        archive_progress_update(
            progress.completed_source_bytes,
            progress.total_source_bytes,
            progress.completed_entries,
            progress.total_entries,
        ),
    )
    .await;
}

pub(crate) async fn send_archive_extraction_progress(
    output: &FileOperationEventSender,
    task_id: u64,
    progress: ArchiveExtractionProgress,
) {
    send_file_operation_progress(
        output,
        task_id,
        archive_progress_update(
            progress.completed_bytes,
            progress.total_bytes,
            progress.completed_entries,
            progress.total_entries,
        ),
    )
    .await;
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

fn archive_progress_update(
    completed_bytes: u64,
    total_bytes: u64,
    completed_items: usize,
    total_items: usize,
) -> FileOperationProgressUpdate {
    if total_bytes == 0 {
        FileOperationProgressUpdate::IndeterminateItems {
            completed: completed_items,
            total: total_items,
        }
    } else {
        FileOperationProgressUpdate::Bytes {
            completed_bytes,
            total_bytes,
            completed_items,
            total_items,
        }
    }
}

fn batch_byte_workload(records: &[TransferJournalRecord]) -> Option<TransferBatchByteWorkload> {
    let records = records
        .iter()
        .map(|record| record.manifest.as_ref().and_then(record_byte_workload))
        .collect::<Option<Vec<_>>>()?;
    let total_bytes = records.iter().try_fold(0_u64, |total, record| {
        total.checked_add(record.logical_bytes)
    })?;
    if total_bytes == 0 {
        return None;
    }

    Some(TransferBatchByteWorkload {
        total_bytes,
        resolved_bytes: 0,
        current_record_bytes: 0,
        current_file_bytes: HashMap::new(),
        records,
    })
}

fn record_byte_workload(manifest: &SourceManifest) -> Option<TransferRecordByteWorkload> {
    let mut logical_bytes = 0_u64;
    let mut regular_files = HashMap::new();
    for entry in &manifest.entries {
        if entry.identity.object_kind != FileObjectKind::RegularFile {
            continue;
        }
        logical_bytes = logical_bytes.checked_add(entry.identity.size)?;
        let source_path = manifest_entry_path(manifest, &entry.relative_path);
        regular_files.insert(source_path, entry.identity.size);
    }
    Some(TransferRecordByteWorkload {
        logical_bytes,
        regular_files,
    })
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

        progress.observe_copy_progress(0, &copy_progress("/source/small", 0, 10));
        progress.observe_copy_progress(0, &copy_progress("/source/small", 10, 10));

        assert_eq!(bytes(progress.snapshot()), (10, 1_000, 1, 2));
    }

    #[test]
    fn top_level_records_share_one_byte_denominator() {
        let records = vec![
            record(0, "/small", &[("", 10)]),
            record(1, "/large", &[("", 990)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(progress.complete_record(0)), (10, 1_000, 1, 2));
        progress.observe_copy_progress(1, &copy_progress("/large", 495, 990));
        assert_eq!(bytes(progress.snapshot()), (505, 1_000, 1, 2));
    }

    #[test]
    fn leaf_progress_is_monotonic_and_must_match_manifest_size() {
        let records = vec![record(0, "/source", &[("file", 100)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(0, &copy_progress("/source/file", 60, 100));
        assert_eq!(bytes(progress.snapshot()), (60, 100, 0, 1));
        progress.observe_copy_progress(0, &copy_progress("/source/file", 20, 100));
        progress.observe_copy_progress(0, &copy_progress("/source/file", 90, 200));
        assert_eq!(bytes(progress.snapshot()), (60, 100, 0, 1));
    }

    #[test]
    fn completed_record_fills_logical_bytes_without_copy_events() {
        let records = vec![record(0, "/source", &[("", 500)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(progress.complete_record(0)), (500, 500, 1, 1));
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

        progress.observe_copy_progress(0, &copy_progress("/source/a", 0, 0));
        progress.observe_copy_progress(0, &copy_progress("/source/b", 0, 0));

        assert_eq!(bytes(progress.snapshot()), (0, 41, 2, 4));
    }

    #[test]
    fn late_events_of_a_finished_file_are_not_counted_twice() {
        let records = vec![record(0, "/source", &[("file", 100), ("other", 5)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        progress.observe_copy_progress(0, &copy_progress("/source/file", 0, 100));
        progress.observe_copy_progress(0, &copy_progress("/source/file", 100, 100));
        progress.observe_copy_progress(0, &copy_progress("/source/file", 100, 100));

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

        progress.observe_copy_progress(0, &copy_progress("/source/small", 0, 10));
        progress.observe_copy_progress(0, &copy_progress("/source/movie.mkv", 0, 3_000));
        progress.observe_copy_progress(0, &copy_progress("/source/medium", 0, 500));
        progress.observe_copy_progress(0, &copy_progress("/source/movie.mkv", 1_200, 3_000));
        progress.observe_copy_progress(0, &copy_progress("/source/small", 10, 10));

        assert_eq!(
            current_file(progress.snapshot()),
            Some(CurrentFileProgress {
                name: "movie.mkv".to_owned(),
                completed_bytes: 1_200,
                total_bytes: 3_000,
            })
        );

        progress.observe_copy_progress(0, &copy_progress("/source/movie.mkv", 3_000, 3_000));
        assert_eq!(
            current_file(progress.snapshot()).map(|file| file.name),
            Some("medium".to_owned())
        );
        assert!(current_file(progress.complete_record(0)).is_none());
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
        progress.observe_copy_progress(0, &copy_progress("/source/file", 0, 10));
        progress.observe_copy_progress(0, &copy_progress("/source/file", 10, 10));
        assert_eq!(items(progress.snapshot()), (1, 3));
        assert_eq!(items(progress.complete_record(0)), (2, 3));
    }

    #[test]
    fn record_without_manifest_counts_as_one_item_however_many_files_it_copies() {
        let mut missing = record(0, "/source", &[]);
        missing.manifest = None;
        let records = vec![missing, record(1, "/next", &[("", 5)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        for name in ["a", "b", "c"] {
            let path = format!("/source/{name}");
            progress.observe_copy_progress(0, &copy_progress(&path, 0, 0));
        }

        assert_eq!(items(progress.snapshot()), (1, 2));
    }

    #[test]
    fn archive_bytes_drive_fraction_while_zero_byte_workload_stays_indeterminate() {
        assert!(matches!(
            archive_progress_update(40, 100, 2, 4),
            FileOperationProgressUpdate::Bytes {
                completed_bytes: 40,
                total_bytes: 100,
                completed_items: 2,
                total_items: 4,
            }
        ));
        assert!(matches!(
            archive_progress_update(0, 0, 2, 4),
            FileOperationProgressUpdate::IndeterminateItems {
                completed: 2,
                total: 4,
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
            missing_progress.complete_record(0),
            zero_progress.complete_record(0),
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

        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots.len(), 2);
        // 批次起点:当前记录 Active(零字节,环画空弧),后续记录排队。
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(0));
        assert_eq!(snapshots[0].total_bytes, Some(10));
        assert!(!snapshots[0].is_directory);
        assert_eq!(snapshots[1].state, TransferEntrySnapshotState::Queued);
        assert_eq!(snapshots[1].completed_bytes, None);

        progress.observe_copy_progress(0, &copy_progress("/small", 4, 10));
        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(4));

        progress.complete_record(0);
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

        // rename 结算推进完成计数,但未发生字节复制:不产生任何占位。
        progress.complete_record(0);
        assert!(progress.transfer_entry_snapshots().is_empty());

        // 分歧退回复制后,只有真正复制过的条目进入占位流。
        progress.observe_copy_progress(1, &copy_progress("/large", 495, 990));
        let snapshots = progress.transfer_entry_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].target, PathBuf::from("/large-target"));
        assert_eq!(snapshots[0].state, TransferEntrySnapshotState::Active);
        assert_eq!(snapshots[0].completed_bytes, Some(495));
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
