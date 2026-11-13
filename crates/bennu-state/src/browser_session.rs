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

/// 列表视图主区域的滚动位置。与多栏每一栏的滚动位置分开存：主区域可能是回收站，
/// 没有目录可当键。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StoredListViewport {
    pub offset_y: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredColumnBrowserViewport {
    pub offset_x: f32,
    pub width: f32,
}

/// 标签保存时所在的位置。回收站不是目录，不存路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredBrowserLocation {
    Directory { path: StoredPath },
    Trash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StoredBrowserTabWire")]
pub struct StoredBrowserTab {
    pub id: u64,
    pub location: StoredBrowserLocation,
    pub selected: Option<StoredPath>,
    pub selected_paths: Vec<StoredPath>,
    pub deepest_open_column_directory: Option<StoredPath>,
    pub expanded_directories: Vec<StoredPath>,
    pub view_mode: StoredBrowserViewMode,
    pub back_stack: Vec<StoredPath>,
    pub forward_stack: Vec<StoredPath>,
}

/// 读取时兼容 0.4.0 及更早的会话：那时存的是目录加回收站标志，回收站标签的目录是一个
/// 伪路径。读进来时回收站标志为真就是回收站，伪路径直接丢掉；写出只写新格式。
#[derive(Deserialize)]
struct StoredBrowserTabWire {
    id: u64,
    #[serde(default)]
    location: Option<StoredBrowserLocation>,
    #[serde(default)]
    directory: Option<StoredPath>,
    #[serde(default)]
    is_trash_view: bool,
    selected: Option<StoredPath>,
    selected_paths: Vec<StoredPath>,
    deepest_open_column_directory: Option<StoredPath>,
    expanded_directories: Vec<StoredPath>,
    view_mode: StoredBrowserViewMode,
    back_stack: Vec<StoredPath>,
    forward_stack: Vec<StoredPath>,
}

impl TryFrom<StoredBrowserTabWire> for StoredBrowserTab {
    type Error = &'static str;

    fn try_from(wire: StoredBrowserTabWire) -> Result<Self, Self::Error> {
        let location = match (wire.location, wire.is_trash_view, wire.directory) {
            (Some(location), _, _) => location,
            (None, true, _) => StoredBrowserLocation::Trash,
            (None, false, Some(path)) => StoredBrowserLocation::Directory { path },
            (None, false, None) => return Err("browser tab has no location"),
        };
        Ok(Self {
            id: wire.id,
            location,
            selected: wire.selected,
            selected_paths: wire.selected_paths,
            deepest_open_column_directory: wire.deepest_open_column_directory,
            expanded_directories: wire.expanded_directories,
            view_mode: wire.view_mode,
            back_stack: wire.back_stack,
            forward_stack: wire.forward_stack,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBrowserPane {
    pub id: u64,
    pub tabs: Vec<StoredBrowserTab>,
    pub active_tab_id: u64,
    #[serde(default)]
    pub column_browser_viewport: StoredColumnBrowserViewport,
    pub column_viewports: Vec<StoredColumnViewport>,
    #[serde(default)]
    pub list_viewport: Option<StoredListViewport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBrowserSession {
    pub panes: Vec<StoredBrowserPane>,
    pub layout: StoredBrowserPaneLayout,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

    fn legacy_tab_json(directory_bytes: &[u8], is_trash_view: bool) -> String {
        format!(
            r#"{{"id":3,"directory":{{"encoding":"unix_bytes","bytes":{directory_bytes:?}}},"is_trash_view":{is_trash_view},"selected":null,"selected_paths":[],"deepest_open_column_directory":null,"expanded_directories":[],"view_mode":"list","back_stack":[],"forward_stack":[]}}"#
        )
    }

    #[test]
    fn a_legacy_trash_tab_is_restored_as_the_trash_without_its_fake_path() {
        // 0.4.0 及更早把回收站标签存成「伪路径 + 回收站标志」。
        let fake_trash_path = [116, 114, 97, 115, 104, 58, 47, 47, 47];
        let tab: StoredBrowserTab =
            serde_json::from_str(&legacy_tab_json(&fake_trash_path, true)).unwrap();
        assert_eq!(tab.location, StoredBrowserLocation::Trash);
    }

    #[test]
    fn a_legacy_directory_tab_keeps_its_directory() {
        let tab: StoredBrowserTab = serde_json::from_str(&legacy_tab_json(b"/srv", false)).unwrap();
        assert_eq!(
            tab.location,
            StoredBrowserLocation::Directory {
                path: StoredPath::from_path(Path::new("/srv"))
            }
        );
    }

    #[test]
    fn a_tab_is_written_with_its_location_and_read_back_unchanged() {
        for location in [
            StoredBrowserLocation::Trash,
            StoredBrowserLocation::Directory {
                path: StoredPath::from_path(Path::new("/home/me")),
            },
        ] {
            let tab = StoredBrowserTab {
                id: 1,
                location,
                selected: None,
                selected_paths: Vec::new(),
                deepest_open_column_directory: None,
                expanded_directories: Vec::new(),
                view_mode: StoredBrowserViewMode::List,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
            };
            let json = serde_json::to_string(&tab).unwrap();
            assert!(!json.contains("is_trash_view"));
            assert_eq!(
                serde_json::from_str::<StoredBrowserTab>(&json).unwrap(),
                tab
            );
        }
    }

    #[test]
    fn a_tab_without_any_location_is_rejected() {
        let json = r#"{"id":3,"is_trash_view":false,"selected":null,"selected_paths":[],"deepest_open_column_directory":null,"expanded_directories":[],"view_mode":"list","back_stack":[],"forward_stack":[]}"#;
        assert!(serde_json::from_str::<StoredBrowserTab>(json).is_err());
    }
}
