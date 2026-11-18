//! 界面语言、文案表与文件大小写法。主程序、预览视图组、文件选择器共用；文案按
//! 消息 ID（英文句式）写成 `tr!("Copy to %1", name)`，见 `message`。

use std::sync::atomic::{AtomicU8, Ordering};

mod catalog;
mod file_size;
mod message;

pub use file_size::{
    format_file_size, format_file_size_compact, format_file_size_exact, parse_file_size,
    FileSizeInputError, BYTES_PER_KIB,
};
pub use message::MessageId;

/// 界面语言。语言设置（UiLanguageSetting 与 TOML 存取）属 app-ui 存储域，经 re-export
/// 同源引用本枚举，避免两侧各持一份定义产生分叉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiLanguage {
    English,
    Chinese,
}

impl UiLanguage {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::English => 0,
            Self::Chinese => 1,
        }
    }

    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Chinese,
            _ => Self::English,
        }
    }

    /// 系统数据（`.desktop` 的 `Name[..]`、类型库的 `xml:lang`）里这种语言的标签，先具体后笼统。
    /// 旧版类型库写 `zh_CN`，新版写 `zh-Hans-CN`，都要认。英文是空表：不带标签的那条就是英文。
    pub const fn locale_tags(self) -> &'static [&'static str] {
        match self {
            Self::English => &[],
            Self::Chinese => &["zh_CN", "zh-Hans-CN", "zh-Hans", "zh"],
        }
    }
}

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(UiLanguage::English.as_u8());

/// 音视频/动图预览的播放时长文案（H:MM:SS / M:SS），纯数字不随语言变化。
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

pub fn set_current_language(language: UiLanguage) {
    CURRENT_LANGUAGE.store(language.as_u8(), Ordering::Relaxed);
}

pub fn current_language() -> UiLanguage {
    UiLanguage::from_u8(CURRENT_LANGUAGE.load(Ordering::Relaxed))
}

pub fn detect_system_language() -> UiLanguage {
    detect_system_language_with(|key| std::env::var(key).ok())
}

fn detect_system_language_with<F>(lookup: F) -> UiLanguage
where
    F: Fn(&str) -> Option<String>,
{
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        let Some(value) = lookup(key) else {
            continue;
        };
        if locale_value_is_chinese(&value) {
            return UiLanguage::Chinese;
        }
    }
    UiLanguage::English
}

fn locale_value_is_chinese(value: &str) -> bool {
    value
        .split(':')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment
                .split(['.', '@'])
                .next()
                .unwrap_or(segment)
                .to_ascii_lowercase()
        })
        .any(|segment| segment == "zh" || segment.starts_with("zh_"))
}

#[cfg(test)]
mod tests;
