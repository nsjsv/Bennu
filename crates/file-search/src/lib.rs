//! 搜索的客户端与两端共享的约定：模型、线协议、目录回退搜索、路径配置与运行身份。
//! 数据库、爬虫、内容提取、socket 服务端和 `bennu-searchd` 在 file-search-daemon，主程序不链接它们。
//!
//! 目录遍历（`filesystem`）、文本匹配（`text_matching`）、路径编码、过滤语义、配置与错误类型以
//! `pub mod` 公开给守护进程：回退搜索与索引查询必须按同一套规则判定，守护进程要直接引用这份实现，
//! 而不是复制一份。

pub mod config;
pub mod error;
mod fallback;
pub mod filesystem;
pub mod filter_semantics;
pub mod model;
pub mod path_encoding;
pub mod protocol;
mod runtime_identity;
pub mod search_path_store;
pub mod text_matching;

pub use config::{
    SearchExcludeRules, SearchIndexConfig, SearchPathDecision, SearchPathPolicy,
    SearchPathPreferences, VersionedSearchPathPreferences,
};
pub use error::{SearchError, SearchResult};
pub use fallback::{
    search_directory_fallback, DirectoryFallbackCompletion, DirectoryFallbackLimits,
};
pub use model::{
    daemon_build_id, normalize_extension_tokens, normalize_size_range, parse_size_text,
    ExtractorCapability, FilterInputError, IndexHealth, IndexPhase, IndexStatus,
    IndexedQueryAvailability, MatchSource, MimePattern, SearchCursor, SearchEntryTypeRule,
    SearchFileKind, SearchFilters, SearchHit, SearchMatchMode, SearchPathConfigurationPhase,
    SearchPathConfigurationStatus, SearchProviderFailure, SearchQuery, SearchResultBatch,
    SearchRootAvailability, SearchRootStatus, SearchScope, SearchServiceEvent, SearchServicePhase,
    SearchServiceRequest, SearchServiceStatus, SearchTextScope, SizeRange, TimeRange,
    MAX_EXTENSION_TOKEN_BYTES, MAX_QUERY_EXTENSIONS, PROTOCOL_VERSION, SIZE_UNIT_BYTES,
};
pub use protocol::{
    configure_path_preferences_via_socket, default_socket_path, path_configuration_via_socket,
    read_service_event, read_service_request, search_via_socket,
    search_via_socket_with_cancellation, shutdown_connected_service, shutdown_via_socket,
    status_via_socket, version_via_socket, write_service_event, write_service_request,
};
pub use runtime_identity::{SearchRuntimeIdentity, SEARCH_RUNTIME_IDENTITY_ENV};
pub use search_path_store::{load_search_path_preferences, search_path_file_in};
