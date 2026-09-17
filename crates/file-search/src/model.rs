use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{SearchPathPreferences, VersionedSearchPathPreferences};
use crate::error::{SearchError, SearchResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchQuery {
    pub query_id: u64,
    pub terms: String,
    pub text_scope: SearchTextScope,
    // 旧客户端不带该字段：serde default 兜底为普通搜索，保持协议双向兼容。
    #[serde(default)]
    pub match_mode: SearchMatchMode,
    pub scope: SearchScope,
    pub recursive: bool,
    pub filters: SearchFilters,
    pub limit: usize,
    pub cursor: Option<SearchCursor>,
}

impl SearchQuery {
    pub fn global(query_id: u64, terms: impl Into<String>) -> Self {
        Self {
            query_id,
            terms: terms.into(),
            text_scope: SearchTextScope::NameAndContent,
            match_mode: SearchMatchMode::Plain,
            scope: SearchScope::Global,
            recursive: true,
            filters: SearchFilters::default(),
            limit: 50,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMatchMode {
    #[default]
    Plain,
    Regex,
}

impl SearchMatchMode {
    /// 正则匹配名称的唯一编译入口：索引路径、fallback 路径与 UI 预校验共用同一语义。
    pub fn name_regex(self, terms: &str) -> SearchResult<regex::Regex> {
        regex::RegexBuilder::new(terms)
            .case_insensitive(true)
            .build()
            .map_err(|error| SearchError::InvalidQuery(format!("invalid regex: {error}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchTextScope {
    NameAndContent,
    NameOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchScope {
    Global,
    Directory(#[serde(with = "crate::path_encoding::serde_path")] PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchFilters {
    pub entry_type_rules: Vec<SearchEntryTypeRule>,
    pub modified: Option<TimeRange>,
    pub accessed: Option<TimeRange>,
    pub created: Option<TimeRange>,
    // 旧客户端不带该字段：serde default 兜底为空，保持协议双向兼容。
    #[serde(default)]
    pub extensions: Vec<String>,
    // 旧客户端不带该字段：serde default 兜底为不过滤大小，保持协议双向兼容。
    #[serde(default)]
    pub size: Option<SizeRange>,
}

/// 大小解析的进制：与资源管理器档位一致，KB = 1024 B。
pub const SIZE_UNIT_BYTES: u64 = 1024;

/// 文件大小区间，语义统一为 `[min_bytes, max_bytes)`（上界排他）。
/// 「空 (0 B)」档位即 `{ min: 0, max: Some(1) }`，不引入任何特判。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeRange {
    /// 含下界。
    pub min_bytes: u64,
    /// 排他上界；None = 不限。
    pub max_bytes: Option<u64>,
}

/// 单个大小文本的唯一解析入口：UI 预检与查询构造共用同一语义。
/// 规则：正十进制数（允许小数）+ 可选单位 `B/KB/MB/GB`（大小写不敏感、
/// 数字与单位间允许空白）；省略单位按字节；负数、非法字符、未知单位、
/// 溢出都报错。
pub fn parse_size_text(input: &str) -> Result<u64, String> {
    let text = input.trim();
    let Some(digits_end) = text
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit() && *character != '.')
        .map(|(index, _)| index)
    else {
        return parse_size_bytes(text, 1);
    };
    let (number_text, unit_text) = text.split_at(digits_end);
    let multiplier = size_unit_multiplier(unit_text.trim())?;
    parse_size_bytes(number_text.trim(), multiplier)
}

/// 自定义大小范围的唯一归一化入口：两侧全空 = 不限；单侧空按缺省
/// （min 缺省 0，max 缺省 None）；最小值大于最大值非法。
pub fn normalize_size_range(min_text: &str, max_text: &str) -> Result<Option<SizeRange>, String> {
    let min_text = min_text.trim();
    let max_text = max_text.trim();
    if min_text.is_empty() && max_text.is_empty() {
        return Ok(None);
    }
    let min_bytes = if min_text.is_empty() {
        0
    } else {
        parse_size_text(min_text)?
    };
    let max_bytes = if max_text.is_empty() {
        None
    } else {
        Some(parse_size_text(max_text)?)
    };
    if max_bytes.is_some_and(|max_bytes| min_bytes > max_bytes) {
        return Err(format!("Invalid size range: {min_text} - {max_text}"));
    }
    Ok(Some(SizeRange {
        min_bytes,
        max_bytes,
    }))
}

fn size_unit_multiplier(unit: &str) -> Result<u64, String> {
    match unit.to_ascii_lowercase().as_str() {
        "" | "b" => Ok(1),
        "kb" => Ok(SIZE_UNIT_BYTES),
        "mb" => Ok(SIZE_UNIT_BYTES * SIZE_UNIT_BYTES),
        "gb" => Ok(SIZE_UNIT_BYTES * SIZE_UNIT_BYTES * SIZE_UNIT_BYTES),
        other => Err(format!("Unknown size unit: {other}")),
    }
}

fn parse_size_bytes(number_text: &str, multiplier: u64) -> Result<u64, String> {
    let invalid = || format!("Invalid size value: {number_text}");
    let overflow = || format!("Size value is too large: {number_text}");
    let (integer_text, fractional_text) = match number_text.split_once('.') {
        Some((integer_text, fractional_text)) => (integer_text, fractional_text),
        None => (number_text, ""),
    };
    // "1." 与 ".5" 这类残缺小数视为误输入，而不是静默补零。
    if integer_text.is_empty() || (number_text.contains('.') && fractional_text.is_empty()) {
        return Err(invalid());
    }
    let integer: u64 = integer_text.parse().map_err(|_| invalid())?;
    let bytes = u128::from(integer.checked_mul(multiplier).ok_or_else(overflow)?);
    if fractional_text.is_empty() {
        return u64::try_from(bytes).map_err(|_| overflow());
    }
    // 小数部分用 u128 中转，避免先乘单位再除进位时的中间溢出；结果向下取整到字节。
    // 裸乘法在超长小数（如 38 位）+ 大单位时会溢出 u128：debug panic、release
    // 静默回绕成错误字节值，必须与整数路径一样归类为 overflow 拒绝。
    let fractional: u128 = fractional_text.parse().map_err(|_| invalid())?;
    let scale = 10u128
        .checked_pow(fractional_text.len() as u32)
        .ok_or_else(overflow)?;
    let fractional_bytes = fractional
        .checked_mul(u128::from(multiplier))
        .ok_or_else(overflow)?
        / scale;
    u64::try_from(bytes + fractional_bytes).map_err(|_| overflow())
}

/// 单个扩展名 token 的最大字节数；超长输入视为误输入而非合法后缀。
pub const MAX_EXTENSION_TOKEN_BYTES: usize = 63;

/// 单次查询允许的自定义扩展名数量上限，与 entry_type_rules 的量级约束一致。
pub const MAX_QUERY_EXTENSIONS: usize = 16;

/// 扩展名 token 的唯一字符白名单：字母数字与 `. - +`。
/// 拒绝空白与 `%` `_` 等通配符，使 SQL 端 LIKE 拼接无需转义。
pub(crate) fn extension_token_is_valid(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_EXTENSION_TOKEN_BYTES
        && token
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// 自定义后缀的唯一解析入口：UI 预检与两条执行路径共用同一语义。
/// 容错规则：逗号/空白分隔、前后点号可省略、大小写不敏感、按序去重。
pub fn normalize_extension_tokens(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    for raw in input.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = raw.trim_matches('.').to_lowercase();
        if token.is_empty() {
            continue;
        }
        if !extension_token_is_valid(&token) {
            return Err(format!("Invalid file extension: {raw}"));
        }
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    if tokens.len() > MAX_QUERY_EXTENSIONS {
        return Err(format!(
            "Too many file extensions (limit is {MAX_QUERY_EXTENSIONS})"
        ));
    }
    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchEntryTypeRule {
    Kind(SearchFileKind),
    Mime(MimePattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MimePattern {
    Exact(String),
    Prefix(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchCursor {
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResultBatch {
    pub query_id: u64,
    pub hits: Vec<SearchHit>,
    pub next_cursor: Option<SearchCursor>,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(with = "crate::path_encoding::serde_path")]
    pub path: PathBuf,
    pub display_name: String,
    pub kind: SearchFileKind,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub accessed_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub rank: f64,
    pub snippet: Option<String>,
    pub match_source: MatchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchFileKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl SearchFileKind {
    pub fn as_storage_value(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }

    pub fn from_storage_value(value: &str) -> Self {
        match value {
            "directory" => Self::Directory,
            "file" => Self::File,
            "symlink" => Self::Symlink,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchSource {
    Name,
    Content,
    Metadata,
}

/// 索引阶段、查询可见计数与维护健康度必须正交，避免瞬时状态互相覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexStatus {
    pub phase: IndexPhase,
    pub visible_indexed_files: u64,
    pub health: IndexHealth,
    pub capabilities: Vec<ExtractorCapability>,
    pub path_configuration: SearchPathConfigurationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPathConfigurationStatus {
    pub desired_revision: u64,
    pub effective_revision: u64,
    pub effective_preferences: SearchPathPreferences,
    pub phase: SearchPathConfigurationPhase,
    pub roots: Vec<SearchRootStatus>,
}

impl Default for SearchPathConfigurationStatus {
    fn default() -> Self {
        Self {
            desired_revision: 0,
            effective_revision: 0,
            effective_preferences: SearchPathPreferences::default(),
            phase: SearchPathConfigurationPhase::Ready,
            roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchPathConfigurationPhase {
    Ready,
    Applying,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRootStatus {
    #[serde(with = "crate::path_encoding::serde_path")]
    pub path: PathBuf,
    pub availability: SearchRootAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchRootAvailability {
    Available,
    Unavailable { message: String },
    MountChanged { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexPhase {
    Starting,
    Checking {
        checked_entries: u64,
        changed_entries: u64,
    },
    Crawling {
        scanned_entries: u64,
        #[serde(with = "crate::path_encoding::serde_path")]
        current_scope: PathBuf,
    },
    Applying {
        pending_mutations: u64,
    },
    Complete,
    Failed {
        message: String,
    },
}

/// 持续监听与维护链路的健康度；索引任务自身失败由 `IndexPhase::Failed` 承载。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexHealth {
    Healthy,
    Degraded { message: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractorCapability {
    pub extension: String,
    pub tool: String,
    pub available: bool,
}

/// 服务阶段、查询可用性和索引进度必须正交，避免客户端用索引进度猜测端点是否可查询。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchServiceStatus {
    pub phase: SearchServicePhase,
    pub query_availability: IndexedQueryAvailability,
    pub index_status: Option<IndexStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchServicePhase {
    Starting,
    Ready,
    Degraded { message: String },
    Failed { message: String },
    ShuttingDown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexedQueryAvailability {
    Unavailable { message: String },
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SearchProviderFailure {
    #[error("search provider unavailable: {message}")]
    Unavailable { message: String },
    #[error("invalid search query: {message}")]
    InvalidQuery { message: String },
    #[error("search provider failed: {message}")]
    Fatal { message: String },
}

/// 协议 payload 含义变化时提升版本，让新客户端能退休仍在运行的旧 daemon。
pub const PROTOCOL_VERSION: u32 = 12;

/// app 更新后用构建标识识别遗留 daemon；本任务保留现有包版本策略。
pub fn daemon_build_id() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchServiceRequest {
    Status,
    GetPathConfiguration,
    ConfigurePathPreferences {
        expected_revision: u64,
        preferences: SearchPathPreferences,
    },
    Search(SearchQuery),
    Cancel {
        query_id: u64,
    },
    /// 查询 daemon 的协议版本和构建标识。
    Version,
    /// 请求 daemon 完成待处理写入后退出，用于退休不兼容的旧进程。
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SearchServiceEvent {
    Status(SearchServiceStatus),
    PathConfiguration {
        configuration: VersionedSearchPathPreferences,
        status: SearchPathConfigurationStatus,
    },
    PathConfigurationFailed {
        failure: SearchProviderFailure,
        status: Option<SearchPathConfigurationStatus>,
    },
    Results(SearchResultBatch),
    SearchFailed {
        query_id: u64,
        failure: SearchProviderFailure,
    },
    Cancelled {
        query_id: u64,
    },
    /// `Version` 的响应同时携带协议版本和 daemon 构建标识。
    Version {
        protocol: u32,
        build: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_extension_tokens, normalize_size_range, parse_size_text, IndexHealth, IndexPhase,
        IndexStatus, IndexedQueryAvailability, SearchError, SearchMatchMode, SearchQuery,
        SearchServiceEvent, SearchServicePhase, SearchServiceStatus, MAX_QUERY_EXTENSIONS,
        SIZE_UNIT_BYTES,
    };

    #[test]
    fn service_phase_and_query_availability_round_trip_independently() {
        let event = SearchServiceEvent::Status(SearchServiceStatus {
            phase: SearchServicePhase::Degraded {
                message: "watcher unavailable".to_owned(),
            },
            query_availability: IndexedQueryAvailability::Available,
            index_status: Some(IndexStatus {
                phase: IndexPhase::Checking {
                    checked_entries: 128,
                    changed_entries: 3,
                },
                visible_indexed_files: 64,
                health: IndexHealth::Degraded {
                    message: "watcher unavailable".to_owned(),
                },
                capabilities: Vec::new(),
                path_configuration: Default::default(),
            }),
        });

        let encoded = serde_json::to_vec(&event).unwrap();
        let decoded: SearchServiceEvent = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, event);
    }
    #[test]
    fn query_without_match_mode_field_defaults_to_plain_search() {
        // 旧客户端查询不含 match_mode：serde default 保证新 daemon 按普通搜索处理。
        let legacy = serde_json::json!({
            "query_id": 7,
            "terms": "needle",
            "text_scope": "NameOnly",
            "scope": "Global",
            "recursive": true,
            "filters": { "entry_type_rules": [] },
            "limit": 50,
            "cursor": null,
        });
        let query: SearchQuery = serde_json::from_value(legacy).unwrap();
        assert_eq!(query.match_mode, SearchMatchMode::Plain);
    }

    #[test]
    fn unknown_wire_fields_are_rejected_instead_of_silently_dropped() {
        // 版本错配必须响亮失败：旧 daemon 收到新客户端字段时拒绝整条查询，
        // 而不是静默丢弃过滤条件后返回错误结果。
        let mut query = serde_json::json!({
            "query_id": 7,
            "terms": "needle",
            "text_scope": "NameOnly",
            "scope": "Global",
            "recursive": true,
            "filters": { "entry_type_rules": [], "future_filter": 1 },
            "limit": 50,
            "cursor": null,
        });
        assert!(serde_json::from_value::<SearchQuery>(query.clone()).is_err());

        query["filters"] = serde_json::json!({ "entry_type_rules": [] });
        query["future_query_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<SearchQuery>(query).is_err());
    }

    #[test]
    fn invalid_name_regex_is_rejected_as_invalid_query() {
        assert!(matches!(
            SearchMatchMode::Regex.name_regex("foo("),
            Err(SearchError::InvalidQuery(_))
        ));
    }

    #[test]
    fn query_without_extensions_field_defaults_to_empty_filter() {
        // 旧客户端查询不含 extensions：serde default 保证新 daemon 按无后缀过滤处理。
        let legacy = serde_json::json!({
            "query_id": 7,
            "terms": "needle",
            "text_scope": "NameOnly",
            "scope": "Global",
            "recursive": true,
            "filters": { "entry_type_rules": [] },
            "limit": 50,
            "cursor": null,
        });
        let query: SearchQuery = serde_json::from_value(legacy).unwrap();

        assert!(query.filters.extensions.is_empty());
    }

    #[test]
    fn normalize_extension_tokens_tolerates_separators_dots_and_case() {
        assert_eq!(
            normalize_extension_tokens(".PDF, docx  tar.gz,, .md").unwrap(),
            vec![
                "pdf".to_owned(),
                "docx".to_owned(),
                "tar.gz".to_owned(),
                "md".to_owned()
            ]
        );
        assert!(normalize_extension_tokens("  ,,, .. ").unwrap().is_empty());
    }

    #[test]
    fn normalize_extension_tokens_rejects_invalid_and_excess_tokens() {
        assert!(normalize_extension_tokens("pd*f").is_err());
        assert!(normalize_extension_tokens("pdf %_").is_err());
        let overflow = (0..=MAX_QUERY_EXTENSIONS)
            .map(|index| format!("x{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(normalize_extension_tokens(&overflow).is_err());
    }

    #[test]
    fn parse_size_text_accepts_units_whitespace_and_fractions() {
        let kb = SIZE_UNIT_BYTES;
        let mb = kb * kb;
        let gb = mb * kb;
        assert_eq!(parse_size_text("10MB").unwrap(), 10 * mb);
        assert_eq!(parse_size_text("500kb").unwrap(), 500 * kb);
        assert_eq!(parse_size_text("2 GB").unwrap(), 2 * gb);
        assert_eq!(parse_size_text("1.5GB").unwrap(), gb + gb / 2);
        // 省略单位按字节；单位与 b 等价。
        assert_eq!(parse_size_text("42").unwrap(), 42);
        assert_eq!(parse_size_text("42 B").unwrap(), 42);
        assert_eq!(parse_size_text("  7b ").unwrap(), 7);
        // 小数向下取整到字节。
        assert_eq!(parse_size_text("0.5KB").unwrap(), kb / 2);
        assert_eq!(parse_size_text("1.25 KB").unwrap(), kb + kb / 4);
    }

    #[test]
    fn parse_size_text_rejects_negative_unknown_and_malformed_values() {
        for invalid in [
            "-5", "-1 B", "5 TB", "5kib", "1M2", "abc", "4.", ".5", "1.2.3", "", "   ",
        ] {
            assert!(parse_size_text(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn parse_size_text_rejects_values_beyond_u64_bytes() {
        assert_eq!(parse_size_text("18446744073709551615").unwrap(), u64::MAX,);
        assert!(parse_size_text("18446744073709551616").is_err());
        assert!(parse_size_text("17179869184 GB").is_err());
    }

    #[test]
    fn parse_size_text_rejects_fractions_whose_unit_scaling_overflows() {
        // 38 位小数本身在 u128 内可表示，但乘上 GB 进制会溢出：
        // 必须报错，而不是 panic（debug）或静默回绕成错误字节值（release）。
        let long_fraction = format!("0.{}GB", "9".repeat(38));
        assert!(parse_size_text(&long_fraction).is_err());

        // 不会溢出的超长小数仍正常解析：向下取整到字节。
        assert_eq!(
            parse_size_text(&format!("0.{}1", "0".repeat(37))).unwrap(),
            0
        );
    }

    #[test]
    fn normalize_size_range_defaults_missing_sides_and_rejects_inversion() {
        // 两侧全空 = 不限。
        assert_eq!(normalize_size_range("", "  ").unwrap(), None);

        // 单侧空按缺省：min 缺省 0，max 缺省不限。
        assert_eq!(
            normalize_size_range("", "10KB").unwrap(),
            Some(super::SizeRange {
                min_bytes: 0,
                max_bytes: Some(10 * SIZE_UNIT_BYTES),
            })
        );
        assert_eq!(
            normalize_size_range("10KB", "").unwrap(),
            Some(super::SizeRange {
                min_bytes: 10 * SIZE_UNIT_BYTES,
                max_bytes: None,
            })
        );

        // 最小值大于最大值非法。
        assert!(normalize_size_range("2MB", "1MB").is_err());
        // 相等仍合法（空区间在执行路径按 [min, max) 恒 false 处理，无特判）。
        assert_eq!(
            normalize_size_range("1MB", "1MB").unwrap(),
            Some(super::SizeRange {
                min_bytes: SIZE_UNIT_BYTES * SIZE_UNIT_BYTES,
                max_bytes: Some(SIZE_UNIT_BYTES * SIZE_UNIT_BYTES),
            })
        );
    }

    #[test]
    fn query_without_size_field_defaults_to_no_size_filter() {
        // 旧客户端查询不含 size：serde default 保证新 daemon 按不过滤大小处理。
        let legacy = serde_json::json!({
            "query_id": 7,
            "terms": "needle",
            "text_scope": "NameOnly",
            "scope": "Global",
            "recursive": true,
            "filters": { "entry_type_rules": [] },
            "limit": 50,
            "cursor": null,
        });
        let query: SearchQuery = serde_json::from_value(legacy).unwrap();

        assert!(query.filters.size.is_none());
    }
}
