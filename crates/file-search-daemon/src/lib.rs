//! 搜索守护进程：索引数据库、爬虫、内容提取、写入线程和 socket 服务端。
//! 模型、线协议帧、目录遍历、文本匹配等两端共用的约定在 file-search，这里按原模块路径引用，
//! 避免回退搜索与索引查询各有一份实现。

mod crawler;
mod daemon;
mod database;
mod extractor;
#[cfg(test)]
mod filter_semantics_tests;
mod logging;
mod managed_search_index;
mod protocol;
#[cfg(test)]
mod search_parity_tests;
mod service_runtime;
mod writer;

use file_search::{
    config, error, filesystem, model, path_encoding, search_path_store, text_matching,
};
use file_search::{
    SearchIndexConfig, SearchPathDecision, SearchPathPolicy, SearchPathPreferences,
    VersionedSearchPathPreferences,
};

pub use crawler::{IndexMaintenanceProgress, RebuildStats, SearchIndexer};
pub use daemon::SearchDaemonCore;
pub use database::{
    EntryStageProgress, FileSignature, IndexedEntryStageState, IndexedFile, KnownEntryState,
    SearchDatabase,
};
pub use extractor::{
    extract_content, extract_with_system_command, CommandSpec, ExtractionOutcome, ExtractionStatus,
};
pub use protocol::{
    serve_bound_search_socket, serve_search_socket, serve_search_socket_with_status,
    BoundSearchSocket, SearchSocketService,
};
pub use service_runtime::SearchServiceRuntime;
pub use writer::IndexWriter;
