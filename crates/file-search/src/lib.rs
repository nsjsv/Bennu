mod config;
mod crawler;
mod daemon;
mod database;
mod error;
mod extractor;
mod fallback;
mod filesystem;
mod logging;
mod managed_search_index;
mod model;
mod path_encoding;
mod protocol;
mod runtime_identity;
mod search_path_store;
mod service_runtime;
mod writer;

pub use config::{
    SearchExcludeRules, SearchIndexConfig, SearchPathDecision, SearchPathPolicy,
    SearchPathPreferences, VersionedSearchPathPreferences,
};
pub use crawler::{IndexMaintenanceProgress, RebuildStats, SearchIndexer};
pub use daemon::SearchDaemonCore;
pub use database::{
    EntryStageProgress, FileSignature, IndexedEntryStageState, IndexedFile, KnownEntryState,
    SearchDatabase,
};
pub use error::{SearchError, SearchResult};
pub use extractor::{
    extract_content, extract_with_system_command, CommandSpec, ExtractionOutcome, ExtractionStatus,
};
pub use fallback::{
    search_directory_fallback, DirectoryFallbackCompletion, DirectoryFallbackLimits,
};
pub use model::{
    daemon_build_id, normalize_extension_tokens, normalize_size_range, parse_size_text,
    ExtractorCapability, IndexHealth, IndexPhase, IndexStatus, IndexedQueryAvailability,
    MatchSource, MimePattern, SearchCursor, SearchEntryTypeRule, SearchFileKind, SearchFilters,
    SearchHit, SearchMatchMode, SearchPathConfigurationPhase, SearchPathConfigurationStatus,
    SearchProviderFailure, SearchQuery, SearchResultBatch, SearchRootAvailability,
    SearchRootStatus, SearchScope, SearchServiceEvent, SearchServicePhase, SearchServiceRequest,
    SearchServiceStatus, SearchTextScope, SizeRange, TimeRange, MAX_EXTENSION_TOKEN_BYTES,
    MAX_QUERY_EXTENSIONS, PROTOCOL_VERSION, SIZE_UNIT_BYTES,
};
pub use protocol::{
    configure_path_preferences_via_socket, default_socket_path, path_configuration_via_socket,
    read_service_event, read_service_request, search_via_socket,
    search_via_socket_with_cancellation, serve_bound_search_socket, serve_search_socket,
    serve_search_socket_with_core, serve_search_socket_with_status, shutdown_connected_service,
    shutdown_via_socket, status_via_socket, version_via_socket, write_service_event,
    write_service_request, BoundSearchSocket, SearchSocketService,
};
pub use runtime_identity::{SearchRuntimeIdentity, SEARCH_RUNTIME_IDENTITY_ENV};
pub use service_runtime::SearchServiceRuntime;
pub use writer::IndexWriter;
