//! 设置这一行 JSON 的读写规则（参照 GNOME dconf 与 KDE KConfig）：
//! 读没有任何副作用，读不懂的项逐项回落默认值；写时保留本版本不认识的键，
//! 整份读不懂的旧数据先备份再覆盖。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

use crate::user_preferences::{StoredUserPreferences, USER_PREFERENCES_KEY};
use crate::{current_time_ms, StoreResult, TaskQueueStore};

/// 读出的设置，读不懂的部分已换成默认值。
#[derive(Debug, Clone, PartialEq)]
pub struct UserPreferencesRead {
    pub preferences: StoredUserPreferences,
    /// 原始载荷：有读不懂的部分时，主程序据此备份。
    pub raw_payload: String,
    pub unreadable: Option<UnreadablePreferences>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnreadablePreferences {
    /// 整份都读不懂（不是 JSON 对象）。
    Whole,
    /// 这些键读不懂、已用默认值；数组键表示其中有元素读不懂被丢掉。
    Keys(Vec<String>),
}

impl TaskQueueStore {
    /// `defaults` 是调用方自己的默认设置：读不懂的项、旧版本没有的项用它补。存储层
    /// 不知道程序的默认值（比如启动目录取主目录），所以由调用方给。
    pub fn read_user_preferences(
        &self,
        defaults: &StoredUserPreferences,
    ) -> StoreResult<Option<UserPreferencesRead>> {
        let connection = self.connection()?;
        Ok(read_payload(&connection)?
            .map(|raw_payload| decode_user_preferences(raw_payload, defaults)))
    }

    pub fn replace_user_preferences(&self, preferences: &StoredUserPreferences) -> StoreResult<()> {
        let connection = self.connection()?;
        write_user_preferences(&connection, self.db_path(), preferences)
    }

