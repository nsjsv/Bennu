use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::{
    ArchiveCreationProgress, ArchiveExtractionProgress, CopyProgress, FileObjectKind,
    SourceManifest, TransferJournalRecord,
};
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;

use crate::model::Message;
use crate::operation_progress::{
    FileOperationProgressUpdate, TransferEntrySnapshot, TransferEntrySnapshotState,
};

use super::super::QueuedTransferMode;

pub(super) struct TransferBatchProgress {
    total_items: usize,
    completed_items: usize,
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
        Self {
            total_items: records.len(),
            completed_items: 0,
            byte_workload: batch_byte_workload(records),
            records: records.iter().map(record_display_meta).collect(),
            copy_observed: records
                .iter()
                .map(|_| matches!(mode, QueuedTransferMode::Copy))
                .collect(),
        }
    }

    pub(super) fn observe_copy_progress(
        &mut self,
        record_index: usize,
        progress: &CopyProgress,
    ) -> Option<FileOperationProgressUpdate> {
        debug_assert_eq!(record_index, self.completed_items);
        let byte_workload = self.byte_workload.as_mut()?;
        let record = byte_workload.records.get(record_index)?;
        let expected_bytes = *record.regular_files.get(progress.from.as_path())?;
        if progress.bytes_total != expected_bytes {
            return None;
        }

        let completed_file_bytes = progress.bytes_done.min(expected_bytes);
        let previous_file_bytes = byte_workload
            .current_file_bytes
            .get(progress.from.as_path())
            .copied()
            .unwrap_or(0);
        if completed_file_bytes <= previous_file_bytes {
            return None;
        }
        byte_workload
            .current_file_bytes
            .insert(progress.from.clone(), completed_file_bytes);
        byte_workload.current_record_bytes += completed_file_bytes - previous_file_bytes;
        self.copy_observed[record_index] = true;
        Some(self.snapshot())
    }

    pub(super) fn complete_record(&mut self, record_index: usize) -> FileOperationProgressUpdate {
        debug_assert_eq!(record_index, self.completed_items);
        if let Some(byte_workload) = self.byte_workload.as_mut() {
            byte_workload.resolved_bytes += byte_workload.records[record_index].logical_bytes;
            byte_workload.current_record_bytes = 0;
            byte_workload.current_file_bytes.clear();
        }
        self.completed_items += 1;
        self.snapshot()
    }

    fn snapshot(&self) -> FileOperationProgressUpdate {
        match &self.byte_workload {
            Some(byte_workload) => FileOperationProgressUpdate::Bytes {
                completed_bytes: byte_workload.resolved_bytes + byte_workload.current_record_bytes,
                total_bytes: byte_workload.total_bytes,
                completed_items: self.completed_items,
                total_items: self.total_items,
            },
            None => FileOperationProgressUpdate::IndeterminateItems {
                completed: self.completed_items,
                total: self.total_items,
            },
        }
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
                let (state, completed_bytes) = if record_index < self.completed_items {
                    (TransferEntrySnapshotState::Completed, meta.total_bytes)
                } else if record_index == self.completed_items {
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

pub(super) fn drain_latest_copy_progress(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<CopyProgress>,
    mut latest: Option<CopyProgress>,
) -> Option<CopyProgress> {
    while let Ok(progress) = receiver.try_recv() {
        latest = Some(progress);
    }
    latest
}

pub(crate) async fn send_archive_creation_progress(
    output: &mut IcedSender<Message>,
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
    output: &mut IcedSender<Message>,
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
    output: &mut IcedSender<Message>,
    task_id: u64,
    progress: FileOperationProgressUpdate,
) {
    let _ = output
        .send(Message::FileOperationProgressed(task_id, progress, Vec::new()))
        .await;
}

/// 传输批次的进度消息:聚合更新与逐条目占位快照同行发送,快照全量
/// 替换 UI 侧任务状态,占位行由此保持单一事实源。
pub(super) async fn send_transfer_batch_progress(
    output: &mut IcedSender<Message>,
    task_id: u64,
    batch_progress: &TransferBatchProgress,
    progress: FileOperationProgressUpdate,
) {
    let snapshots = batch_progress.transfer_entry_snapshots();
    let _ = output
        .send(Message::FileOperationProgressed(task_id, progress, snapshots))
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
        let FileOperationProgressUpdate::Bytes {
            completed_bytes,
            total_bytes,
            completed_items,
            total_items,
        } = update
        else {
            panic!("expected byte progress");
        };
        (completed_bytes, total_bytes, completed_items, total_items)
    }

    #[test]
    fn drain_latest_copy_progress_keeps_last_sent_sample() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(copy_progress("/source/file", 3, 10))
            .expect("send first progress sample");
        sender
            .send(copy_progress("/source/file", 7, 10))
            .expect("send last progress sample");

        let latest = drain_latest_copy_progress(&mut receiver, None)
            .expect("buffered progress should be retained");

        assert_eq!(latest.bytes_done, 7);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn small_first_file_does_not_make_directory_look_complete() {
        let records = vec![record(0, "/source", &[("small", 10), ("large", 990)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        let update = progress
            .observe_copy_progress(0, &copy_progress("/source/small", 10, 10))
            .unwrap();

        assert_eq!(bytes(update), (10, 1_000, 0, 1));
    }

    #[test]
    fn top_level_records_share_one_byte_denominator() {
        let records = vec![
            record(0, "/small", &[("", 10)]),
            record(1, "/large", &[("", 990)]),
        ];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(progress.complete_record(0)), (10, 1_000, 1, 2));
        assert_eq!(
            bytes(
                progress
                    .observe_copy_progress(1, &copy_progress("/large", 495, 990))
                    .unwrap()
            ),
            (505, 1_000, 1, 2)
        );
    }

    #[test]
    fn leaf_progress_is_monotonic_and_must_match_manifest_size() {
        let records = vec![record(0, "/source", &[("file", 100)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(
            bytes(
                progress
                    .observe_copy_progress(0, &copy_progress("/source/file", 60, 100))
                    .unwrap()
            ),
            (60, 100, 0, 1)
        );
        assert!(progress
            .observe_copy_progress(0, &copy_progress("/source/file", 20, 100))
            .is_none());
        assert!(progress
            .observe_copy_progress(0, &copy_progress("/source/file", 90, 200))
            .is_none());
    }

    #[test]
    fn completed_record_fills_logical_bytes_without_copy_events() {
        let records = vec![record(0, "/source", &[("", 500)])];
        let mut progress = TransferBatchProgress::new(&records, QueuedTransferMode::Copy);

        assert_eq!(bytes(progress.complete_record(0)), (500, 500, 1, 1));
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
    fn missing_manifest_and_zero_byte_batch_remain_indeterminate() {
        let mut missing = record(0, "/source", &[("", 10)]);
        missing.manifest = None;
        let mut missing_progress = TransferBatchProgress::new(&[missing], QueuedTransferMode::Copy);
        let zero_records = vec![record(0, "/empty", &[("", 0)])];
        let mut zero_progress =
            TransferBatchProgress::new(&zero_records, QueuedTransferMode::Copy);

        assert!(matches!(
            missing_progress.complete_record(0),
            FileOperationProgressUpdate::IndeterminateItems {
                completed: 1,
                total: 1
            }
        ));
        assert!(matches!(
            zero_progress.complete_record(0),
            FileOperationProgressUpdate::IndeterminateItems {
                completed: 1,
                total: 1
            }
        ));
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
        directory_record
            .manifest
            .as_mut()
            .unwrap()
            .entries
            .insert(
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
