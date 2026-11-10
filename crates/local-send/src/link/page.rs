//! 手机上打开的网页：文字按 Bennu 当前语言填进模板（同官方 `/i18n.json` 的做法，只是
//! 直接写进页面，少一次请求）。

use bennu_localization::{current_language, tr, UiLanguage};

use crate::qr::html_escape;

const UPLOAD_TEMPLATE: &str = include_str!("upload.html");

pub(super) fn upload(computer_name: &str, base_path: &str) -> String {
    let strings = serde_json::json!({
        "base": base_path,
        "connecting": tr!("Connecting..."),
        "fileOf": tr!("file %1 of %2", "%1", "%2"),
        "sent": tr!("Sent %1 file(s). You can send more.", "%1"),
        "canceled": tr!("The computer canceled this transfer."),
        "stopped": tr!("The computer has stopped receiving."),
        "unreachable": tr!("Cannot reach the computer."),
        "failed": tr!("Could not send %1: %2", "%1", "%2"),
        "notStarted": tr!("Could not start sending: %1", "%1"),
    });
    // 脚本里的 JSON：`</` 会提前结束 <script>，文件名之类的内容不能借此注入。
    let strings = strings.to_string().replace("</", "<\\/");
    UPLOAD_TEMPLATE
        .replace("{{LANG}}", language_tag())
        .replace("{{TITLE}}", &title(computer_name))
        .replace(
            "{{NOSCRIPT}}",
            &html_escape(&tr!("This page needs JavaScript.")),
        )
        .replace("{{CHOOSE}}", &html_escape(&tr!("Choose files...")))
        .replace("{{DROP_HINT}}", &html_escape(&tr!("Or drop files here.")))
        .replace("{{STRINGS}}", &strings)
}

/// 停下之后再打开链接看到的页面。
pub(super) fn stopped(computer_name: &str) -> String {
    format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title></head><body style=\"font-family: system-ui, sans-serif; \
         text-align: center\"><h1>{title}</h1><p>{}</p></body></html>",
        language_tag(),
        html_escape(&tr!("The computer has stopped receiving.")),
        title = title(computer_name),
    )
}

fn title(computer_name: &str) -> String {
    html_escape(&tr!("Send files to %1", computer_name))
}

fn language_tag() -> &'static str {
    match current_language() {
        UiLanguage::English => "en",
        UiLanguage::Chinese => "zh-CN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_break_out_of_the_page() {
        let page = upload("<b>PC</b></script>", "/token");

        assert!(!page.contains("<b>PC"));
        assert!(page.contains("&lt;b&gt;PC"));
        assert!(!page.contains("{{"));
    }
}
