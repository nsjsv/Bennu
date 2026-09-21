//! 传入条目占位的派生层:占位不是独立状态,而是渲染时从操作队列任务
//! 快照(`FileOperationTask::transfer_progress`)过滤推导的纯函数结果。
//! 任务终结后占位自动消失,无清理逻辑、无漂移可能。
//!
//! 排序语义对齐 `file_core::sort`(sort.rs 的 compare_entries):
//! 先 directories_first,再按字段比较,平局回退名字,最后套方向。

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use file_core::{DirectoryEntry, SortDirection, SortField};

use crate::operation_progress::{active_byte_fraction, TransferEntrySnapshotState};
use crate::operation_queue::FileOperationQueue;

/// 目标目录正在传入的顶层条目占位。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferPlaceholder {
    /// 最终落地名(含重名序号),占位行直接显示。
    pub(crate) name: String,
    pub(crate) is_directory: bool,
    /// `Some(0..1)` 走进度环;`None` 表示未开始/字节数未知,画空环。
    pub(crate) progress: Option<f32>,
    /// 子树整体字节量(复制文件夹时为清单合计),Size 排序键;未知按 0。
    pub(crate) total_bytes: Option<u64>,
    /// 任务入队时刻,Modified 排序键(≈最新落地)。
    pub(crate) enqueued_at: SystemTime,
}

/// 占位插入位置使用的排序配置,来自视图当前目录的实际排序
/// (根目录为扫描选项,展开目录为各自的 directory_order_phase)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferSortOptions {
    pub(crate) field: SortField,
    pub(crate) direction: SortDirection,
    pub(crate) directories_first: bool,
}

/// 收集目标为 `directory` 的未终结传输占位。排队未运行的批次同样出现
/// (入队即初始化快照);运行中任务由后台逐条目快照整批替换驱动。
pub(crate) fn transfer_placeholders_for_directory(
    queue: &FileOperationQueue,
    directory: &Path,
) -> Vec<TransferPlaceholder> {
    let mut placeholders = Vec::new();
    for task in queue.tasks() {
        if task.status.is_terminal() {
            continue;
        }
        for snapshot in &task.transfer_progress {
            if snapshot.target.parent() != Some(directory) {
                continue;
            }
            let progress = match snapshot.state {
                TransferEntrySnapshotState::Completed => Some(1.0),
                TransferEntrySnapshotState::Queued => None,
                TransferEntrySnapshotState::Active => snapshot
                    .completed_bytes
                    .zip(snapshot.total_bytes)
                    .and_then(|(completed, total)| active_byte_fraction(completed, total)),
            };
            placeholders.push(TransferPlaceholder {
                name: snapshot
                    .target
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                is_directory: snapshot.is_directory,
                progress,
                total_bytes: snapshot.total_bytes,
                enqueued_at: task.enqueued_at,
            });
        }
    }
    placeholders
}

