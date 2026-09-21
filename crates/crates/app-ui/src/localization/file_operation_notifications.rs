pub(super) fn translate(text: &str) -> Option<String> {
    if let Some(operation) = text.strip_suffix(" completed") {
        return Some(format!("{}已完成", translated_operation(operation)?));
    }
    if let Some(operation) = text.strip_suffix(" failed") {
        return Some(format!("{}失败", translated_operation(operation)?));
    }
    None
}

fn translated_operation(operation: &str) -> Option<&'static str> {
    super::exact_translation(operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_file_operation_notification_summaries() {
        assert_eq!(translate("Copy completed").as_deref(), Some("复制已完成"));
        assert_eq!(
            translate("Extract Archive failed").as_deref(),
            Some("解压归档失败")
        );
        assert_eq!(translate("Unknown completed"), None);
    }
}