    /// 主程序启动修复：先把读不懂的原始载荷另存成数据库所在目录下的文件，再写入
    /// 修复后的设置（原始载荷是 JSON 对象时，其中本版本不认识的键照样保留）。备份
    /// 失败返回 Err，什么都不写；备份成功后写回的结果单独交给调用方。
    pub fn replace_unreadable_user_preferences(
        &self,
        raw_payload: &str,
        repaired: &StoredUserPreferences,
    ) -> StoreResult<UnreadablePreferencesBackup> {
        let path = back_up_payload(self.db_path(), raw_payload)?;
        Ok(UnreadablePreferencesBackup {
            path,
            write_back: write_repaired_payload(self, raw_payload, repaired),
        })
    }
}

/// 读不懂的设置已备份。写回失败时库里仍是原数据，但它已有备份，之后照常保存
/// 覆盖它不会丢东西，所以写回失败不像备份失败那样要拦住保存。
#[derive(Debug)]
pub struct UnreadablePreferencesBackup {
    pub path: PathBuf,
    pub write_back: StoreResult<()>,
}

fn write_repaired_payload(
    store: &TaskQueueStore,
    raw_payload: &str,
    repaired: &StoredUserPreferences,
) -> StoreResult<()> {
    let mut payload = payload_object(repaired)?;
    if let Ok(Value::Object(original)) = serde_json::from_str::<Value>(raw_payload) {
        keep_unknown_keys(&mut payload, original);
    }
    upsert_payload(&store.connection()?, payload)
}

fn read_payload(connection: &Connection) -> StoreResult<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT payload_json FROM user_preferences WHERE preference_key = ?1",
            rusqlite::params![USER_PREFERENCES_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

fn decode_user_preferences(
    raw_payload: String,
    defaults: &StoredUserPreferences,
) -> UserPreferencesRead {
    let (preferences, unreadable) = match serde_json::from_str(&raw_payload) {
        Ok(preferences) => (preferences, None),
        Err(_) => salvage(&raw_payload, defaults),
    };
    UserPreferencesRead {
        preferences,
        raw_payload,
        unreadable,
    }
}

/// 整体解析失败时逐项挽救：以调用方的默认设置序列化出的对象为底，每个已知键单独
/// 代入试解析，能解析的保留；数组键整体不行时逐个元素试（一条坏掉的网络连接不连累
/// 其余）。缺失的键用默认值、不算读不懂（旧版本没有这一项）；不认识的键在这里
/// 忽略，写时原样保留。
fn salvage(
    raw_payload: &str,
    defaults: &StoredUserPreferences,
) -> (StoredUserPreferences, Option<UnreadablePreferences>) {
    let Ok(Value::Object(stored)) = serde_json::from_str::<Value>(raw_payload) else {
        return (defaults.clone(), Some(UnreadablePreferences::Whole));
    };
    let mut merged = payload_object(defaults).expect("preferences serialize to JSON");
    let mut unreadable = Vec::new();
    for (key, value) in stored {
        if !merged.contains_key(&key) {
            continue;
        }
        if accepts(&merged, &key, &value) {
            merged.insert(key, value);
            continue;
        }
        if let Value::Array(items) = value {
            let readable_items = items
                .into_iter()
                .filter(|item| accepts(&merged, &key, &Value::Array(vec![item.clone()])))
                .collect();
            merged.insert(key.clone(), Value::Array(readable_items));
        }
        unreadable.push(key);
    }
    let preferences = serde_json::from_value(Value::Object(merged))
        .expect("every kept key was accepted on its own");
    unreadable.sort();
    (
        preferences,
        (!unreadable.is_empty()).then_some(UnreadablePreferences::Keys(unreadable)),
    )
}

fn accepts(base: &Map<String, Value>, key: &str, value: &Value) -> bool {
    let mut candidate = base.clone();
    candidate.insert(key.to_owned(), value.clone());
    serde_json::from_value::<StoredUserPreferences>(Value::Object(candidate)).is_ok()
}

/// 写设置的唯一入口（单独保存与关闭程序时的事务共用）。现有一行里本版本不认识的
/// 键原样并入新载荷，降级使用不会丢掉新版本写下的设置；现有一行整份读不懂时，
/// 没有先备份就不覆盖它。
pub(crate) fn write_user_preferences(
    connection: &Connection,
    db_path: &Path,
    preferences: &StoredUserPreferences,
) -> StoreResult<()> {
    let mut payload = payload_object(preferences)?;
    if let Some(existing) = read_payload(connection)? {
        match serde_json::from_str::<Value>(&existing) {
            Ok(Value::Object(existing)) => keep_unknown_keys(&mut payload, existing),
            _ => {
                back_up_payload(db_path, &existing)?;
            }
        }
    }
    upsert_payload(connection, payload)
}

fn payload_object(preferences: &StoredUserPreferences) -> StoreResult<Map<String, Value>> {
    match serde_json::to_value(preferences)? {
        Value::Object(object) => Ok(object),
        _ => unreachable!("StoredUserPreferences serializes to a JSON object"),
    }
}

/// 本版本序列化出的载荷包含全部已知键，原载荷里多出来的就是不认识的键。
fn keep_unknown_keys(payload: &mut Map<String, Value>, original: Map<String, Value>) {
    for (key, value) in original {
        payload.entry(key).or_insert(value);
    }
}

fn upsert_payload(connection: &Connection, payload: Map<String, Value>) -> StoreResult<()> {
    connection.execute(
        "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(preference_key) DO UPDATE SET
             payload_json = excluded.payload_json,
             updated_at_ms = excluded.updated_at_ms",
        rusqlite::params![
            USER_PREFERENCES_KEY,
            Value::Object(payload).to_string(),
            current_time_ms()
        ],
    )?;
    Ok(())
}

/// 备份写成一个新文件（`create_new`，不覆盖已有备份），写完落盘。文件名用 Unix
/// 毫秒：本包不引入日期库，提示里给出完整路径。
fn back_up_payload(db_path: &Path, raw_payload: &str) -> StoreResult<PathBuf> {
    let directory = db_path.parent().unwrap_or_else(|| Path::new(""));
    let path = directory.join(format!("user-preferences-{}.json", current_time_ms()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(raw_payload.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

#[cfg(test)]
#[path = "preferences_payload_tests.rs"]
mod tests;
