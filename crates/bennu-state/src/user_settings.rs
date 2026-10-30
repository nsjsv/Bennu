//! 用户设置：整份设置文档（读写规则见 `preferences_payload`）与多栏视图的栏宽。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::database::StateDatabase;
use crate::StoreResult;

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS ui_column_view_preferences (
    preference_key TEXT PRIMARY KEY,
    value_real REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS user_preferences (
    preference_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

const COLUMN_WIDTH_PREFERENCE_PREFIX: &str = "column_width.";
/// 所有栏宽 override 共享的参考内容宽度。read_column_widths 只解析数字后缀,
/// 该键天然被忽略;replace_column_widths 负责整体重写时一并维护它。
const COLUMN_WIDTH_REFERENCE_CONTENT_WIDTH_KEY: &str = "column_width.reference_content_width";

#[derive(Debug, Clone)]
pub struct UserSettingsStore {
    pub(crate) database: StateDatabase,
}

impl From<StateDatabase> for UserSettingsStore {
    fn from(database: StateDatabase) -> Self {
        Self { database }
    }
}

impl UserSettingsStore {
    pub fn new(db_path: impl AsRef<Path>) -> StoreResult<Self> {
        StateDatabase::open(db_path).map(Self::from)
    }

    pub fn db_path(&self) -> &Path {
        self.database.path()
    }

    pub fn read_column_widths(&self) -> StoreResult<HashMap<usize, f64>> {
        read_indexed_column_widths(&self.database.read())
    }

    /// 读取栏宽 override 的参考内容宽度;从未写入过时返回 None(旧数据兜底)。
    pub fn read_column_width_reference_content_width(&self) -> StoreResult<Option<f64>> {
        let connection = self.database.read();
        let value = connection
            .query_row(
                "SELECT value_real
                 FROM ui_column_view_preferences
                 WHERE preference_key = ?1",
                params![COLUMN_WIDTH_REFERENCE_CONTENT_WIDTH_KEY],
                |row| row.get::<_, f64>(0),
            )
            .optional()?;
        Ok(value.filter(|width| width.is_finite() && *width > 0.0))
    }

    pub fn replace_column_widths(
        &self,
        widths: HashMap<usize, f64>,
        reference_content_width: Option<f64>,
    ) -> StoreResult<()> {
        let connection = self.database.write();
        let indexed_pattern = format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}%");
        connection.execute(
            "DELETE FROM ui_column_view_preferences
             WHERE preference_key LIKE ?1",
            params![indexed_pattern],
        )?;

        let mut widths = widths
            .into_iter()
            .filter(|(_, width)| width.is_finite())
            .collect::<Vec<_>>();
        widths.sort_by_key(|(column_index, _)| *column_index);
        for (column_index, width) in widths {
            connection.execute(
                "INSERT INTO ui_column_view_preferences (preference_key, value_real)
                 VALUES (?1, ?2)",
                params![column_width_preference_key(column_index), width],
            )?;
        }
        if let Some(reference_content_width) =
            reference_content_width.filter(|width| width.is_finite() && *width > 0.0)
        {
            connection.execute(
                "INSERT INTO ui_column_view_preferences (preference_key, value_real)
                 VALUES (?1, ?2)",
                params![
                    COLUMN_WIDTH_REFERENCE_CONTENT_WIDTH_KEY,
                    reference_content_width
                ],
            )?;
        }
        Ok(())
    }
}

fn read_indexed_column_widths(connection: &Connection) -> StoreResult<HashMap<usize, f64>> {
    let indexed_pattern = format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}%");
    let mut statement = connection.prepare(
        "SELECT preference_key, value_real
         FROM ui_column_view_preferences
         WHERE preference_key LIKE ?1
         ORDER BY preference_key ASC",
    )?;
    let rows = statement.query_map(params![indexed_pattern], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;

    let mut widths = HashMap::new();
    for row in rows {
        let (key, width) = row?;
        let Some(index) = key
            .strip_prefix(COLUMN_WIDTH_PREFERENCE_PREFIX)
            .and_then(|index| index.parse::<usize>().ok())
        else {
            continue;
        };
        if width.is_finite() {
            widths.insert(index, width);
        }
    }
    Ok(widths)
}

fn column_width_preference_key(column_index: usize) -> String {
    format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}{column_index}")
}
