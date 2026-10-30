use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::error::SearchResult;
use crate::model::{
    MatchSource, SearchCursor, SearchFileKind, SearchHit, SearchMatchMode, SearchQuery,
    SearchResultBatch, SearchScope, SearchTextScope,
};
use crate::text_matching::{
    content_term_tokens, fts_phrase, locate_content_term, name_term_tokens, SearchTerms,
};

use super::{
    path_from_storage_bytes, path_to_storage, recursive_storage_range, RecursiveStorageRange,
    SearchDatabase,
};

const QUERY_VISIBLE_PREDICATE: &str = "f.tombstoned = 0 AND f.observation_state = 'observable'";
const SEARCH_SNIPPET_CHARACTER_LIMIT: usize = 240;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FullTextQueryPlan {
    FilterWithMetadata,
    UnfilteredRank,
}

/// 查询词按 `text_matching` 的规则落到 SQL：够 3 个字的词进 FTS 表达式（名称 trigram
/// 短语，`NameAndContent` 时或上内容短语）；不满 3 个字的词作为附加条件，名称直接在
/// 规范化名称上找子串。
struct TextPredicates {
    match_expression: Option<String>,
    short_terms: Vec<ShortTerm>,
}

struct ShortTerm {
    term: String,
    content_expression: Option<String>,
}

impl TextPredicates {
    fn new(terms: &SearchTerms, text_scope: SearchTextScope) -> Self {
        let mut phrases = Vec::new();
        let mut short_terms = Vec::new();
        for term in terms.iter() {
            let content_expression = match text_scope {
                SearchTextScope::NameAndContent => content_term_tokens(term)
                    .map(|tokens| format!("content : {}", fts_phrase(&tokens))),
                SearchTextScope::NameOnly => None,
            };
            match name_term_tokens(term) {
                Some(tokens) => {
                    let name_expression = format!("name : {}", fts_phrase(&tokens));
                    phrases.push(match content_expression {
                        Some(content_expression) => {
                            format!("({name_expression} OR {content_expression})")
                        }
                        None => name_expression,
                    });
                }
                None => short_terms.push(ShortTerm {
                    term: term.to_owned(),
                    content_expression,
                }),
            }
        }
        Self {
            match_expression: (!phrases.is_empty()).then(|| phrases.join(" AND ")),
            short_terms,
        }
    }

    fn is_empty(&self) -> bool {
        self.match_expression.is_none() && self.short_terms.is_empty()
    }

    fn append_short_term_sql(&self, sql: &mut String, values: &mut Vec<Value>) {
        for short_term in &self.short_terms {
            sql.push_str(" AND (instr(f.search_name, ?) > 0");
            values.push(Value::Text(short_term.term.clone()));
            if let Some(content_expression) = &short_term.content_expression {
                sql.push_str(
                    " OR f.rowid IN (
                        SELECT rowid FROM file_search_fts WHERE file_search_fts MATCH ?
                     )",
                );
                values.push(Value::Text(content_expression.clone()));
            }
            sql.push(')');
        }
    }
}

