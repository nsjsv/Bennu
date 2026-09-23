//! 面板文本构造辅助：自 app-ui typography.rs 下沉的单源实现。翻译走
//! bennu-localization::translate_current（文案表唯一来源，禁止复制文案），
//! 非 ASCII 文本启用 Advanced shaping 的规则与原实现逐字节一致；
//! app-ui 的 typography 模块 re-export 本模块，45 处调用路径零改动。

use iced::widget::text;
use iced::Theme;

pub fn readable_text<'a>(content: impl ReadableTextContent) -> iced::widget::Text<'a, Theme> {
    let content = if content.should_localize() {
        bennu_localization::translate_current(&content.into_text_content())
    } else {
        content.into_text_content()
    };
    let needs_advanced_shaping = needs_advanced_text_shaping(&content);
    let label = text(content);

    if needs_advanced_shaping {
        label.shaping(iced::widget::text::Shaping::Advanced)
    } else {
        label
    }
}

pub trait ReadableTextContent {
    fn should_localize(&self) -> bool;
    fn into_text_content(self) -> String;
}

impl ReadableTextContent for String {
    fn should_localize(&self) -> bool {
        false
    }

    fn into_text_content(self) -> String {
        self
    }
}

impl ReadableTextContent for &str {
    fn should_localize(&self) -> bool {
        true
    }

    fn into_text_content(self) -> String {
        self.to_owned()
    }
}

impl ReadableTextContent for &String {
    fn should_localize(&self) -> bool {
        false
    }

    fn into_text_content(self) -> String {
        self.clone()
    }
}

impl ReadableTextContent for std::borrow::Cow<'_, str> {
    fn should_localize(&self) -> bool {
        false
    }

    fn into_text_content(self) -> String {
        self.into_owned()
    }
}

pub fn localized_text<'a>(content: impl ReadableTextContent) -> iced::widget::Text<'a, Theme> {
    readable_text(bennu_localization::translate_current(
        &content.into_text_content(),
    ))
}

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
