use std::time::{Duration, SystemTime};

use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

const UTC_TIMESTAMP_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");

pub(crate) fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    format_file_size_with_unit_table(bytes, &UNITS, " ")
}

/// 紧凑变体:数值压缩口径与 [`format_file_size`] 完全一致(1024 进制、
/// <10 保一位小数),仅缩写单位并去掉空格,供分组索引栏等宽度受限的
/// 短标签使用。两个出口共用同一压缩核心,数值不会各算各的。
pub(crate) fn format_file_size_compact(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    format_file_size_with_unit_table(bytes, &UNITS, "")
}

fn format_file_size_with_unit_table(bytes: u64, units: &[&str; 6], separator: &str) -> String {
    if bytes < 1024 {
        return format!("{bytes}{separator}{}", units[0]);
    }

    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if value < 10.0 {
        format!("{value:.1}{separator}{}", units[unit])
    } else {
        format!("{value:.0}{separator}{}", units[unit])
    }
}

pub(crate) fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub(crate) fn format_system_time(time: SystemTime) -> String {
    OffsetDateTime::from(time)
        .format(UTC_TIMESTAMP_FORMAT)
        .expect("the static UTC timestamp format is valid")
}

pub(crate) fn format_middle_ellipsized_text(content: &str, max_chars: usize) -> String {
    const MARKER: &str = "...";

    if content.len() <= max_chars {
        return content.to_owned();
    }

    if content.is_ascii() {
        return format_ascii_middle_ellipsized_text(content, max_chars);
    }

    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    if max_chars <= MARKER.len() {
        return MARKER.chars().take(max_chars).collect();
    }

    let visible_chars = max_chars - MARKER.len();
    let start_chars = visible_chars.div_ceil(2);
    let end_chars = visible_chars / 2;
    let start: String = content.chars().take(start_chars).collect();
    let end: String = content
        .chars()
        .rev()
        .take(end_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!("{start}{MARKER}{end}")
}

fn format_ascii_middle_ellipsized_text(content: &str, max_chars: usize) -> String {
    const MARKER: &str = "...";

    if max_chars <= MARKER.len() {
        return MARKER[..max_chars].to_owned();
    }

    let visible_chars = max_chars - MARKER.len();
    let start_chars = visible_chars.div_ceil(2);
    let end_chars = visible_chars / 2;
    let mut text = String::with_capacity(max_chars);
    text.push_str(&content[..start_chars]);
    text.push_str(MARKER);
    text.push_str(&content[content.len() - end_chars..]);
    text
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{format_duration, format_middle_ellipsized_text, format_system_time};

    #[test]
    fn keeps_short_text() {
        assert_eq!(format_middle_ellipsized_text("Documents", 16), "Documents");
    }

    #[test]
    fn trims_long_text_from_middle() {
        assert_eq!(
            format_middle_ellipsized_text("very-long-directory-name", 12),
            "very-...name"
        );
    }

    #[test]
    fn formats_audio_duration() {
        assert_eq!(format_duration(Duration::from_secs(65)), "1:05");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1:01:01");
    }

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
