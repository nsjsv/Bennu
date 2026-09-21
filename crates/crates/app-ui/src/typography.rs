use iced::widget::text;
use iced::Theme;

pub(crate) fn readable_text<'a>(
    content: impl ReadableTextContent,
) -> iced::widget::Text<'a, Theme, iced::Renderer> {
    let content = if content.should_localize() {
        crate::localization::translate_current(&content.into_text_content())
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

pub(crate) trait ReadableTextContent {
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

pub(crate) fn localized_text<'a>(
    content: impl ReadableTextContent,
) -> iced::widget::Text<'a, Theme, iced::Renderer> {
    readable_text(crate::localization::translate_current(
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
