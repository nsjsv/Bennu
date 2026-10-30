//! 预览文案用的格式化。文件大小只有 `bennu_localization` 一份实现，这里转出去，
//! 预览各处的既有调用路径不变。

pub use bennu_localization::{format_file_size, format_file_size_compact};

use bennu_localization::tr;
use file_core::FileKind;

/// 属性窗口与右侧信息区的"类型"一栏，绘制时按当前语言取。
pub fn file_kind_label(kind: FileKind) -> String {
    match kind {
        FileKind::Directory => tr!("Folder"),
        FileKind::File => tr!("File"),
        FileKind::Symlink => tr!("Symbolic Link"),
        FileKind::Other => tr!("Other"),
    }
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
