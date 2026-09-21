/// 表数据与 SQL 结果共用的行数上限：实际查询多读一行用于判断截断。
pub(crate) const SQLITE_ROW_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub(crate) struct SqliteDatabasePreview {
    pub(crate) tables: Vec<SqliteTableSummary>,
}

#[derive(Debug, Clone)]
pub(crate) struct SqliteTableSummary {
    pub(crate) name: String,
    pub(crate) row_count: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SqliteTableData {
    pub(crate) table: String,
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<SqliteCellValue>>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SqlQueryOutcome {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<SqliteCellValue>>,
    pub(crate) truncated: bool,
}

/// BLOB 只保留长度（渲染为 `<BLOB n bytes>`），不把字节拷进 UI。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SqliteCellValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqlitePreviewTab {
    Tables,
    Sql,
}

#[derive(Debug, Clone)]
pub(crate) enum SqlitePreviewMessage {
    TabSelected(SqlitePreviewTab),
    TableFilterChanged(String),
    TableSelected(String),
    TableDataLoaded(u64, Result<SqliteTableData, String>),
    SqlTextChanged(String),
    SqlSubmitted,
    SqlFinished(u64, Result<SqlQueryOutcome, String>),
}
