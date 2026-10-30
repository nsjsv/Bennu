//! 浏览会话：窗格、标签、选择与前进后退栈，退出时保存、下次启动时恢复。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::database::{current_time_ms, StateDatabase};
use crate::{StoreError, StoreResult, StoredPath};

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS browser_session (
    session_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

pub(crate) const BROWSER_SESSION_KEY: &str = "main";

#[derive(Debug, Clone)]
pub struct BrowserSessionStore {
    database: StateDatabase,
}

impl From<StateDatabase> for BrowserSessionStore {
    fn from(database: StateDatabase) -> Self {
        Self { database }
    }
}

impl BrowserSessionStore {
    pub fn new(db_path: impl AsRef<Path>) -> StoreResult<Self> {
        StateDatabase::open(db_path).map(Self::from)
    }

    pub fn read_browser_session(&self) -> StoreResult<Option<StoredBrowserSession>> {
        let connection = self.database.read();
        let payload_json = connection
            .query_row(
                "SELECT payload_json FROM browser_session WHERE session_key = ?1",
                params![BROWSER_SESSION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        // 读不懂时原样留着：读取没有副作用，下次保存会话时自然覆盖。
        payload_json
            .map(|payload_json| serde_json::from_str(&payload_json))
            .transpose()
            .map_err(StoreError::Json)
    }

    pub fn replace_browser_session(&self, session: &StoredBrowserSession) -> StoreResult<()> {
        let connection = self.database.write();
        upsert_browser_session(&connection, &serde_json::to_string(session)?)
    }

    pub fn clear_browser_session(&self) -> StoreResult<()> {
        let connection = self.database.write();
        connection.execute(
            "DELETE FROM browser_session WHERE session_key = ?1",
            params![BROWSER_SESSION_KEY],
        )?;
        Ok(())
    }
}

/// 单独保存与关闭程序的事务共用的写入口。
pub(crate) fn upsert_browser_session(
    connection: &Connection,
    payload_json: &str,
) -> StoreResult<()> {
    connection.execute(
        "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(session_key) DO UPDATE SET
             payload_json = excluded.payload_json,
             updated_at_ms = excluded.updated_at_ms",
        params![BROWSER_SESSION_KEY, payload_json, current_time_ms()],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredBrowserViewMode {
    Columns,
    List,
    Icons,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredSplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredBrowserPaneLayout {
    Single {
        active: u64,
    },
    Split {
        axis: StoredSplitAxis,
        first: u64,
        second: u64,
        active: u64,
        #[serde(default = "default_split_first_portion")]
        first_portion: u16,
    },
}

fn default_split_first_portion() -> u16 {
    500
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredColumnViewport {
    pub directory: StoredPath,
    pub offset_y: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredColumnBrowserViewport {
    pub offset_x: f32,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBrowserTab {
    pub id: u64,
    pub directory: StoredPath,
    pub is_trash_view: bool,
    pub selected: Option<StoredPath>,
    pub selected_paths: Vec<StoredPath>,
    pub deepest_open_column_directory: Option<StoredPath>,
    pub expanded_directories: Vec<StoredPath>,
    pub view_mode: StoredBrowserViewMode,
    pub back_stack: Vec<StoredPath>,
    pub forward_stack: Vec<StoredPath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBrowserPane {
    pub id: u64,
    pub tabs: Vec<StoredBrowserTab>,
    pub active_tab_id: u64,
    #[serde(default)]
    pub column_browser_viewport: StoredColumnBrowserViewport,
    pub column_viewports: Vec<StoredColumnViewport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBrowserSession {
    pub panes: Vec<StoredBrowserPane>,
    pub layout: StoredBrowserPaneLayout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_layout_without_portion_defaults_to_equal() {
        let layout: StoredBrowserPaneLayout = serde_json::from_str(
            r#"{"kind":"split","axis":"horizontal","first":0,"second":1,"active":0}"#,
        )
        .expect("legacy split layout should deserialize");

        assert_eq!(
            layout,
            StoredBrowserPaneLayout::Split {
                axis: StoredSplitAxis::Horizontal,
                first: 0,
                second: 1,
                active: 0,
                first_portion: 500,
            }
        );
    }
}
