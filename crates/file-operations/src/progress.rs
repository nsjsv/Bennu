use std::path::PathBuf;

use bennu_state::StoredProgress;
use file_core::byte_progress::{active_byte_fraction, ACTIVE_PROGRESS_LIMIT};

/// 驱动进度条比例的度量。字节数和项数可以同时存在（文字都显示），
/// 但比例只由其中一种决定，由执行器发来的更新种类选定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOperationProgressMeasure {
    Unknown,
    Bytes,
    Items,
}

/// 正在搬运的单个文件。并行搬运时执行器只挑最大的那个发来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentFileProgress {
    pub name: String,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

impl CurrentFileProgress {
    pub fn fraction(&self) -> Option<f32> {
        active_byte_fraction(self.completed_bytes, self.total_bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOperationProgress {
    measure: FileOperationProgressMeasure,
    bytes: Option<(u64, u64)>,
    /// (已完成, 总数)；总数未知（tar 流式解压）时只有已完成数。
    items: Option<(usize, Option<usize>)>,
    current_file: Option<CurrentFileProgress>,
    completed: bool,
}

impl FileOperationProgress {
    pub fn pending() -> Self {
        Self {
            measure: FileOperationProgressMeasure::Unknown,
            bytes: None,
            items: None,
            current_file: None,
            completed: false,
        }
    }

    pub fn fraction(&self) -> Option<f32> {
        if self.completed {
            return Some(1.0);
        }
        match self.measure {
            FileOperationProgressMeasure::Unknown => None,
            FileOperationProgressMeasure::Bytes => self
                .bytes
                .and_then(|(completed, total)| active_byte_fraction(completed, total)),
            // 和字节比例同一个上限：最后一项写完到结算之间不显示满格。
            FileOperationProgressMeasure::Items => self.items.and_then(|(completed, total)| {
                total.map(|total| (completed as f32 / total as f32).min(ACTIVE_PROGRESS_LIMIT))
            }),
        }
    }

    pub fn bytes(&self) -> Option<(u64, u64)> {
        self.bytes
    }

    pub fn items(&self) -> Option<(usize, Option<usize>)> {
        self.items
    }

    pub fn current_file(&self) -> Option<&CurrentFileProgress> {
        self.current_file.as_ref()
    }

    pub fn mark_complete(&mut self) {
        self.completed = true;
        self.bytes = self.bytes.map(|(_, total)| (total, total));
        self.items = self
            .items
            .map(|(completed, total)| (total.unwrap_or(completed), total));
        self.current_file = None;
    }

    pub fn to_stored(&self) -> StoredProgress {
        match self.fraction() {
            Some(fraction) => StoredProgress::with_fraction(fraction as f64),
            None => StoredProgress::pending(),
        }
    }

    pub fn update(&mut self, update: FileOperationProgressUpdate) {
        match update {
            FileOperationProgressUpdate::Bytes {
                completed_bytes,
                total_bytes,
                completed_items,
                total_items,
            } => {
                self.update_items(completed_items, total_items);
                if self.update_bytes(completed_bytes, total_bytes) {
                    self.select_measure(FileOperationProgressMeasure::Bytes);
                }
            }
            FileOperationProgressUpdate::Transfer {
                completed_items,
                total_items,
                bytes,
                current_file,
            } => {
                if self.update_items(completed_items, Some(total_items)) {
                    self.select_measure(FileOperationProgressMeasure::Items);
                }
                if let Some((completed_bytes, total_bytes)) = bytes {
                    self.update_bytes(completed_bytes, total_bytes);
                }
                self.current_file = current_file;
            }
            FileOperationProgressUpdate::IndeterminateItems { completed, total } => {
                self.update_items(completed, Some(total));
            }
            FileOperationProgressUpdate::Indeterminate => {}
        }
    }

    /// 第一次拿到可信分母时选定度量，之后不换：同一任务的进度条不能
    /// 从按字节跳成按项数。
    fn select_measure(&mut self, measure: FileOperationProgressMeasure) {
        if self.measure == FileOperationProgressMeasure::Unknown {
            self.measure = measure;
        }
    }

    /// 只增不减；分母变了的更新整条忽略。返回分母是否可信（已采用）。
    fn update_bytes(&mut self, completed: u64, total: u64) -> bool {
        if total == 0 {
            return false;
        }
        let completed = completed.min(total);
        match self.bytes {
            Some((current, current_total)) if current_total == total => {
                self.bytes = Some((current.max(completed), total));
                true
            }
            Some(_) => false,
            None => {
                self.bytes = Some((completed, total));
                true
            }
        }
    }

    /// 同 `update_bytes`；`total == None` 表示总数未知，只累计已完成数，
    /// 不算可信分母。
    fn update_items(&mut self, completed: usize, total: Option<usize>) -> bool {
        if total == Some(0) {
            return false;
        }
        let completed = total.map_or(completed, |total| completed.min(total));
        match self.items {
            Some((current, current_total)) if current_total == total => {
                self.items = Some((current.max(completed), total));
                total.is_some()
            }
            Some(_) => false,
            None => {
                self.items = Some((completed, total));
                total.is_some()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum FileOperationProgressUpdate {
    /// 比例按字节算；项数只用于文字，总数未知时为 `None`。
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
        completed_items: usize,
        total_items: Option<usize>,
    },
    /// 比例按项数算；字节只用于文字，当前文件单独一条进度。
    Transfer {
        completed_items: usize,
        total_items: usize,
        bytes: Option<(u64, u64)>,
        current_file: Option<CurrentFileProgress>,
    },
    /// 只有文字计数，不出比例。
    IndeterminateItems {
        completed: usize,
        total: usize,
    },
    Indeterminate,
}

/// 单个传入条目的传输快照:占位行的单一事实源,由任务 `transfer_progress`
/// 全量替换持有,视图渲染时从队列派生,不另建占位状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferEntrySnapshot {
    /// 最终落地路径(含重名序号,入队后不变),`parent()` 用于匹配目标目录。
    pub target: PathBuf,
    pub is_directory: bool,
    /// `None` 表示未开始或字节数未知(排队中/清单缺失),占位画空环。
    pub completed_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub state: TransferEntrySnapshotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferEntrySnapshotState {
    Queued,
    Active,
    Completed,
}
