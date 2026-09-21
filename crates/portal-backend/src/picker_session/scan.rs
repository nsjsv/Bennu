//! 目录扫描与内容加载载荷：扫描结果按目标路径回填根列表或展开节点。
//! 独立成文件，控制 picker_session/mod.rs 的行数。

use std::path::PathBuf;

use file_core::entry::DirectoryEntry;
use file_core::scan::{scan_directory, ScanOptions};

/// 目录内容加载状态（根列表与展开节点共用）。
pub(crate) enum DirectoryListing {
    Pending,
    Ready(Vec<DirectoryEntry>),
    Failed(String),
}

/// 扫描完成载荷：携带扫描目标，供会话把结果路由回根列表或展开节点。
#[derive(Debug, Clone)]
pub(crate) struct DirectoryScanResult {
    pub(crate) directory: PathBuf,
    pub(crate) outcome: Result<DirectoryScanOutcome, String>,
}

/// 扫描成功内容（避免 session 依赖 file-core 错误类型细节）。
#[derive(Debug, Clone)]
pub(crate) struct DirectoryScanOutcome {
    pub(crate) entries: Vec<DirectoryEntry>,
}

/// 执行目录扫描（上层在 iced Task 里调用）。
pub(crate) async fn scan_listing(directory: PathBuf) -> Result<DirectoryScanOutcome, String> {
    let scan = scan_directory(&directory, ScanOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(DirectoryScanOutcome {
        entries: scan.entries,
    })
}
