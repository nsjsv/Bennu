use std::time::SystemTime;

use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

const UTC_TIMESTAMP_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");

// 纯搬移：容量格式化（含紧凑变体与共享压缩核心）已下沉 bennu-preview
// （预览命令层超限文案引用）；re-export 维持 crate::formatting::* 既有
// 调用路径。
pub(crate) use bennu_preview::formatting::{format_file_size, format_file_size_compact};

pub(crate) fn format_system_time(time: SystemTime) -> String {
    OffsetDateTime::from(time)
        .format(UTC_TIMESTAMP_FORMAT)
        .expect("the static UTC timestamp format is valid")
}

// 中间省略纯字符串算法已随测量文本 widget 一并下沉 bennu-theme 共享；
// re-export 保持 crate 内引用路径不变。
pub(crate) use bennu_theme::measured_text::format_middle_ellipsized_text;

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::format_system_time;

    #[test]
    fn formats_system_time_as_utc_timestamp() {
        for (time, expected) in [
            (UNIX_EPOCH, "1970-01-01 00:00:00 UTC"),
            (
                UNIX_EPOCH - Duration::from_secs(1),
                "1969-12-31 23:59:59 UTC",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(951_782_400),
                "2000-02-29 00:00:00 UTC",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(1_704_067_200),
                "2024-01-01 00:00:00 UTC",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(4_107_542_400),
                "2100-03-01 00:00:00 UTC",
            ),
        ] {
            assert_eq!(format_system_time(time), expected);
        }
    }
}
