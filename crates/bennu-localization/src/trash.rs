// 下沉调整：UiLanguage 原经 crate::config 引用 app-ui，现为本 crate 根项。
use crate::UiLanguage;

pub(super) fn refresh_failed(language: UiLanguage, error: &str) -> String {
    match language {
        UiLanguage::English => format!("Trash refresh failed: {error}"),
        UiLanguage::Chinese => format!("回收站刷新失败：{error}"),
    }
}

pub(super) fn additional_warning_count(language: UiLanguage, count: usize) -> String {
    match language {
        UiLanguage::English => format!("{count} more warning(s)"),
        UiLanguage::Chinese => format!("另有 {count} 条警告"),
    }
}

pub(super) fn warning_summary(language: UiLanguage, count: usize) -> String {
    match language {
        UiLanguage::English => format!("Trash loaded with {count} warning(s)"),
        UiLanguage::Chinese => format!("回收站已加载，但有 {count} 条警告"),
    }
}

pub(super) fn tracking_warning(language: UiLanguage, warning: &str) -> String {
    const PREFIX: &str = "Moved to Trash, but undo information could not be recorded.";
    let Some(detail) = warning.strip_prefix(PREFIX) else {
        return crate::translate(language, warning).into_owned();
    };
    format!("{}{}", crate::translate(language, PREFIX), detail)
}
