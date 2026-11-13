//! 桌面的最近使用列表（`recently-used.xbel`，文件管理器、编辑器、归档工具等打开
//! 文件时都会写）。搜索排序把它当作「常用」信号：用户主动打开过的文件比只是被改过的
//! 更可能是要找的那个。它只是加分项，读不到时搜索照常进行。

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// 正常的列表只有几十 KB；超过这么大说明文件异常，不读。
const MAX_RECENT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECENT_ENTRIES: usize = 10_000;
/// 写列表的一方用 1970 年附近的时间表示「没有」，不能当成很久以前用过。
const EARLIEST_REAL_USE_MS: i64 = 86_400_000;

/// 路径 → 最后使用时间（Unix 毫秒）。
pub(crate) type LastUsed = Arc<HashMap<PathBuf, i64>>;

#[derive(Default)]
pub(crate) struct RecentFiles {
    source: Option<PathBuf>,
    loaded: Mutex<LoadedRecentFiles>,
}

#[derive(Default)]
struct LoadedRecentFiles {
    signature: Option<(SystemTime, u64)>,
    last_used: LastUsed,
}

impl RecentFiles {
    pub(crate) fn at(source: PathBuf) -> Self {
        Self {
            source: Some(source),
            loaded: Mutex::default(),
        }
    }

    /// 当前列表。每次只 `stat` 一次，文件的修改时间或大小变了才重读。
    pub(crate) fn last_used(&self) -> LastUsed {
        let Some(source) = &self.source else {
            return LastUsed::default();
        };
        let signature = std::fs::metadata(source)
            .ok()
            .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
        let mut loaded = self.loaded.lock().expect("recent files mutex poisoned");
        if loaded.signature != signature {
            loaded.last_used = Arc::new(match signature {
                Some((_, length)) if length <= MAX_RECENT_FILE_BYTES => std::fs::read(source)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|text| parse_recently_used(&text))
                    .unwrap_or_default(),
                _ => HashMap::new(),
            });
            loaded.signature = signature;
        }
        Arc::clone(&loaded.last_used)
    }
}

/// 解析 xbel。坏掉的文件只保留出错前读到的条目。
fn parse_recently_used(text: &str) -> HashMap<PathBuf, i64> {
    let mut last_used = HashMap::new();
    let mut reader = Reader::from_str(text);
    while last_used.len() < MAX_RECENT_ENTRIES {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"bookmark" =>
            {
                if let Some((path, used_ms)) = bookmark_last_use(&element) {
                    let entry = last_used.entry(path).or_insert(used_ms);
                    *entry = (*entry).max(used_ms);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    last_used
}

fn bookmark_last_use(element: &BytesStart<'_>) -> Option<(PathBuf, i64)> {
    let mut path = None;
    let mut used_ms = None::<i64>;
    for attribute in element.attributes().flatten() {
        let value = attribute.unescape_value().ok()?;
        match attribute.key.local_name().as_ref() {
            b"href" => path = file_uri_path(&value),
            b"added" | b"modified" | b"visited" => {
                if let Some(time_ms) =
                    utc_timestamp_ms(&value).filter(|ms| *ms >= EARLIEST_REAL_USE_MS)
                {
                    used_ms = Some(used_ms.map_or(time_ms, |current| current.max(time_ms)));
                }
            }
            _ => {}
        }
    }
    Some((path?, used_ms?))
}

/// `file:///…` 或 `file://localhost/…` → 原始字节路径；其他协议不是本地文件。
fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri
        .strip_prefix("file://")
        .map(|rest| rest.strip_prefix("localhost").unwrap_or(rest))
        .filter(|rest| rest.starts_with('/'))?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

/// 列表里的 ISO 8601 UTC 时间：`2024-11-10T07:17:23Z` 或带小数秒。
fn utc_timestamp_ms(value: &str) -> Option<i64> {
    let (date, time) = value.strip_suffix('Z')?.split_once('T')?;
    let mut date_parts = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
    );
    let (whole_seconds, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut time_parts = whole_seconds.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        time_parts.next()?.ok()?,
        time_parts.next()?.ok()?,
        time_parts.next()?.ok()?,
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let milliseconds = format!("{fraction:0<3}")[..3].parse::<i64>().ok()?;
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some(seconds * 1_000 + milliseconds)
}

/// 公历日期到 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[path = "recent_files_tests.rs"]
mod tests;
