//! 界面语言、文案表与文件大小写法。主程序、预览视图组、文件选择器共用；文案按
//! 消息 ID（英文句式）写成 `tr!("Copy to %1", name)`，见 `message`。

use std::sync::atomic::{AtomicU8, Ordering};

mod catalog;
mod message;

pub use message::MessageId;

/// 界面语言。语言设置（UiLanguageSetting 与 TOML 存取）属 app-ui 存储域，经 re-export
/// 同源引用本枚举，避免两侧各持一份定义产生分叉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(UiLanguage::English.as_u8());

/// 全应用唯一的文件大小写法，同 KDE `KFormat::formatByteSize` 的默认：1024 进制、
/// IEC 单位、一位小数（"14.3 MiB"）；不到 1 KiB 写字节数（"123 B"）。放在本 crate
/// 是因为它最底层，主窗口、文件选择器、传输窗口和扫码页都能用到。
pub fn format_file_size(bytes: u64) -> String {
    format_size_with_units(bytes, ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"], " ")
}

/// 宽度受限的短标签（分组索引栏）：数值口径与 [`format_file_size`] 相同，单位缩成
/// 一个字母、不留空格（"14.3M"）。
pub fn format_file_size_compact(bytes: u64) -> String {
    format_size_with_units(bytes, ["B", "K", "M", "G", "T", "P", "E"], "")
}

fn format_size_with_units(bytes: u64, units: [&str; 7], separator: &str) -> String {
    if bytes < 1024 {
        return format!("{bytes}{separator}{}", units[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1}{separator}{}", units[unit])
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
