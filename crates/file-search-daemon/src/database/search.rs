use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use crate::error::{SearchError, SearchResult};
use crate::model::{
    MatchSource, SearchCursor, SearchFileKind, SearchHit, SearchMatchMode, SearchProviderFailure,
    SearchQuery, SearchResultBatch, SearchScope, SearchTextScope,
};
use crate::query_state::QueryState;
use crate::ranked_snapshots::RankedRow;
use crate::ranking::{relevance, RankingFacts};
use crate::text_matching::{
    content_term_tokens, fts_phrase, locate_content_term, name_term_tokens, SearchTerms,
};

use super::{
    path_from_storage_bytes, path_to_storage, recursive_storage_range, RecursiveStorageRange,
    SearchDatabase,
};

const QUERY_VISIBLE_PREDICATE: &str = "f.tombstoned = 0 AND f.observation_state = 'observable'";
/// 命中行的列，顺序固定，后面紧跟预览列；`hit_from_row` 按这个顺序读。
const HIT_COLUMNS: &str = "f.path, f.display_name, f.kind, f.size, f.modified_ms, f.accessed_ms,
    f.created_ms, f.search_name";
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
    pub(crate) fn search(
        &self,
        query: &SearchQuery,
        query_state: &QueryState,
    ) -> SearchResult<SearchResultBatch> {
        if query.match_mode == SearchMatchMode::Regex {
            return self.search_regex(query);
        }
        let terms = SearchTerms::parse(&query.terms);
        let text = TextPredicates::new(&terms, query.text_scope);
        if text.is_empty() {
            self.search_by_metadata(query, &terms, &text)
        } else {
            self.search_ranked(query, &terms, &text, query_state)
        }
    }

    /// 有搜索词就按相关度排：排序要看每条命中的名字、位置和时间，拿不到「只算前一页」
    /// 的捷径。所以第一页一次给全部命中打分并定格，后续页按位置回表，不再重新打分。
    fn search_ranked(
        &self,
        query: &SearchQuery,
        terms: &SearchTerms,
        text: &TextPredicates,
        query_state: &QueryState,
    ) -> SearchResult<SearchResultBatch> {
        let limit = query.limit.clamp(1, 200);
        let ranked_snapshots = &query_state.ranked_snapshots;
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, rows, position) = match query.cursor {
            None => {
                let rows: Arc<[RankedRow]> = ranked_rows(
                    &transaction,
                    query,
                    terms,
                    text,
                    &query_state.recent_files.last_used(),
                )?
                .into();
                (ranked_snapshots.insert(query, Arc::clone(&rows)), rows, 0)
            }
            Some(SearchCursor::RankedSnapshot {
                snapshot_id,
                position,
            }) => {
                let rows =
                    ranked_snapshots
                        .get(snapshot_id, query)
                        .ok_or(SearchError::SearchFailed {
                            query_id: query.query_id,
                            failure: SearchProviderFailure::RankedSnapshotExpired,
                        })?;
                (snapshot_id, rows, position)
            }
            Some(SearchCursor::Offset { .. }) => {
                return Err(SearchError::InvalidQuery(
                    "relevance-ranked search pages by snapshot position, not by offset".to_owned(),
                ));
            }
        };
        if position > rows.len() {
            return Err(SearchError::InvalidQuery(
                "ranked search cursor is past the end of its results".to_owned(),
            ));
        }
        let (hits, next_position) =
            ranked_page(&transaction, query, terms, text, &rows, position, limit)?;
        transaction.commit()?;

        let has_more = next_position < rows.len();
        Ok(SearchResultBatch {
            query_id: query.query_id,
            hits,
            next_cursor: has_more.then_some(SearchCursor::RankedSnapshot {
                snapshot_id,
                position: next_position,
            }),
            finished: !has_more,
        })
    }

    /// 没有够 3 个字的词时按元数据顺序扫描，凑满一页即停。
    fn search_by_metadata(
        &self,
        query: &SearchQuery,
        terms: &SearchTerms,
        text: &TextPredicates,
    ) -> SearchResult<SearchResultBatch> {
        let limit = query.limit.clamp(1, 200);
        let offset = live_offset(query)?;
        let transaction = self.connection.unchecked_transaction()?;
        let (sql, values) = metadata_sql(query, text, limit.saturating_add(1), offset);
        let hits = {
            let mut statement = transaction.prepare(&sql)?;
            let rows = statement
                .query_map(params_from_iter(values), |row| {
                    hit_from_row(row, 0, query, terms, 0.0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        transaction.commit()?;
        Ok(offset_page(query, hits, limit, offset))
    }

    /// 正则无法在 FTS 倒排索引上执行：按 rowid 扫描序流式过滤名称，凑满一页提前退出。
    fn search_regex(&self, query: &SearchQuery) -> SearchResult<SearchResultBatch> {
        let limit = query.limit.clamp(1, 200);
        let fetch_limit = limit.saturating_add(1);
        let offset = live_offset(query)?;
        let name_regex = query.match_mode.name_regex(&query.terms)?;
        let transaction = self.connection.unchecked_transaction()?;
        let (sql, values) = regex_scan_sql(query);
        let hits = {
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
        Ok(offset_page(query, hits, limit, offset))
    }

    /// 第一页实际执行的语句的查询计划。
    #[cfg(test)]
    pub(super) fn search_plan(&self, query: &SearchQuery) -> SearchResult<Vec<String>> {
        let text = TextPredicates::new(&SearchTerms::parse(&query.terms), query.text_scope);
        let (sql, values) = if text.is_empty() {
            metadata_sql(query, &text, query.limit.clamp(1, 200).saturating_add(1), 0)
        } else {
            candidate_sql(
                query,
                &text,
                full_text_query_plan(&self.connection, query, &text)?,
            )
        };
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

fn live_offset(query: &SearchQuery) -> SearchResult<usize> {
    match query.cursor {
        None => Ok(0),
        Some(SearchCursor::Offset { offset }) => Ok(offset),
        Some(SearchCursor::RankedSnapshot { .. }) => Err(SearchError::InvalidQuery(
            "only relevance-ranked search pages by snapshot position".to_owned(),
        )),
    }
}

/// `hits` 多取了一条，用来判断后面还有没有。
fn offset_page(
    query: &SearchQuery,
    mut hits: Vec<SearchHit>,
    limit: usize,
    offset: usize,
) -> SearchResultBatch {
    let has_more = hits.len() > limit;
    hits.truncate(limit);
    let next_cursor = has_more.then_some(SearchCursor::Offset {
        offset: offset + hits.len(),
    });
    SearchResultBatch {
        query_id: query.query_id,
        hits,
        next_cursor,
        finished: !has_more,
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

fn preview_columns(query: &SearchQuery, text: &TextPredicates) -> (&'static str, &'static str) {
    if query.text_scope == SearchTextScope::NameAndContent && !text.is_empty() {
        (
            "file_search_snippets.preview",
            " LEFT JOIN file_search_snippets ON file_search_snippets.file_rowid = f.rowid",
        )
    } else {
        ("NULL", "")
    }
}

/// 打分要用的候选列，顺序固定：rowid、只算正文的 bm25（没有全文表达式时为 NULL）、
/// display_name、search_name、path、modified_ms。
const CANDIDATE_COLUMNS: &str = "f.display_name, f.search_name, f.path, f.modified_ms";

/// 全部候选，不在 SQL 里排序（见 `ranked_rows`）。有够 3 个字的词时由 FTS 驱动，
/// 全可见且没有范围和过滤条件时（`UnfilteredRank`）不带可见性谓词；只有短词时按
/// 名字子串和正文短语扫描。
fn candidate_sql(
    query: &SearchQuery,
    text: &TextPredicates,
    full_text_plan: FullTextQueryPlan,
) -> (String, Vec<Value>) {
    let Some(match_expression) = text.match_expression.as_ref() else {
        let mut values = Vec::new();
        let mut sql = format!("SELECT f.rowid, NULL, {CANDIDATE_COLUMNS} FROM files f");
        append_metadata_conditions(&mut sql, &mut values, query, text);
        return (sql, values);
    };
    let mut values = vec![Value::Text(match_expression.clone())];
    // 名字列权重为 0：名字匹配由排序档位决定，bm25 只衡量正文。
    let mut sql = format!(
        "SELECT f.rowid, bm25(file_search_fts, 0.0, 1.0), {CANDIDATE_COLUMNS}
         FROM file_search_fts JOIN files f ON f.rowid = file_search_fts.rowid
         WHERE file_search_fts MATCH ?"
    );
    if full_text_plan == FullTextQueryPlan::FilterWithMetadata {
        sql.push_str(" AND ");
        sql.push_str(QUERY_VISIBLE_PREDICATE);
        append_scope_filter(&mut sql, &mut values, query);
        text.append_short_term_sql(&mut sql, &mut values);
        query.filters.append_sql(&mut sql, &mut values);
    }
    (sql, values)
}

/// 给全部候选打分，按相关度从高到低排，同分按 rowid 排，翻页顺序因此是确定的。
/// 排序在这里而不是 SQL 里做：全部命中都要排时，SQL 排序比读出来排慢一倍。
fn ranked_rows(
    connection: &Connection,
    query: &SearchQuery,
    terms: &SearchTerms,
    text: &TextPredicates,
    last_used: &HashMap<std::path::PathBuf, i64>,
) -> SearchResult<Vec<RankedRow>> {
    let full_text_plan = if text.match_expression.is_some() {
        full_text_query_plan(connection, query, text)?
    } else {
        FullTextQueryPlan::FilterWithMetadata
    };
    let short_term_content = short_term_content_scores(connection, text)?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let (sql, values) = candidate_sql(query, text, full_text_plan);
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement
        .query_map(params_from_iter(values), |row| {
            let rowid: i64 = row.get(0)?;
            let content_bm25: Option<f64> = row.get(1)?;
            let display_name: String = row.get(2)?;
            let search_name: String = row.get(3)?;
            let path = path_from_storage_bytes(row.get(4)?);
            let facts = RankingFacts {
                display_name: &display_name,
                search_name: &search_name,
                depth: depth_below_scope(query, &path),
                modified_ms: row.get(5)?,
                last_used_ms: last_used.get(&path).copied(),
                content_bm25: content_bm25.or_else(|| short_term_content.get(&rowid).copied()),
            };
            Ok(RankedRow {
                rowid,
                rank: relevance(terms, &facts, now_ms),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.sort_unstable_by(|left, right| {
        right
            .rank
            .total_cmp(&left.rank)
            .then(left.rowid.cmp(&right.rowid))
    });
    Ok(rows)
}

/// 只有短词时没有驱动查询的全文表达式，正文相关度要按每个短词的正文短语另查一次。
fn short_term_content_scores(
    connection: &Connection,
    text: &TextPredicates,
) -> SearchResult<HashMap<i64, f64>> {
    let mut scores = HashMap::new();
    if text.match_expression.is_some() {
        return Ok(scores);
    }
    for content_expression in text
        .short_terms
        .iter()
        .filter_map(|short_term| short_term.content_expression.as_ref())
    {
        let mut statement = connection.prepare_cached(
            "SELECT rowid, bm25(file_search_fts, 0.0, 1.0) FROM file_search_fts
             WHERE file_search_fts MATCH ?",
        )?;
        let term_scores = statement.query_map([content_expression], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        for term_score in term_scores {
            let (rowid, score) = term_score?;
            *scores.entry(rowid).or_insert(0.0) += score;
        }
    }
    Ok(scores)
}

/// 相对搜索目录的层数，直接子项为 0；不递归时都算直接子项；全局搜索没有层数。
fn depth_below_scope(query: &SearchQuery, path: &Path) -> Option<usize> {
    match &query.scope {
        SearchScope::Directory(directory) if query.recursive => path
            .strip_prefix(directory)
            .ok()
            .map(|relative| relative.components().count().saturating_sub(1)),
        SearchScope::Directory(_) => Some(0),
        SearchScope::Global => None,
    }
}

/// 从定格的 `position` 起凑一页。定格之后文件可能被删除或隐藏，回表时带上可见性
/// 条件，查不到的跳过、继续往后取。返回这一页和下一页的起点。
fn ranked_page(
    connection: &Connection,
    query: &SearchQuery,
    terms: &SearchTerms,
    text: &TextPredicates,
    rows: &[RankedRow],
    mut position: usize,
    limit: usize,
) -> SearchResult<(Vec<SearchHit>, usize)> {
    let (preview_projection, preview_join) = preview_columns(query, text);
    let mut hits = Vec::with_capacity(limit);
    while hits.len() < limit && position < rows.len() {
        let chunk = &rows[position..rows.len().min(position + limit - hits.len())];
        let placeholders = vec!["?"; chunk.len()].join(", ");
        // NOT INDEXED：可见性条件会让规划器改走 (tombstoned, observation_state, …) 索引，
        // 把全部可见文件扫一遍再比 rowid；这里只要按主键取这几行。
        let sql = format!(
            "SELECT f.rowid, {HIT_COLUMNS}, {preview_projection}
             FROM files f NOT INDEXED{preview_join}
             WHERE f.rowid IN ({placeholders}) AND {QUERY_VISIBLE_PREDICATE}"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut visible = statement
            .query_map(params_from_iter(chunk.iter().map(|row| row.rowid)), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    hit_from_row(row, 1, query, terms, 0.0)?,
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        hits.extend(chunk.iter().filter_map(|row| {
            visible.remove(&row.rowid).map(|hit| SearchHit {
                rank: row.rank,
                ..hit
            })
        }));
        position += chunk.len();
    }
    Ok((hits, position))
}

/// 只有短词或没有词时按元数据顺序扫描，凑满一页即停。
fn metadata_sql(
    query: &SearchQuery,
    text: &TextPredicates,
    limit: usize,
    offset: usize,
) -> (String, Vec<Value>) {
    let (preview_projection, preview_join) = preview_columns(query, text);
    let mut values = Vec::new();
    let mut sql = format!("SELECT {HIT_COLUMNS}, {preview_projection} FROM files f{preview_join}");
    append_metadata_conditions(&mut sql, &mut values, query, text);
    sql.push_str(" ORDER BY f.modified_ms DESC, f.display_name ASC LIMIT ? OFFSET ?");
    values.push(Value::Integer(limit as i64));
    values.push(Value::Integer(offset as i64));
    (sql, values)
}

/// 不经 FTS 驱动时的条件：可见性、范围、短词、过滤。递归范围先按路径区间取 rowid，
/// 让外层沿可见性索引走。
fn append_metadata_conditions(
    sql: &mut String,
    values: &mut Vec<Value>,
    query: &SearchQuery,
    text: &TextPredicates,
) {
    match recursive_path_range(query) {
        Some(range) => {
            sql.push_str(" WHERE f.rowid IN (SELECT rowid FROM files WHERE ");
            append_recursive_path_range(sql, values, "path", &range);
            sql.push_str(") AND ");
            sql.push_str(QUERY_VISIBLE_PREDICATE);
        }
        None => {
            sql.push_str(" WHERE ");
            sql.push_str(QUERY_VISIBLE_PREDICATE);
            append_scope_filter(sql, values, query);
        }
    }
    text.append_short_term_sql(sql, values);
    query.filters.append_sql(sql, values);
}

/// 从第 `first_column` 列起按 `HIT_COLUMNS` 加预览列的顺序读一条命中。
fn hit_from_row(
    row: &rusqlite::Row<'_>,
    first_column: usize,
    query: &SearchQuery,
    terms: &SearchTerms,
    rank: f64,
) -> rusqlite::Result<SearchHit> {
    let path_bytes: Vec<u8> = row.get(first_column)?;
    let display_name: String = row.get(first_column + 1)?;
    let kind = SearchFileKind::from_storage_value(row.get_ref(first_column + 2)?.as_str()?);
    let search_name: String = row.get(first_column + 7)?;
    let preview: Option<String> = row.get(first_column + 8)?;
    let match_source = match_source_for_hit(query.text_scope, terms, &search_name);
    let snippet = match (match_source, preview) {
        (MatchSource::Content, Some(preview)) => {
            content_snippet_from_preview(&search_name, &preview, terms)
        }
        _ => None,
    };
    Ok(SearchHit {
        path: path_from_storage_bytes(path_bytes),
        display_name,
        kind,
        size: row.get::<_, i64>(first_column + 3)? as u64,
        modified_ms: row.get(first_column + 4)?,
        accessed_ms: row.get(first_column + 5)?,
        created_ms: row.get(first_column + 6)?,
        rank,
        snippet,
        match_source,
    })
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
