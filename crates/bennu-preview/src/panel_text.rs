//! 面板文本构造：主程序与文件选择器共用。

use iced::widget::text;
use iced::Theme;

/// 只收 `String`：界面文案先经 `tr!` 取好，文件名等用户数据原样传入；
/// 写成 `readable_text("Settings")` 编译不过，不会漏翻译。
pub fn readable_text<'a>(content: String) -> iced::widget::Text<'a, Theme> {
    let needs_advanced_shaping = needs_advanced_text_shaping(&content);
    let label = text(content);

    if needs_advanced_shaping {
        label.shaping(iced::widget::text::Shaping::Advanced)
    } else {
        label
    }
}

/// 非 ASCII（中文文件名、中文文案）要 Advanced shaping，否则显示成方框。
fn needs_advanced_text_shaping(content: &str) -> bool {
    !content.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::needs_advanced_text_shaping;

    #[test]
    fn ascii_text_keeps_fast_shaping() {
        assert!(!needs_advanced_text_shaping("Bennu"));
    }

    #[test]
    fn chinese_text_uses_advanced_shaping() {
        assert!(needs_advanced_text_shaping("设置"));
    }
}
