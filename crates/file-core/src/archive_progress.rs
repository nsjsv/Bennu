//! 压缩与解压共用的进度形状。哪种度量驱动进度条由产出方决定，执行器
//! 照着映射，不再按“总字节是不是 0”去猜。

/// 正在处理的单个条目（zip 解压、zip/tar.gz 压缩能拿到它的字节进度）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntryProgress {
    pub name: String,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveProgress {
    /// 按条目个数算（目录不计）。`bytes` 只用于文字。
    Entries {
        completed_entries: usize,
        total_entries: usize,
        bytes: Option<(u64, u64)>,
        current_entry: Option<ArchiveEntryProgress>,
    },
    /// 按字节算：7z 只报整体百分比；tar 流式解压按已读的压缩包字节算，
    /// 事先不知道条目总数。
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
        completed_entries: usize,
        total_entries: Option<usize>,
    },
}
