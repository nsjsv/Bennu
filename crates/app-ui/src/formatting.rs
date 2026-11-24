use std::fmt::Display;
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone, Utc};

use crate::model::BrowserLocation;

// 文件大小只有 bennu_localization 一份实现；re-export 维持 crate::formatting::*
// 既有调用路径。
pub(crate) use bennu_localization::{format_file_size, format_file_size_compact};

/// "1 item" / "3 items"：确认框、拖放、通知、目录摘要共用。
pub(crate) fn item_count_label(count: usize) -> String {
    if count == 1 {
        tr!("1 item")
    } else {
        tr!("%1 items", count)
    }
}

/// 列表、大图、多栏三种视图的空内容提示。搜索结果为空时说清是在搜、搜错了、还没输入，
/// 还是真的没搜到。
pub(crate) fn empty_location_message(location: &BrowserLocation) -> String {
    match location {
        BrowserLocation::Directory(_) => tr!("No items"),
        BrowserLocation::Trash => tr!("Trash is empty"),
        BrowserLocation::Search(search) => {
            if let Some(failure) = &search.window.failure {
                failure.clone()
            } else if search.window.is_loading {
                tr!("Searching...")
            } else if search.run.active_query.is_none() {
                tr!("Type to search")
            } else {
                tr!("No search results")
            }
        }
    }
}

/// 显示用的时间：本机时区（跟着系统的 `TZ`、`/etc/localtime` 走，系统改了时区不用重启）。
pub(crate) fn format_system_time(time: SystemTime) -> String {
    format_time_in(time, &Local)
}

/// 某个时刻在本机时区里是哪一天：按日期分组、过零点重分都按它。
pub(crate) fn local_date(time: SystemTime) -> chrono::NaiveDate {
    date_in(time, &Local)
}

pub(crate) fn date_in<Zone: TimeZone>(time: SystemTime, zone: &Zone) -> chrono::NaiveDate {
    DateTime::<Utc>::from(time).with_timezone(zone).date_naive()
}

fn format_time_in<Zone: TimeZone>(time: SystemTime, zone: &Zone) -> String
where
    Zone::Offset: Display,
{
    DateTime::<Utc>::from(time)
        .with_timezone(zone)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

// 中间省略纯字符串算法已随测量文本 widget 一并下沉 bennu-theme 共享；
// re-export 保持 crate 内引用路径不变。
pub(crate) use bennu_theme::measured_text::format_middle_ellipsized_text;

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn times_are_written_in_the_given_zone() {
        for (time, expected) in [
            (UNIX_EPOCH, "1970-01-01 00:00:00"),
            (UNIX_EPOCH - Duration::from_secs(1), "1969-12-31 23:59:59"),
            (
                UNIX_EPOCH + Duration::from_secs(951_782_400),
                "2000-02-29 00:00:00",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(4_107_542_400),
                "2100-03-01 00:00:00",
            ),
        ] {
            assert_eq!(format_time_in(time, &Utc), expected);
        }
        // 2024-01-01 00:00 UTC 在上海是早上 8 点，在纽约还是前一天晚上。
        let new_year = UNIX_EPOCH + Duration::from_secs(1_704_067_200);
        assert_eq!(
            format_time_in(new_year, &chrono_tz::Asia::Shanghai),
            "2024-01-01 08:00:00"
        );
        assert_eq!(
            format_time_in(new_year, &chrono_tz::America::New_York),
            "2023-12-31 19:00:00"
        );
        assert_eq!(
            date_in(new_year, &chrono_tz::America::New_York),
            chrono::NaiveDate::from_ymd_opt(2023, 12, 31).unwrap()
        );
    }

    #[test]
    fn the_shown_time_is_in_the_system_zone() {
        let time = UNIX_EPOCH + Duration::from_secs(1_704_067_200);
        assert_eq!(format_system_time(time), format_time_in(time, &Local));
        assert!(!format_system_time(time).contains("UTC"));
    }
}