/// 占位相对真实条目的排序位置:Greater 表示占位应排在条目之后。
/// 与 sort.rs 一致:directories_first 先行且不受方向反转。
pub(crate) fn compare_placeholder_with_entry(
    placeholder: &TransferPlaceholder,
    entry: &DirectoryEntry,
    sort: TransferSortOptions,
) -> Ordering {
    if sort.directories_first
        && placeholder.is_directory != (entry.kind == file_core::FileKind::Directory)
    {
        return if placeholder.is_directory {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let field_ordering = match sort.field {
        SortField::Name => compare_names(&placeholder.name, entry.name()),
        SortField::Size => placeholder
            .total_bytes
            .unwrap_or(0)
            .cmp(&entry.metadata.len)
            .then_with(|| compare_names(&placeholder.name, entry.name())),
        SortField::Kind => placeholder_kind_rank(placeholder.is_directory)
            .cmp(&entry_kind_rank(entry))
            .then_with(|| compare_names(&placeholder.name, entry.name())),
        SortField::Modified => Some(placeholder.enqueued_at)
            .cmp(&entry.metadata.modified)
            .then_with(|| compare_names(&placeholder.name, entry.name())),
    };
    apply_sort_direction(field_ordering, sort.direction)
}

fn compare_placeholder_with_placeholder(
    left: &TransferPlaceholder,
    right: &TransferPlaceholder,
    sort: TransferSortOptions,
) -> Ordering {
    if sort.directories_first && left.is_directory != right.is_directory {
        return if left.is_directory {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let field_ordering = match sort.field {
        SortField::Name => compare_names(&left.name, OsStr::new(&right.name)),
        SortField::Size => left
            .total_bytes
            .unwrap_or(0)
            .cmp(&right.total_bytes.unwrap_or(0))
            .then_with(|| compare_names(&left.name, OsStr::new(&right.name))),
        SortField::Kind => placeholder_kind_rank(left.is_directory)
            .cmp(&placeholder_kind_rank(right.is_directory))
            .then_with(|| compare_names(&left.name, OsStr::new(&right.name))),
        SortField::Modified => left
            .enqueued_at
            .cmp(&right.enqueued_at)
            .then_with(|| compare_names(&left.name, OsStr::new(&right.name))),
    };
    apply_sort_direction(field_ordering, sort.direction)
}

/// 把占位按排序合入真实条目流:返回值顺序即渲染顺序。占位来自多个
/// 队列任务、彼此无序,先按同一比较器排好再归并。
///
/// 与真实条目同名的占位不再单独成行——目录复制期间目标目录从创建起
/// 就与占位同名并存,独立占位行会让整个复制过程出现两行同名;改为把
/// 占位作为 `transfer` 装饰挂到真实行上(图标叠加进度环)。
pub(crate) fn merge_entries_with_placeholders<'a>(
    entries: &'a [DirectoryEntry],
    placeholders: &[TransferPlaceholder],
    sort: TransferSortOptions,
) -> Vec<MergedTransferItem<'a>> {
    let entry_names: std::collections::HashSet<&OsStr> =
        entries.iter().map(|entry| entry.name()).collect();
    let mut decorations: std::collections::HashMap<String, TransferPlaceholder> =
        std::collections::HashMap::new();
    let mut unmatched: Vec<TransferPlaceholder> = Vec::new();
    for placeholder in placeholders {
        if entry_names.contains(OsStr::new(&placeholder.name)) {
            decorations.insert(placeholder.name.clone(), placeholder.clone());
        } else {
            unmatched.push(placeholder.clone());
        }
    }
    unmatched.sort_by(|left, right| compare_placeholder_with_placeholder(left, right, sort));
    let mut merged = Vec::with_capacity(entries.len() + unmatched.len());
    let mut pending = unmatched.iter().peekable();
    for entry in entries {
        while let Some(placeholder) = pending.peek() {
            if compare_placeholder_with_entry(placeholder, entry, sort) != Ordering::Less {
                break;
            }
            merged.push(MergedTransferItem::Placeholder(
                pending.next().unwrap().clone(),
            ));
        }
        let transfer = decorations.remove(entry.name().to_string_lossy().as_ref());
        merged.push(MergedTransferItem::Entry { entry, transfer });
    }
    merged.extend(pending.map(|placeholder| MergedTransferItem::Placeholder(placeholder.clone())));
    merged
}

#[derive(Debug)]
pub(crate) enum MergedTransferItem<'a> {
    /// 真实条目;`transfer` 为正在写入该条目的同名传输占位,行渲染时在
    /// 图标槽叠加进度环。
    Entry {
        entry: &'a DirectoryEntry,
        transfer: Option<TransferPlaceholder>,
    },
    /// 占位按值携带:条目借用视图数据,占位来自队列派生的临时集合。
    Placeholder(TransferPlaceholder),
}

/// 占位在 Kind 排序下的档位:只区分目录/文件,与 sort.rs 的 kind_rank
/// 对齐(目录 0、文件 1);符号链接落地后按扩展名图标呈现,归入文件档。
fn placeholder_kind_rank(is_directory: bool) -> u8 {
    if is_directory {
        0
    } else {
        1
    }
}

fn entry_kind_rank(entry: &DirectoryEntry) -> u8 {
    match entry.kind {
        file_core::FileKind::Directory => 0,
        file_core::FileKind::File => 1,
        file_core::FileKind::Symlink => 2,
        file_core::FileKind::Other => 3,
    }
}

fn apply_sort_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

#[cfg(unix)]
fn compare_names(left: &str, right: &OsStr) -> Ordering {
    use std::os::unix::ffi::OsStrExt;
    left.as_bytes().cmp(right.as_bytes())
}

#[cfg(not(unix))]
fn compare_names(left: &str, right: &OsStr) -> Ordering {
    left.cmp(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

    use super::*;

    fn entry(path: &str, kind: FileKind, len: u64, modified: Option<SystemTime>) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from(path),
            kind,
            EntryMetadata {
                len,
                modified,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    fn placeholder(name: &str, is_directory: bool, progress: Option<f32>) -> TransferPlaceholder {
        TransferPlaceholder {
            name: name.to_owned(),
            is_directory,
            progress,
            total_bytes: Some(500),
            enqueued_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn name_sort(direction: SortDirection) -> TransferSortOptions {
        TransferSortOptions {
            field: SortField::Name,
            direction,
            directories_first: false,
        }
    }

    #[test]
    fn terminal_tasks_stop_producing_placeholders() {
        let directory = Path::new("/target");
        let (mut queue, _storage) = placeholder_test_queue();
        let running_id = enqueue_transfer(&mut queue, vec![transfer("/target/incoming.txt")]);
        let canceled_id = enqueue_transfer(&mut queue, vec![transfer("/target/canceled.txt")]);
        queue.cancel(canceled_id);
        queue.finish(
            canceled_id,
            crate::operation_queue::FileOperationFinish::Canceled,
        );
        queue.finish(
            running_id,
            crate::operation_queue::FileOperationFinish::Failed("disk full".to_owned()),
        );

        assert!(transfer_placeholders_for_directory(&queue, directory).is_empty());
    }

    #[test]
    fn queued_copy_shows_placeholders_with_unknown_progress() {
        let directory = Path::new("/target");
        let (mut queue, _storage) = placeholder_test_queue();
        enqueue_transfer(
            &mut queue,
            vec![transfer("/target/b.txt"), transfer("/target/a.txt")],
        );

        let placeholders = transfer_placeholders_for_directory(&queue, directory);
        assert_eq!(placeholders.len(), 2);
        assert!(placeholders
            .iter()
            .all(|placeholder| placeholder.progress.is_none()));
        assert_eq!(placeholders[0].name, "b.txt");
    }

    #[test]
    fn move_task_without_transfer_progress_stays_empty() {
        let directory = Path::new("/target");
        let (mut queue, _storage) = placeholder_test_queue();
        enqueue_move(&mut queue, vec![transfer("/target/renamed.txt")]);

        // 同盘 move 的占位初值为空:rename 路径无字节进度,永不出现占位。
        assert!(transfer_placeholders_for_directory(&queue, directory).is_empty());
    }

    #[test]
    fn running_snapshot_progress_maps_to_ring_fraction_and_completion() {
        use crate::operation_progress::{TransferEntrySnapshot, TransferEntrySnapshotState};

        let directory = Path::new("/target");
        let (mut queue, _storage) = placeholder_test_queue();
        let task_id = enqueue_transfer(&mut queue, vec![transfer("/target/incoming.txt")]);
        queue.update_progress(
            task_id,
            crate::operation_progress::FileOperationProgressUpdate::Bytes {
                completed_bytes: 0,
                total_bytes: 0,
                completed_items: 0,
                total_items: 1,
            },
            vec![TransferEntrySnapshot {
                target: PathBuf::from("/target/incoming.txt"),
                is_directory: false,
                completed_bytes: Some(250),
                total_bytes: Some(1_000),
                state: TransferEntrySnapshotState::Active,
            }],
        );

        let placeholders = transfer_placeholders_for_directory(&queue, directory);
        assert_eq!(placeholders.len(), 1);
        let progress = placeholders[0]
            .progress
            .expect("active transfer has a fraction");
        assert!((progress - 0.25).abs() < 0.001, "got {progress}");

        // 完成态精确 1.0;排队态保持 None(空环)。
        queue.update_progress(
            task_id,
            crate::operation_progress::FileOperationProgressUpdate::Bytes {
                completed_bytes: 1_000,
                total_bytes: 1_000,
                completed_items: 1,
                total_items: 1,
            },
            vec![TransferEntrySnapshot {
                target: PathBuf::from("/target/incoming.txt"),
                is_directory: false,
                completed_bytes: Some(1_000),
                total_bytes: Some(1_000),
                state: TransferEntrySnapshotState::Completed,
            }],
        );
        let placeholders = transfer_placeholders_for_directory(&queue, directory);
        assert_eq!(placeholders[0].progress, Some(1.0));
    }

    #[test]
    fn placeholders_match_only_their_target_directory() {
        let (mut queue, _storage) = placeholder_test_queue();
        enqueue_transfer(&mut queue, vec![transfer("/elsewhere/file.txt")]);

        assert!(transfer_placeholders_for_directory(&queue, Path::new("/target")).is_empty());
        assert_eq!(
            transfer_placeholders_for_directory(&queue, Path::new("/elsewhere")).len(),
            1
        );
    }

    #[test]
    fn same_name_placeholder_decorates_the_entry_without_a_duplicate_row() {
        // 目录复制期间目标目录从创建起就与占位同名并存:同名占位必须挂到
        // 真实行上做装饰,而不是再渲染一行独立占位(两行同名)。
        let entries = [entry("/target/incoming.bin", FileKind::File, 100, None)];
        let placeholders = [placeholder("incoming.bin", false, Some(0.25))];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Ascending),
        );

        assert_eq!(merged.len(), 1);
        match &merged[0] {
            MergedTransferItem::Entry { transfer, .. } => {
                let transfer = transfer
                    .as_ref()
                    .expect("same-name transfer decorates the entry");
                assert_eq!(transfer.name, "incoming.bin");
                assert_eq!(transfer.progress, Some(0.25));
            }
            MergedTransferItem::Placeholder(_) => {
                panic!("placeholder must not duplicate the entry")
            }
        }
    }

    #[test]
    fn same_name_directory_placeholder_decorates_the_real_directory() {
        // 目录场景(用户报告的形态):真实目录已在列表中且逐步填充。
        let entries = [entry(
            "/target/Hollow Knight Silksong",
            FileKind::Directory,
            442,
            None,
        )];
        let placeholders = [placeholder("Hollow Knight Silksong", true, None)];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Ascending),
        );

        assert_eq!(merged.len(), 1);
        assert!(matches!(
            &merged[0],
            MergedTransferItem::Entry {
                transfer: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn merge_inserts_placeholder_at_sorted_name_position() {
        let entries = [
            entry("/target/alpha.txt", FileKind::File, 1, None),
            entry("/target/zulu.txt", FileKind::File, 1, None),
        ];
        let placeholders = [placeholder("mike.txt", false, Some(0.5))];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Ascending),
        );

        let labels = merged
            .iter()
            .map(|item| match item {
                MergedTransferItem::Entry { entry, .. } => {
                    entry.name().to_string_lossy().into_owned()
                }
                MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["alpha.txt", "mike.txt", "zulu.txt"]);
    }

    #[test]
    fn merge_respects_descending_direction_and_directories_first() {
        let entries = [
            entry("/target/b.txt", FileKind::File, 1, None),
            entry("/target/a.txt", FileKind::File, 1, None),
        ];
        let placeholders = [placeholder("z.txt", false, None)];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Descending),
        );
        let labels = merged
            .iter()
            .map(|item| match item {
                MergedTransferItem::Entry { entry, .. } => {
                    entry.name().to_string_lossy().into_owned()
                }
                MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["z.txt", "b.txt", "a.txt"]);

        // dirs_first:目录占位排在任何文件之前,与方向无关。
        let directory_placeholders = [placeholder("z-dir", true, None)];
        let merged = merge_entries_with_placeholders(
            &entries,
            &directory_placeholders,
            TransferSortOptions {
                directories_first: true,
                ..name_sort(SortDirection::Descending)
            },
        );
        assert!(matches!(merged[0], MergedTransferItem::Placeholder(_)));
    }

    #[test]
    fn size_sort_uses_total_bytes_and_modified_sort_defaults_to_enqueue_time() {
        let entries = [
            entry("/target/small.txt", FileKind::File, 100, None),
            entry("/target/big.txt", FileKind::File, 10_000, None),
        ];
        let placeholder = placeholder("incoming.txt", false, None);

        // 占位 total_bytes=500:按 Size 升序落在 small 与 big 之间。
        let size_placeholders = [placeholder.clone()];
        let merged = merge_entries_with_placeholders(
            &entries,
            &size_placeholders,
            TransferSortOptions {
                field: SortField::Size,
                ..name_sort(SortDirection::Ascending)
            },
        );
        let labels = merged
            .iter()
            .map(|item| match item {
                MergedTransferItem::Entry { entry, .. } => {
                    entry.name().to_string_lossy().into_owned()
                }
                MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["small.txt", "incoming.txt", "big.txt"]);

        // Modified:占位键是入队时刻(晚于条目的旧时间戳),升序排最后。
        let modified = SystemTime::UNIX_EPOCH;
        let old_entries = [entry(
            "/target/old.txt",
            FileKind::File,
            1,
            Some(modified - std::time::Duration::from_secs(60)),
        )];
        let modified_placeholders = [placeholder];
        let merged = merge_entries_with_placeholders(
            &old_entries,
            &modified_placeholders,
            TransferSortOptions {
                field: SortField::Modified,
                ..name_sort(SortDirection::Ascending)
            },
        );
        assert!(matches!(merged[1], MergedTransferItem::Placeholder(_)));
    }

    #[test]
    fn merge_sorts_unordered_placeholders_from_multiple_tasks() {
        // 占位派生自队列任务顺序,彼此无序;合并必须先排序再归并,
        // 否则一批多文件的渲染顺序违反"按名字排在正常位置"。
        let entries = [entry("/target/zulu.txt", FileKind::File, 1, None)];
        let placeholders = [
            placeholder("mike.txt", false, None),
            placeholder("alpha.txt", false, None),
            placeholder("bravo.txt", false, None),
        ];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Ascending),
        );

        let labels = merged
            .iter()
            .map(|item| match item {
                MergedTransferItem::Entry { entry, .. } => {
                    entry.name().to_string_lossy().into_owned()
                }
                MergedTransferItem::Placeholder(placeholder) => placeholder.name.clone(),
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, ["alpha.txt", "bravo.txt", "mike.txt", "zulu.txt"]);
    }

    #[test]
    fn same_name_tie_puts_real_entry_before_placeholder() {
        // 重名冲突策略为替换时,占位与真文件同名:同名占位装饰真实行,
        // 不再单独成行(旧行为是两行同名并存整个复制过程)。
        let entries = [entry("/target/dup.txt", FileKind::File, 1, None)];
        let placeholders = [placeholder("dup.txt", false, None)];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            name_sort(SortDirection::Ascending),
        );

        assert_eq!(merged.len(), 1);
        assert!(matches!(
            merged[0],
            MergedTransferItem::Entry {
                transfer: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn kind_sort_ranks_directory_placeholder_with_directories() {
        // Kind 排序(不开 dirs_first)下目录占位与目录条目同档,排在文件前。
        let entries = [entry("/target/file.txt", FileKind::File, 1, None)];
        let placeholders = [placeholder("incoming", true, None)];

        let merged = merge_entries_with_placeholders(
            &entries,
            &placeholders,
            TransferSortOptions {
                field: SortField::Kind,
                ..name_sort(SortDirection::Ascending)
            },
        );

        assert!(matches!(merged[0], MergedTransferItem::Placeholder(_)));
        assert!(matches!(
            merged[1],
            MergedTransferItem::Entry { transfer: None, .. }
        ));
    }

    fn transfer(target: &str) -> crate::operation_queue::QueuedTransfer {
        crate::operation_queue::QueuedTransfer::new(PathBuf::from("/source"), PathBuf::from(target))
    }

    /// 恢复式传输入队需要任务存储;测试用同步落盘的临时存储,
    /// TempDir 由测试绑定到用例结束。
    fn placeholder_test_queue() -> (FileOperationQueue, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let store =
            file_operation_store::TaskQueueStore::new(directory.path().join("state.sqlite"))
                .unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store);
        (queue, directory)
    }

    fn enqueue_transfer(
        queue: &mut FileOperationQueue,
        mut transfers: Vec<crate::operation_queue::QueuedTransfer>,
    ) -> u64 {
        for transfer in &mut transfers {
            transfer.conflict_strategy = file_core::TransferConflictStrategy::Fail;
        }
        match queue.enqueue(crate::operation_queue::QueuedFileOperation::Copy {
            transfers,
            verification: file_core::FileOperationVerification::BasicMetadata,
        }) {
            crate::operation_queue::FileOperationEnqueueOutcome::Queued { task_id } => task_id,
            crate::operation_queue::FileOperationEnqueueOutcome::QueuedWithStorageWarning {
                error,
                ..
            }
            | crate::operation_queue::FileOperationEnqueueOutcome::Rejected { error } => {
                panic!("copy should enqueue, got storage error: {error}")
            }
        }
    }

    fn enqueue_move(
        queue: &mut FileOperationQueue,
        transfers: Vec<crate::operation_queue::QueuedTransfer>,
    ) -> u64 {
        match queue.enqueue(crate::operation_queue::QueuedFileOperation::Move {
            transfers,
            verification: file_core::FileOperationVerification::BasicMetadata,
        }) {
            crate::operation_queue::FileOperationEnqueueOutcome::Queued { task_id } => task_id,
            crate::operation_queue::FileOperationEnqueueOutcome::QueuedWithStorageWarning {
                error,
                ..
            }
            | crate::operation_queue::FileOperationEnqueueOutcome::Rejected { error } => {
                panic!("move should enqueue, got storage error: {error}")
            }
        }
    }
}
