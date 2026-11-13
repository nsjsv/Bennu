//! 过滤条件（类型、MIME、后缀、大小、时间）的唯一语义：本地回退按条目事实判断，
//! 索引查询拼成同义的 SQL 条件。两份实现放在一起，共用 file-search-daemon 里
//! `filter_semantics_tests` 的测试矩阵（需要真实数据库），改一边必须同时改另一边。
//!
//! 类型条件要按类型库解释（别名、“某类型及其子类”），所以先 `resolve` 成具体的类型名，
//! 判断和 SQL 都只看解释好的结果。

use rusqlite::types::Value;

use crate::mime::MimeDatabase;
use crate::model::{
    MimePattern, SearchEntryTypeRule, SearchFileKind, SearchFilters, SizeRange, TimeRange,
};

/// 判断过滤条件所需的条目事实，全部来自同一次观察。`search_name` 是规范化后的名称。
pub struct EntryFacts<'a> {
    pub search_name: &'a str,
    pub kind: SearchFileKind,
    pub mime_type: Option<&'a str>,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub accessed_ms: Option<i64>,
    pub created_ms: Option<i64>,
}

/// 按类型库解释好的过滤条件。类型条件之间是“或”：任一类别、任一具体类型、任一前缀。
pub struct ResolvedFilters<'a> {
    filters: &'a SearchFilters,
    kinds: Vec<SearchFileKind>,
    mime_types: Vec<String>,
    mime_prefixes: Vec<&'a str>,
}

impl SearchFilters {
    /// 精确类型去别名，“某类型及其子类”展开成库里的全部子类。
    pub fn resolve(&self, types: &MimeDatabase) -> ResolvedFilters<'_> {
        let mut resolved = ResolvedFilters {
            filters: self,
            kinds: Vec::new(),
            mime_types: Vec::new(),
            mime_prefixes: Vec::new(),
        };
        for rule in &self.entry_type_rules {
            match rule {
                SearchEntryTypeRule::Kind(kind) => resolved.kinds.push(*kind),
                SearchEntryTypeRule::Mime(MimePattern::Exact(mime)) => {
                    resolved.mime_types.push(types.canonical(mime));
                }
                SearchEntryTypeRule::Mime(MimePattern::Inheriting(base)) => {
                    resolved
                        .mime_types
                        .extend(types.inheriting_types(base).iter().cloned());
                }
                SearchEntryTypeRule::Mime(MimePattern::Prefix(prefix)) => {
                    resolved.mime_prefixes.push(prefix);
                }
            }
        }
        resolved.mime_types.sort();
        resolved.mime_types.dedup();
        resolved
    }
}

impl ResolvedFilters<'_> {
    fn has_type_rules(&self) -> bool {
        !self.filters.entry_type_rules.is_empty()
    }

    pub fn matches(&self, entry: &EntryFacts<'_>) -> bool {
        (!self.has_type_rules() || self.type_matches(entry))
            && extension_matches(&self.filters.extensions, entry)
            && size_matches(self.filters.size, entry)
            && time_matches(self.filters.modified, entry.modified_ms)
            && time_matches(self.filters.accessed, entry.accessed_ms)
            && time_matches(self.filters.created, entry.created_ms)
    }

    fn type_matches(&self, entry: &EntryFacts<'_>) -> bool {
        self.kinds.contains(&entry.kind)
            || entry.mime_type.is_some_and(|mime_type| {
                self.mime_types
                    .binary_search_by(|candidate| candidate.as_str().cmp(mime_type))
                    .is_ok()
                    || self
                        .mime_prefixes
                        .iter()
                        .any(|prefix| mime_type.starts_with(prefix))
            })
    }

    /// 与 `matches` 同义的 SQL 条件（以 ` AND …` 追加），列取自 `files` 的别名 `f`。
    pub fn append_sql(&self, sql: &mut String, values: &mut Vec<Value>) {
        if self.has_type_rules() {
            self.append_type_sql(sql, values);
        }
        append_extensions_sql(sql, values, &self.filters.extensions);
        append_size_sql(sql, values, self.filters.size);
        append_time_sql(sql, values, "f.modified_ms", self.filters.modified);
        append_time_sql(sql, values, "f.accessed_ms", self.filters.accessed);
        append_time_sql(sql, values, "f.created_ms", self.filters.created);
    }

    fn append_type_sql(&self, sql: &mut String, values: &mut Vec<Value>) {
        let mut alternatives = Vec::new();
        if !self.kinds.is_empty() {
            alternatives.push(format!("f.kind IN ({})", placeholders(self.kinds.len())));
            values.extend(
                self.kinds
                    .iter()
                    .map(|kind| Value::Text(kind.as_storage_value().to_owned())),
            );
        }
        if !self.mime_types.is_empty() {
            alternatives.push(format!(
                "f.mime_type IN ({})",
                placeholders(self.mime_types.len())
            ));
            values.extend(self.mime_types.iter().cloned().map(Value::Text));
        }
        // 与 `starts_with` 一样区分大小写；不用 LIKE（只折叠 ASCII，且要转义通配符）。
        for prefix in &self.mime_prefixes {
            alternatives.push("substr(f.mime_type, 1, ?) = ?".to_owned());
            values.push(Value::Integer(prefix.chars().count() as i64));
            values.push(Value::Text((*prefix).to_owned()));
        }
        // 每条规则至少解释出一项（展开时总包含基类型本身），括号里不会是空的。
        sql.push_str(" AND (");
        sql.push_str(&alternatives.join(" OR "));
        sql.push(')');
    }
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

