use std::path::PathBuf;

use file_operation_store::StoredProgress;

// 纯搬移：字节进度分数与远程下载面板随预览模型下沉 bennu-preview，
// 此处 re-export 维持 crate::operation_progress::* 调用路径单源
// （本文件与传输占位行消费分数函数；轨道 SVG 句柄由任务队列视图
// 经 bennu-preview 路径直接消费）。
pub(crate) use bennu_preview::operation_progress::active_byte_fraction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOperationWorkProgress {
    Indeterminate,
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
    },
    Completed {
        bytes: Option<(u64, u64)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileOperationProgress {
    work: FileOperationWorkProgress,
    completed_items: usize,
    total_items: Option<usize>,
}

impl FileOperationProgress {
    pub(crate) fn pending() -> Self {
        Self {
            work: FileOperationWorkProgress::Indeterminate,
            completed_items: 0,
            total_items: None,
        }
    }

    pub(crate) fn fraction(self) -> Option<f32> {
        match self.work {
            FileOperationWorkProgress::Indeterminate => None,
            FileOperationWorkProgress::Bytes {
                completed_bytes,
                total_bytes,
            } => active_byte_fraction(completed_bytes, total_bytes),
            FileOperationWorkProgress::Completed { .. } => Some(1.0),
        }
    }

    pub(crate) fn bytes(self) -> Option<(u64, u64)> {
        match self.work {
            FileOperationWorkProgress::Bytes {
                completed_bytes,
                total_bytes,
            } => Some((completed_bytes, total_bytes)),
            FileOperationWorkProgress::Completed { bytes } => bytes,
            FileOperationWorkProgress::Indeterminate => None,
        }
    }

    pub(crate) fn items(self) -> Option<(usize, usize)> {
        self.total_items
            .map(|total_items| (self.completed_items, total_items))
    }

    pub(crate) fn mark_complete(&mut self) {
        let bytes = match self.work {
            FileOperationWorkProgress::Bytes { total_bytes, .. } => {
                Some((total_bytes, total_bytes))
            }
            FileOperationWorkProgress::Indeterminate
            | FileOperationWorkProgress::Completed { .. } => None,
        };
        self.work = FileOperationWorkProgress::Completed { bytes };
        if let Some(total_items) = self.total_items {
            self.completed_items = total_items;
        }
    }

    pub(crate) fn to_stored(self) -> StoredProgress {
        match self.fraction() {
            Some(fraction) => StoredProgress::with_fraction(fraction as f64),
            None => StoredProgress::pending(),
        }
    }

    pub(crate) fn update(&mut self, update: FileOperationProgressUpdate) {
        match update {
            FileOperationProgressUpdate::Bytes {
                completed_bytes,
                total_bytes,
                completed_items,
                total_items,
            } => {
                self.update_items(completed_items, total_items);
                if total_bytes == 0 {
                    return;
                }
                let completed_bytes = completed_bytes.min(total_bytes);
                self.work = match self.work {
                    FileOperationWorkProgress::Indeterminate => FileOperationWorkProgress::Bytes {
                        completed_bytes,
                        total_bytes,
                    },
                    FileOperationWorkProgress::Bytes {
                        completed_bytes: current_completed_bytes,
                        total_bytes: current_total_bytes,
                    } if current_total_bytes == total_bytes => FileOperationWorkProgress::Bytes {
                        completed_bytes: current_completed_bytes.max(completed_bytes),
                        total_bytes,
                    },
                    current => current,
                };
            }
            FileOperationProgressUpdate::IndeterminateItems { completed, total } => {
                self.update_items(completed, total);
            }
            FileOperationProgressUpdate::Indeterminate => {}
        }
    }

    fn update_items(&mut self, completed: usize, total: usize) {
        if total == 0 {
            return;
        }
        let completed = completed.min(total);
        match self.total_items {
            Some(current_total) if current_total == total => {
                self.completed_items = self.completed_items.max(completed);
            }
            Some(_) => {}
            None => {
                self.completed_items = completed;
                self.total_items = Some(total);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum FileOperationProgressUpdate {
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
        completed_items: usize,
        total_items: usize,
    },
    IndeterminateItems {
        completed: usize,
        total: usize,
    },
    Indeterminate,
}

/// 单个传入条目的传输快照:占位行的单一事实源,由任务 `transfer_progress`
/// 全量替换持有,视图渲染时从队列派生,不另建占位状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferEntrySnapshot {
    /// 最终落地路径(含重名序号,入队后不变),`parent()` 用于匹配目标目录。
    pub(crate) target: PathBuf,
    pub(crate) is_directory: bool,
    /// `None` 表示未开始或字节数未知(排队中/清单缺失),占位画空环。
    pub(crate) completed_bytes: Option<u64>,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) state: TransferEntrySnapshotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferEntrySnapshotState {
    Queued,
    Active,
    Completed,
}
