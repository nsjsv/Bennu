//! 按字段排的结果顺序。要对全部命中排，所以第一页取出全部候选、排好后定格，翻页按位置
//! 取，与相关度共用定格。顺序规则与界面目录排序一致（见 `SearchOrder`）。

use std::cmp::Ordering;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use file_search::filter_semantics::ResolvedFilters;
use rusqlite::{params_from_iter, Connection};

use crate::error::SearchResult;
use crate::model::{
    SearchFileKind, SearchMatchMode, SearchQuery, SearchSortDirection, SearchSortField,
    SearchTextScope,
};
use crate::result_snapshots::SnapshotRow;
use crate::text_matching::SearchTerms;

use super::{candidate_sql, path_from_storage_bytes, TextPredicates};

struct OrderedCandidate {
    rowid: i64,
    path: PathBuf,
    kind: SearchFileKind,
    size: i64,
    modified_ms: Option<i64>,
}

impl OrderedCandidate {
    fn name(&self) -> &[u8] {
        self.path
            .file_name()
            .map_or(self.path.as_os_str(), |name| name)
            .as_bytes()
    }

    fn location(&self) -> &[u8] {
        self.path
            .parent()
            .map_or(&[][..], |parent| parent.as_os_str().as_bytes())
    }
}

/// 全部命中按 `field` 排好的行；同序时按 rowid，翻页顺序因此是确定的。
pub(super) fn ordered_rows(
    connection: &Connection,
    query: &SearchQuery,
    text: &TextPredicates,
    filters: &ResolvedFilters<'_>,
    field: SearchSortField,
    direction: SearchSortDirection,
    directories_first: bool,
) -> SearchResult<Vec<SnapshotRow>> {
    let mut candidates = match query.match_mode {
        SearchMatchMode::Plain => plain_candidates(connection, query, text, filters)?,
        SearchMatchMode::Regex => regex_candidates(connection, query, filters)?,
    };
    candidates.sort_unstable_by(|left, right| {
        compare_candidates(left, right, field, direction, directories_first)
            .then(left.rowid.cmp(&right.rowid))
    });
    Ok(candidates
        .into_iter()
        .map(|candidate| SnapshotRow {
            rowid: candidate.rowid,
            rank: 0.0,
        })
        .collect())
}

fn plain_candidates(
    connection: &Connection,
    query: &SearchQuery,
    text: &TextPredicates,
    filters: &ResolvedFilters<'_>,
) -> SearchResult<Vec<OrderedCandidate>> {
    let (sql, values) = candidate_sql(query, text, filters);
    let mut statement = connection.prepare(&sql)?;
    let candidates = statement
        .query_map(params_from_iter(values), candidate_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(candidates)
}

/// 正则不能走全文索引：按范围和过滤取出候选，再按名字过滤。
fn regex_candidates(
    connection: &Connection,
    query: &SearchQuery,
    filters: &ResolvedFilters<'_>,
) -> SearchResult<Vec<OrderedCandidate>> {
    let name_regex = query.match_mode.name_regex(&query.terms)?;
    let no_terms = TextPredicates::new(&SearchTerms::parse(""), SearchTextScope::NameOnly);
    let (sql, values) = candidate_sql(query, &no_terms, filters);
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        let display_name: String = row.get(2)?;
        if name_regex.is_match(&display_name) {
            candidates.push(candidate_from_row(row)?);
        }
    }
    Ok(candidates)
}

/// 候选列的顺序见 `CANDIDATE_COLUMNS`：rowid、bm25、display_name、search_name、path、
/// modified_ms、kind、size。
fn candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OrderedCandidate> {
    Ok(OrderedCandidate {
        rowid: row.get(0)?,
        path: path_from_storage_bytes(row.get(4)?),
        modified_ms: row.get(5)?,
        kind: SearchFileKind::from_storage_value(row.get_ref(6)?.as_str()?),
        size: row.get(7)?,
    })
}

fn compare_candidates(
    left: &OrderedCandidate,
    right: &OrderedCandidate,
    field: SearchSortField,
    direction: SearchSortDirection,
    directories_first: bool,
) -> Ordering {
    if directories_first {
        let left_is_directory = left.kind == SearchFileKind::Directory;
        let right_is_directory = right.kind == SearchFileKind::Directory;
        if left_is_directory != right_is_directory {
            return right_is_directory.cmp(&left_is_directory);
        }
    }
    let by_name = || left.name().cmp(right.name());
    let ordering = match field {
        SearchSortField::Name => by_name(),
        SearchSortField::Location => left.location().cmp(right.location()).then_with(by_name),
        SearchSortField::Size => left.size.cmp(&right.size).then_with(by_name),
        SearchSortField::Kind => kind_rank(left.kind)
            .cmp(&kind_rank(right.kind))
            .then_with(by_name),
        SearchSortField::Modified => left.modified_ms.cmp(&right.modified_ms).then_with(by_name),
    };
    match direction {
        SearchSortDirection::Ascending => ordering,
        SearchSortDirection::Descending => ordering.reverse(),
    }
}

fn kind_rank(kind: SearchFileKind) -> u8 {
    match kind {
        SearchFileKind::Directory => 0,
        SearchFileKind::File => 1,
        SearchFileKind::Symlink => 2,
        SearchFileKind::Other => 3,
    }
}

#[cfg(test)]
#[path = "ordered_search_tests.rs"]
mod tests;