fn directory_storage_value() -> Value {
    Value::Text(SearchFileKind::Directory.as_storage_value().to_owned())
}

/// 后缀 token 只含字母数字与 `. - +`（解析时校验），没有 GLOB 通配符；GLOB 区分大小写，
/// 两边都已规范化，与 `ends_with` 同义。
fn append_extensions_sql(sql: &mut String, values: &mut Vec<Value>, extensions: &[String]) {
    if extensions.is_empty() {
        return;
    }
    sql.push_str(" AND (f.kind <> ? AND (");
    values.push(directory_storage_value());
    for (index, extension) in extensions.iter().enumerate() {
        if index > 0 {
            sql.push_str(" OR ");
        }
        sql.push_str("f.search_name GLOB ?");
        values.push(Value::Text(format!("*.{extension}")));
    }
    sql.push_str("))");
}

fn append_size_sql(sql: &mut String, values: &mut Vec<Value>, size: Option<SizeRange>) {
    let Some(size) = size else {
        return;
    };
    sql.push_str(" AND (f.kind = ? OR (f.size >= ?");
    values.push(directory_storage_value());
    values.push(Value::Integer(storage_size_bound(size.min_bytes)));
    if let Some(max_bytes) = size.max_bytes {
        sql.push_str(" AND f.size < ?");
        values.push(Value::Integer(storage_size_bound(max_bytes)));
    }
    sql.push_str("))");
}

/// size 列是 SQLite 整数：超过 i64 的边界饱和为 i64::MAX，该边界在真实文件尺寸之外，
/// 不改变过滤语义。
fn storage_size_bound(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

/// 列为 NULL 时比较结果为 NULL（不匹配），与"没有该时间不匹配"同义。
fn append_time_sql(
    sql: &mut String,
    values: &mut Vec<Value>,
    column: &str,
    range: Option<TimeRange>,
) {
    let Some(range) = range else {
        return;
    };
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push_str(" >= ? AND ");
    sql.push_str(column);
    sql.push_str(" <= ?");
    values.push(Value::Integer(range.start_ms));
    values.push(Value::Integer(range.end_ms));
}

/// 后缀只作用于文件类条目：规范化名称以 `.` + 规范化后缀结尾，多段后缀（`tar.gz`）同样成立；
/// 目录即使名称以点后缀结尾也不匹配。
fn extension_matches(extensions: &[String], entry: &EntryFacts<'_>) -> bool {
    extensions.is_empty()
        || (entry.kind != SearchFileKind::Directory
            && extensions.iter().any(|token| {
                entry
                    .search_name
                    .strip_suffix(token.as_str())
                    .is_some_and(|stem| stem.ends_with('.'))
            }))
}

/// 大小只作用于文件类条目：目录永不因大小被排除；文件按 `[min, max)` 判定。
fn size_matches(range: Option<SizeRange>, entry: &EntryFacts<'_>) -> bool {
    range.is_none_or(|range| {
        entry.kind == SearchFileKind::Directory
            || (entry.size >= range.min_bytes && range.max_bytes.is_none_or(|max| entry.size < max))
    })
}

/// 时间按闭区间判定；条目没有该时间时不匹配。
fn time_matches(range: Option<TimeRange>, value: Option<i64>) -> bool {
    range.is_none_or(|range| {
        value.is_some_and(|value| value >= range.start_ms && value <= range.end_ms)
    })
}