impl SearchDatabase {
    pub fn search(&self, query: &SearchQuery) -> SearchResult<SearchResultBatch> {
        if query.match_mode == SearchMatchMode::Regex {
            return self.search_regex(query);
        }
        let limit = query.limit.clamp(1, 200);
        let fetch_limit = limit.saturating_add(1);
        let offset = query.cursor.map_or(0, |cursor| cursor.offset);
        let terms = SearchTerms::parse(&query.terms);
        let text = TextPredicates::new(&terms, query.text_scope);
        let transaction = self.connection.unchecked_transaction()?;
        let full_text_plan = full_text_query_plan(&transaction, query, &text)?;
        let (sql, values) = search_sql(query, &text, fetch_limit, offset, full_text_plan);

        let mut hits = {
            let mut statement = transaction.prepare(&sql)?;
            let rows = statement
                .query_map(params_from_iter(values), |row| {
                    let path_bytes: Vec<u8> = row.get(0)?;
                    let display_name: String = row.get(1)?;
                    let kind = SearchFileKind::from_storage_value(row.get_ref(2)?.as_str()?);
                    let score: f64 = row.get(7)?;
                    let search_name: String = row.get(8)?;
                    let preview: Option<String> = row.get(9)?;
                    let match_source = match_source_for_hit(query.text_scope, &terms, &search_name);
                    let snippet = match (match_source, preview) {
                        (MatchSource::Content, Some(preview)) => {
                            content_snippet_from_preview(&search_name, &preview, &terms)
                        }
                        _ => None,
                    };
                    Ok(SearchHit {
                        path: path_from_storage_bytes(path_bytes),
                        display_name,
                        kind,
                        size: row.get::<_, i64>(3)? as u64,
                        modified_ms: row.get(4)?,
                        accessed_ms: row.get(5)?,
                        created_ms: row.get(6)?,
                        rank: -score,
                        snippet,
                        match_source,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        transaction.commit()?;

        let has_more = hits.len() > limit;
        hits.truncate(limit);
        let next_cursor = has_more.then_some(SearchCursor {
            offset: offset + hits.len(),
        });
        Ok(SearchResultBatch {
            query_id: query.query_id,
            hits,
            next_cursor,
            finished: !has_more,
        })
    }

    /// 正则无法在 FTS 倒排索引上执行：按 rowid 扫描序流式过滤名称，凑满一页提前退出。
    fn search_regex(&self, query: &SearchQuery) -> SearchResult<SearchResultBatch> {
        let limit = query.limit.clamp(1, 200);
        let fetch_limit = limit.saturating_add(1);
        let offset = query.cursor.map_or(0, |cursor| cursor.offset);
        let name_regex = query.match_mode.name_regex(&query.terms)?;
        let transaction = self.connection.unchecked_transaction()?;
        let (sql, values) = regex_scan_sql(query);
        let mut hits = {
            let mut statement = transaction.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(values))?;
            let mut hits = Vec::with_capacity(fetch_limit);
            let mut skipped = 0_usize;
            while let Some(row) = rows.next()? {
                let display_name: String = row.get(1)?;
                if !name_regex.is_match(&display_name) {
                    continue;
                }
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                let path_bytes: Vec<u8> = row.get(0)?;
                let kind_value: String = row.get(2)?;
                let kind = SearchFileKind::from_storage_value(&kind_value);
                hits.push(SearchHit {
                    path: path_from_storage_bytes(path_bytes),
                    display_name,
                    kind,
                    size: row.get::<_, i64>(3)? as u64,
                    modified_ms: row.get(4)?,
                    accessed_ms: row.get(5)?,
                    created_ms: row.get(6)?,
                    rank: 0.0,
                    snippet: None,
                    match_source: if query.terms.is_empty() {
                        MatchSource::Metadata
                    } else {
                        MatchSource::Name
                    },
                });
                if hits.len() == fetch_limit {
                    break;
                }
            }
            hits
        };
        transaction.commit()?;

        let has_more = hits.len() > limit;
        hits.truncate(limit);
        let next_cursor = has_more.then_some(SearchCursor {
            offset: offset + hits.len(),
        });
        Ok(SearchResultBatch {
            query_id: query.query_id,
            hits,
            next_cursor,
            finished: !has_more,
        })
    }

    #[cfg(test)]
    pub(super) fn search_plan(&self, query: &SearchQuery) -> SearchResult<Vec<String>> {
        let limit = query.limit.clamp(1, 200).saturating_add(1);
        let offset = query.cursor.map_or(0, |cursor| cursor.offset);
        let text = TextPredicates::new(&SearchTerms::parse(&query.terms), query.text_scope);
        let full_text_plan = full_text_query_plan(&self.connection, query, &text)?;
        let (sql, values) = search_sql(query, &text, limit, offset, full_text_plan);
        let mut statement = self
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let plan = statement
            .query_map(params_from_iter(values), |row| row.get(3))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        plan
    }

    pub fn indexed_file_count(&self) -> SearchResult<u64> {
        let sql = format!("SELECT COUNT(*) FROM files f WHERE {QUERY_VISIBLE_PREDICATE}");
        let count: i64 = self.connection.query_row(&sql, [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }
}

/// 查询词实际使用的 FTS 表达式（只含够 3 个字的词）。整库重建拿它核对旧命中，测试拿它
/// 对照显式 `bm25()`。
pub(super) fn full_text_match_expression(
    terms: &str,
    text_scope: SearchTextScope,
) -> Option<String> {
    TextPredicates::new(&SearchTerms::parse(terms), text_scope).match_expression
}

fn regex_scan_sql(query: &SearchQuery) -> (String, Vec<Value>) {
    let mut values = Vec::new();
    let mut sql = "SELECT f.path, f.display_name, f.kind, f.size, f.modified_ms,
                f.accessed_ms, f.created_ms
             FROM files f
             WHERE "
        .to_owned()
        + QUERY_VISIBLE_PREDICATE;
    append_scope_filter(&mut sql, &mut values, query);
    query.filters.append_sql(&mut sql, &mut values);
    (sql, values)
}

fn full_text_query_plan(
    connection: &rusqlite::Connection,
    query: &SearchQuery,
    text: &TextPredicates,
) -> SearchResult<FullTextQueryPlan> {
    let unrestricted_full_text_query = text.match_expression.is_some()
        && text.short_terms.is_empty()
        && matches!(query.scope, SearchScope::Global)
        && query.filters.entry_type_rules.is_empty()
        && query.filters.modified.is_none()
        && query.filters.accessed.is_none()
        && query.filters.created.is_none()
        && query.filters.extensions.is_empty()
        && query.filters.size.is_none();
    if !unrestricted_full_text_query {
        return Ok(FullTextQueryPlan::FilterWithMetadata);
    }
    let hidden_rows_exist = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM files
            WHERE tombstoned <> 0 OR observation_state <> 'observable'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(if hidden_rows_exist {
        FullTextQueryPlan::FilterWithMetadata
    } else {
        FullTextQueryPlan::UnfilteredRank
    })
}

/// 列顺序固定：path, display_name, kind, size, modified, accessed, created, score,
/// search_name, preview。有 FTS 表达式时由 FTS5 原生 rank 顺序驱动并提前满足 LIMIT；
/// 只有短词或没有词时按元数据顺序扫描，凑满一页即停。
fn search_sql(
    query: &SearchQuery,
    text: &TextPredicates,
    limit: usize,
    offset: usize,
    full_text_plan: FullTextQueryPlan,
) -> (String, Vec<Value>) {
    let recursive_range = recursive_path_range(query);
    let range_drives_query = text.match_expression.is_none() && recursive_range.is_some();
    let (preview_projection, preview_join) =
        if query.text_scope == SearchTextScope::NameAndContent && !text.is_empty() {
            (
                "file_search_snippets.preview",
                " LEFT JOIN file_search_snippets ON file_search_snippets.file_rowid = f.rowid",
            )
        } else {
            ("NULL", "")
        };
    let score_projection = if text.match_expression.is_some() {
        "file_search_fts.rank"
    } else {
        "0.0"
    };
    let mut values = Vec::new();
    let mut sql = format!(
        "SELECT f.path, f.display_name, f.kind, f.size, f.modified_ms, f.accessed_ms,
                f.created_ms, {score_projection} AS score, f.search_name,
                {preview_projection} AS preview"
    );
    if let Some(match_expression) = text.match_expression.as_ref() {
        sql.push_str(" FROM file_search_fts JOIN files f ON f.rowid = file_search_fts.rowid");
        sql.push_str(preview_join);
        sql.push_str(" WHERE file_search_fts MATCH ?");
        values.push(Value::Text(match_expression.clone()));
        if full_text_plan == FullTextQueryPlan::FilterWithMetadata {
            sql.push_str(" AND ");
            sql.push_str(QUERY_VISIBLE_PREDICATE);
        }
    } else if let Some(range) = recursive_range.as_ref() {
        sql.push_str(" FROM files f");
        sql.push_str(preview_join);
        sql.push_str(" WHERE f.rowid IN (SELECT rowid FROM files WHERE ");
        append_recursive_path_range(&mut sql, &mut values, "path", range);
        sql.push_str(") AND ");
        sql.push_str(QUERY_VISIBLE_PREDICATE);
    } else {
        sql.push_str(" FROM files f");
        sql.push_str(preview_join);
        sql.push_str(" WHERE ");
        sql.push_str(QUERY_VISIBLE_PREDICATE);
    }

    if !range_drives_query {
        append_scope_filter(&mut sql, &mut values, query);
    }
    text.append_short_term_sql(&mut sql, &mut values);
    query.filters.append_sql(&mut sql, &mut values);
    if text.match_expression.is_some() {
        // FTS5 可在原生 rank 顺序下提前满足 LIMIT，避免评分并排序全部命中。
        sql.push_str(" ORDER BY file_search_fts.rank LIMIT ? OFFSET ?");
    } else {
        sql.push_str(" ORDER BY f.modified_ms DESC, f.display_name ASC LIMIT ? OFFSET ?");
    }
    values.push(Value::Integer(limit as i64));
    values.push(Value::Integer(offset as i64));
    (sql, values)
}

fn append_scope_filter(sql: &mut String, values: &mut Vec<Value>, query: &SearchQuery) {
    let SearchScope::Directory(directory) = &query.scope else {
        return;
    };
    if query.recursive {
        let range = recursive_path_range(query).expect("recursive directory range");
        sql.push_str(" AND ");
        append_recursive_path_range(sql, values, "f.path", &range);
    } else {
        sql.push_str(" AND f.parent_path = ?");
        values.push(Value::Blob(path_to_storage(directory)));
    }
}

fn recursive_path_range(query: &SearchQuery) -> Option<RecursiveStorageRange> {
    if !query.recursive {
        return None;
    }
    let SearchScope::Directory(directory) = &query.scope else {
        return None;
    };
    Some(recursive_storage_range(directory))
}

fn append_recursive_path_range(
    sql: &mut String,
    values: &mut Vec<Value>,
    column: &str,
    range: &RecursiveStorageRange,
) {
    sql.push('(');
    sql.push_str(column);
    sql.push_str(" = ? OR (");
    values.push(Value::Blob(range.exact_path.clone()));
    sql.push_str(column);
    sql.push_str(" >= ? AND ");
    sql.push_str(column);
    sql.push_str(" < ?))");
    values.push(Value::Blob(range.descendant_lower.clone()));
    values.push(Value::Blob(range.descendant_upper.clone()));
}

/// 所有词都是规范化名称的子串 → 名称命中；否则至少有一个词只在内容里。
fn match_source_for_hit(
    text_scope: SearchTextScope,
    terms: &SearchTerms,
    search_name: &str,
) -> MatchSource {
    if terms.is_empty() {
        return MatchSource::Metadata;
    }
    if text_scope == SearchTextScope::NameOnly || terms.all_in_name(search_name) {
        MatchSource::Name
    } else {
        MatchSource::Content
    }
}

/// 只为不在名称里的词在预览里按内容规则定位；命中位置在预览之外时没有摘要。
fn content_snippet_from_preview(
    search_name: &str,
    preview: &str,
    terms: &SearchTerms,
) -> Option<String> {
    let normalized = collapse_preview_whitespace(preview)?;
    let match_character_index = terms
        .iter()
        .filter(|term| !search_name.contains(term))
        .find_map(|term| locate_content_term(&normalized, term))?;
    Some(snippet_around_character(&normalized, match_character_index))
}

fn collapse_preview_whitespace(preview: &str) -> Option<String> {
    let mut normalized = String::with_capacity(preview.len());
    let mut pending_space = false;
    for character in preview.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(character);
    }
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn snippet_around_character(content: &str, match_character_index: usize) -> String {
    let characters = content.chars().collect::<Vec<_>>();
    if characters.len() <= SEARCH_SNIPPET_CHARACTER_LIMIT {
        return content.to_owned();
    }

    let mut start = match_character_index
        .min(characters.len().saturating_sub(1))
        .saturating_sub(60);
    let prefix_characters = usize::from(start > 0) * 3;
    let mut end = (start
        + SEARCH_SNIPPET_CHARACTER_LIMIT
            .saturating_sub(prefix_characters)
            .saturating_sub(3))
    .min(characters.len());
    if end == characters.len() {
        start = characters
            .len()
            .saturating_sub(SEARCH_SNIPPET_CHARACTER_LIMIT.saturating_sub(3));
        end = characters.len();
    }

    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str("...");
    }
    snippet.extend(characters[start..end].iter());
    if end < characters.len() {
        snippet.push_str("...");
    }
    snippet
}
