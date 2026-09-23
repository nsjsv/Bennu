/// 表数据与 SQL 结果共用的行数上限：实际查询多读一行用于判断截断。
pub const SQLITE_ROW_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub struct SqliteDatabasePreview {
    pub tables: Vec<SqliteTableSummary>,
}

#[derive(Debug, Clone)]
pub struct SqliteTableSummary {
    pub name: String,
    pub row_count: u64,
}

#[derive(Debug, Clone)]
pub struct SqliteTableData {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqliteCellValue>>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SqlQueryOutcome {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqliteCellValue>>,
    pub truncated: bool,
}

/// BLOB 只保留长度（渲染为 `<BLOB n bytes>`），不把字节拷进 UI。
#[derive(Debug, Clone, PartialEq)]
pub enum SqliteCellValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlitePreviewTab {
    Tables,
    Sql,
}

#[derive(Debug, Clone)]
pub enum SqlitePreviewMessage {
    TabSelected(SqlitePreviewTab),
    TableFilterChanged(String),
    TableSelected(String),
    TableDataLoaded(u64, Result<SqliteTableData, String>),
    SqlTextChanged(String),
    SqlSubmitted,
    SqlFinished(u64, Result<SqlQueryOutcome, String>),
}

// 只读加载层自 app-ui 的 sqlite_preview.rs 随迁（命令层依赖；
// app-ui 状态机经 bennu_preview 路径调用）。
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::OpenFlags;

pub async fn load_sqlite_preview(path: PathBuf) -> Result<SqliteDatabasePreview, String> {
    tokio::task::spawn_blocking(move || load_sqlite_preview_blocking(&path))
        .await
        .map_err(|error| format!("sqlite preview worker failed: {error}"))?
}

pub async fn load_sqlite_table_data(
    path: PathBuf,
    table: String,
) -> Result<SqliteTableData, String> {
    tokio::task::spawn_blocking(move || load_sqlite_table_data_blocking(&path, &table))
        .await
        .map_err(|error| format!("sqlite preview worker failed: {error}"))?
}

pub async fn run_sqlite_sql(path: PathBuf, sql: String) -> Result<SqlQueryOutcome, String> {
    tokio::task::spawn_blocking(move || run_sqlite_sql_blocking(&path, &sql))
        .await
        .map_err(|error| format!("sqlite preview worker failed: {error}"))?
}

/// 预览只读打开：任何写语句（含用户 SQL）都被 SQLite 拒绝，杜绝写坏用户数据库。
/// preview_config 读主软件 state.sqlite 用户偏好时复用同一安全约束。
pub(crate) fn open_read_only(path: &Path) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Could not open database: {error}"))
}

fn sqlite_error(error: rusqlite::Error) -> String {
    format!("Could not read database: {error}")
}

fn load_sqlite_preview_blocking(path: &Path) -> Result<SqliteDatabasePreview, String> {
    let connection = open_read_only(path)?;
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY name",
        )
        .map_err(sqlite_error)?;
    let table_names: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;

    let mut tables = Vec::with_capacity(table_names.len());
    for name in table_names {
        let row_count: u64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {}", quote_identifier(&name)),
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        tables.push(SqliteTableSummary { name, row_count });
    }

    Ok(SqliteDatabasePreview { tables })
}

fn load_sqlite_table_data_blocking(path: &Path, table: &str) -> Result<SqliteTableData, String> {
    let connection = open_read_only(path)?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT * FROM {} LIMIT {}",
            quote_identifier(table),
            SQLITE_ROW_LIMIT + 1
        ))
        .map_err(sqlite_error)?;
    let columns: Vec<String> = (0..statement.column_count())
        .map(|index| statement.column_name(index).unwrap_or_default().to_owned())
        .collect();
    let mut query_rows = statement.query([]).map_err(sqlite_error)?;
    let mut rows: Vec<Vec<SqliteCellValue>> = Vec::new();
    while let Some(row) = query_rows.next().map_err(sqlite_error)? {
        rows.push(row_values(&row).map_err(sqlite_error)?);
    }
    let truncated = rows.len() > SQLITE_ROW_LIMIT;
    rows.truncate(SQLITE_ROW_LIMIT);

    Ok(SqliteTableData {
        table: table.to_owned(),
        columns,
        rows,
        truncated,
    })
}

fn run_sqlite_sql_blocking(path: &Path, sql: &str) -> Result<SqlQueryOutcome, String> {
    let connection = open_read_only(path)?;
    let mut statement = connection.prepare(sql).map_err(sqlite_error)?;
    let columns: Vec<String> = (0..statement.column_count())
        .map(|index| statement.column_name(index).unwrap_or_default().to_owned())
        .collect();
    let mut query_rows = statement.query([]).map_err(sqlite_error)?;
    let mut rows: Vec<Vec<SqliteCellValue>> = Vec::new();
    while let Some(row) = query_rows.next().map_err(sqlite_error)? {
        rows.push(row_values(&row).map_err(sqlite_error)?);
    }
    let truncated = rows.len() > SQLITE_ROW_LIMIT;
    rows.truncate(SQLITE_ROW_LIMIT);

    Ok(SqlQueryOutcome {
        columns,
        rows,
        truncated,
    })
}

fn row_values(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<SqliteCellValue>> {
    let column_count = row.as_ref().column_count();
    (0..column_count)
        .map(|index| Ok(cell_value(row.get_ref(index)?)))
        .collect()
}

fn cell_value(value: ValueRef<'_>) -> SqliteCellValue {
    match value {
        ValueRef::Null => SqliteCellValue::Null,
        ValueRef::Integer(integer) => SqliteCellValue::Integer(integer),
        ValueRef::Real(real) => SqliteCellValue::Real(real),
        ValueRef::Text(text) => SqliteCellValue::Text(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(blob) => SqliteCellValue::Blob(blob.len() as u64),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests;
