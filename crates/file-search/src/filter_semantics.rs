//! 过滤条件（类型、MIME、后缀、大小、时间）的唯一语义：本地回退按条目事实判断，
//! 索引查询拼成同义的 SQL 条件。两份实现放在一起，共用 file-search-daemon 里
//! `filter_semantics_tests` 的测试矩阵（需要真实数据库），改一边必须同时改另一边。

use rusqlite::types::Value;

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

impl SearchFilters {
    pub fn matches(&self, entry: &EntryFacts<'_>) -> bool {
        (self.entry_type_rules.is_empty()
            || self
                .entry_type_rules
                .iter()
                .any(|rule| entry_type_rule_matches(rule, entry)))
            && extension_matches(&self.extensions, entry)
            && size_matches(self.size, entry)
            && time_matches(self.modified, entry.modified_ms)
            && time_matches(self.accessed, entry.accessed_ms)
            && time_matches(self.created, entry.created_ms)
    }

    /// 与 `matches` 同义的 SQL 条件（以 ` AND …` 追加），列取自 `files` 的别名 `f`。
    pub fn append_sql(&self, sql: &mut String, values: &mut Vec<Value>) {
        append_entry_type_rules_sql(sql, values, &self.entry_type_rules);
        append_extensions_sql(sql, values, &self.extensions);
        append_size_sql(sql, values, self.size);
        append_time_sql(sql, values, "f.modified_ms", self.modified);
        append_time_sql(sql, values, "f.accessed_ms", self.accessed);
        append_time_sql(sql, values, "f.created_ms", self.created);
    }
}

fn directory_storage_value() -> Value {
    Value::Text(SearchFileKind::Directory.as_storage_value().to_owned())
}

fn append_entry_type_rules_sql(
    sql: &mut String,
    values: &mut Vec<Value>,
    rules: &[SearchEntryTypeRule],
) {
    if rules.is_empty() {
        return;
    }
    sql.push_str(" AND (");
    for (index, rule) in rules.iter().enumerate() {
        if index > 0 {
            sql.push_str(" OR ");
        }
        match rule {
            SearchEntryTypeRule::Kind(kind) => {
                sql.push_str("f.kind = ?");
                values.push(Value::Text(kind.as_storage_value().to_owned()));
            }
            SearchEntryTypeRule::Mime(MimePattern::Exact(mime_type)) => {
                sql.push_str("f.mime_type = ?");
                values.push(Value::Text(mime_type.clone()));
            }
            // 与 `starts_with` 一样区分大小写；不用 LIKE（只折叠 ASCII，且要转义通配符）。
            SearchEntryTypeRule::Mime(MimePattern::Prefix(prefix)) => {
                sql.push_str("substr(f.mime_type, 1, ?) = ?");
                values.push(Value::Integer(prefix.chars().count() as i64));
                values.push(Value::Text(prefix.clone()));
            }
        }
    }
    sql.push(')');
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

fn entry_type_rule_matches(rule: &SearchEntryTypeRule, entry: &EntryFacts<'_>) -> bool {
    match rule {
        SearchEntryTypeRule::Kind(expected) => *expected == entry.kind,
        SearchEntryTypeRule::Mime(MimePattern::Exact(expected)) => {
            entry.mime_type == Some(expected.as_str())
        }
        SearchEntryTypeRule::Mime(MimePattern::Prefix(prefix)) => entry
            .mime_type
            .is_some_and(|mime_type| mime_type.starts_with(prefix.as_str())),
    }
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
