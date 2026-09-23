//! 预览加载/超限文案使用的容量格式化。自 app-ui 的 formatting.rs 下沉
//! （预览命令层引用；app-ui re-export 维持 35 处既有调用路径）。

pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    format_file_size_with_unit_table(bytes, &UNITS, " ")
}

/// 音视频/动图预览的播放时长文案（H:MM:SS / M:SS）。自 app-ui
/// formatting.rs 下沉（音频/视频/动图面板消费；app-ui re-export）。
pub fn format_duration(duration: std::time::Duration) -> String {
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

/// 紧凑变体:数值压缩口径与 [`format_file_size`] 完全一致(1024 进制、
/// <10 保一位小数),仅缩写单位并去掉空格,供分组索引栏等宽度受限的
/// 短标签使用。两个出口共用同一压缩核心,数值不会各算各的。
pub fn format_file_size_compact(bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::format_duration;
    use std::time::Duration;

    #[test]
    fn formats_audio_duration() {
        assert_eq!(format_duration(Duration::from_secs(65)), "1:05");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1:01:01");
    }
}
