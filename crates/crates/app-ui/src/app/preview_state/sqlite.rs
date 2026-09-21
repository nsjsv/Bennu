use std::path::PathBuf;

use iced::{Point, Task};

use super::FileBrowser;
use crate::model::{
    Message, PreviewContent, PreviewState, SqlQueryOutcome, SqliteDatabasePreview,
    SqlitePreviewMessage, SqlitePreviewTab, SqliteTableData,
};

/// SQLite 预览的交互状态：随预览会话在 `start_sqlite_preview` 重建。
pub(crate) struct SqlitePreviewState {
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
    pub(crate) active_tab: SqlitePreviewTab,
    pub(crate) selected_table: Option<String>,
    pub(crate) table_loading: bool,
    pub(crate) table_data: Option<SqliteTableData>,
    pub(crate) sql_text: String,
    pub(crate) sql_running: bool,
    pub(crate) sql_result: Option<Result<SqlQueryOutcome, String>>,
    pub(crate) table_filter: String,
    pub(crate) tables_width: f32,
}

/// 表列表默认宽度；拖动分隔条后随会话保持。
pub(crate) const SQLITE_DEFAULT_TABLES_WIDTH: f32 = 240.0;
const SQLITE_TABLES_MIN_WIDTH: f32 = 140.0;
const SQLITE_DATA_MIN_WIDTH: f32 = 320.0;

/// 表列表宽度拖动：记录起点，移动时按指针位移调整宽度。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SqliteTablesResizeDrag {
    cursor_start_x: f32,
    width_start: f32,
}

fn sqlite_table_data_command(path: PathBuf, table: String, generation: u64) -> Task<Message> {
    Task::perform(
        crate::sqlite_preview::load_sqlite_table_data(path.clone(), table.clone()),
        move |outcome| {
            Message::SqlitePreview(SqlitePreviewMessage::TableDataLoaded(generation, outcome))
        },
    )
}

fn sqlite_sql_command(path: PathBuf, sql: String, generation: u64) -> Task<Message> {
    Task::perform(
        crate::sqlite_preview::run_sqlite_sql(path.clone(), sql),
        move |outcome| {
            Message::SqlitePreview(SqlitePreviewMessage::SqlFinished(generation, outcome))
        },
    )
}

impl FileBrowser {
    pub(in crate::app) fn start_sqlite_preview(
        &mut self,
        path: PathBuf,
        preview: &SqliteDatabasePreview,
    ) -> Task<Message> {
        self.sqlite_preview_generation = self.sqlite_preview_generation.wrapping_add(1);
        let generation = self.sqlite_preview_generation;
        let mut state = SqlitePreviewState {
            path: path.clone(),
            generation,
            active_tab: SqlitePreviewTab::Tables,
            table_filter: String::new(),
            sql_text: String::new(),
            tables_width: SQLITE_DEFAULT_TABLES_WIDTH,
            selected_table: None,
            table_loading: false,
            table_data: None,
            sql_running: false,
            sql_result: None,
        };
        let command = match preview.tables.first() {
            // 自动选中第一个表并加载数据。
            Some(summary) => {
                state.selected_table = Some(summary.name.clone());
                state.table_loading = true;
                sqlite_table_data_command(path, summary.name.clone(), generation)
            }
            None => Task::none(),
        };
        self.sqlite_preview = Some(state);
        command
    }

    pub(in crate::app) fn handle_sqlite_preview_message(
        &mut self,
        message: SqlitePreviewMessage,
    ) -> Task<Message> {
        match message {
            SqlitePreviewMessage::TabSelected(tab) => {
                if let Some(state) = self.active_sqlite_preview_mut() {
                    state.active_tab = tab;
                }
                Task::none()
            }
            SqlitePreviewMessage::TableFilterChanged(filter) => {
                if let Some(state) = self.active_sqlite_preview_mut() {
                    state.table_filter = filter;
                }
                Task::none()
            }
            SqlitePreviewMessage::SqlTextChanged(text) => {
                if let Some(state) = self.active_sqlite_preview_mut() {
                    state.sql_text = text;
                }
                Task::none()
            }
            SqlitePreviewMessage::SqlSubmitted => {
                let Some(state) = self.active_sqlite_preview_mut() else {
                    return Task::none();
                };
                if state.sql_running {
                    return Task::none();
                }
                let sql = state.sql_text.trim().to_owned();
                if sql.is_empty() {
                    return Task::none();
                }
                let generation = state.generation;
                let path = state.path.clone();
                state.sql_running = true;
                return sqlite_sql_command(path, sql, generation);
            }
            SqlitePreviewMessage::TableSelected(table) => {
                let Some(state) = self.active_sqlite_preview_mut() else {
                    return Task::none();
                };
                if state.selected_table.as_deref() == Some(table.as_str()) {
                    return Task::none();
                }
                let generation = state.generation;
                let path = state.path.clone();
                state.selected_table = Some(table.clone());
                state.table_loading = true;
                state.table_data = None;
                sqlite_table_data_command(path, table, generation)
            }
            SqlitePreviewMessage::TableDataLoaded(generation, outcome) => {
                let Some(state) = self.active_sqlite_preview_mut() else {
                    return Task::none();
                };
                if state.generation != generation || !state.table_loading {
                    return Task::none();
                }
                // 切表竞态：过期表的数据到达时保留 loading 状态等当前表。
                if let Ok(data) = &outcome {
                    if state.selected_table.as_deref() != Some(data.table.as_str()) {
                        return Task::none();
                    }
                }
                state.table_loading = false;
                state.table_data = outcome.ok();
                Task::none()
            }
            SqlitePreviewMessage::SqlFinished(generation, outcome) => {
                let Some(state) = self.active_sqlite_preview_mut() else {
                    return Task::none();
                };
                if state.generation != generation || !state.sql_running {
                    return Task::none();
                }
                state.sql_running = false;
                state.sql_result = Some(outcome);
                Task::none()
            }
        }
    }

    /// 只有当前预览确实是 SQLite 文件时才返回状态，防止过期消息写进其他预览。
    pub(in crate::app) fn active_sqlite_preview_mut(&mut self) -> Option<&mut SqlitePreviewState> {
        if matches!(
            self.preview,
            Some(PreviewState::Ready(PreviewContent::Sqlite(_)))
        ) {
            self.sqlite_preview.as_mut()
        } else {
            None
        }
    }

    pub(in crate::app) fn clear_sqlite_preview(&mut self) {
        self.sqlite_preview = None;
    }
}

impl FileBrowser {
    pub(in crate::app) fn start_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        let cursor_start_x = self.cursor_position.x;
        let width_start = self
            .active_sqlite_preview_mut()
            .map(|state| state.tables_width);
        let Some(width_start) = width_start else {
            return Task::none();
        };
        self.sqlite_tables_resize_drag = Some(SqliteTablesResizeDrag {
            cursor_start_x,
            width_start,
        });
        Task::none()
    }

    pub(in crate::app) fn update_sqlite_tables_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.sqlite_tables_resize_drag else {
            return;
        };
        let max_width =
            (self.preview_size.width - SQLITE_DATA_MIN_WIDTH).max(SQLITE_TABLES_MIN_WIDTH);
        let width = drag.width_start + position.x - drag.cursor_start_x;
        if let Some(state) = self.active_sqlite_preview_mut() {
            state.tables_width = width.clamp(SQLITE_TABLES_MIN_WIDTH, max_width);
        }
    }

    pub(in crate::app) fn finish_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        self.sqlite_tables_resize_drag = None;
        Task::none()
    }
}
