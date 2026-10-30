//! 写进日志或给用户看的错误细节：长度受限，URI 里的凭据与形如 `password=…` 的赋值被抹掉。
//! 文件操作队列、主程序的日志页和各处错误提示共用同一份，凭据不会因为某条路径忘了脱敏而落盘。

use std::sync::OnceLock;

use regex::Regex;

pub const LOG_MESSAGE_CHAR_LIMIT: usize = 1_000;

pub fn bounded_application_log_message(message: &str) -> String {
    let mut characters = message.chars();
    let prefix = characters
        .by_ref()
        .take(LOG_MESSAGE_CHAR_LIMIT)
        .collect::<String>();
    if characters.next().is_none() {
        return prefix;
    }

    let mut truncated = prefix
        .chars()
        .take(LOG_MESSAGE_CHAR_LIMIT - 1)
        .collect::<String>();
    truncated.push('…');
    truncated
}

pub fn sanitized_application_log_detail(detail: &str) -> String {
    static URI_USERINFO: OnceLock<Regex> = OnceLock::new();
    static SENSITIVE_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();

    let uri_userinfo = URI_USERINFO.get_or_init(|| {
        Regex::new(r"(?P<scheme>[A-Za-z][A-Za-z0-9+.-]*://)[^/\s?#@]*@")
            .expect("credential URI regex must compile")
    });
    let sensitive_assignment = SENSITIVE_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(password|passwd|token|secret|authorization|cookie)\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("sensitive assignment regex must compile")
    });
    let redacted = uri_userinfo.replace_all(detail, "${scheme}[redacted]@");
    let redacted = sensitive_assignment.replace_all(&redacted, "$1=[redacted]");
    bounded_application_log_message(&redacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_length_is_bounded_with_an_ellipsis() {
        let long_message = "界".repeat(LOG_MESSAGE_CHAR_LIMIT + 1);
        let bounded = bounded_application_log_message(&long_message);

        assert_eq!(bounded.chars().count(), LOG_MESSAGE_CHAR_LIMIT);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn persisted_log_detail_redacts_uri_and_assignment_credentials() {
        let detail = "mount smb://alice:secret@example.test/share failed; token=abc123";
        let sanitized = sanitized_application_log_detail(detail);

        assert_eq!(
            sanitized,
            "mount smb://[redacted]@example.test/share failed; token=[redacted]"
        );
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("abc123"));
    }
}
