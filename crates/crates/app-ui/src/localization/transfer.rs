//! 本地文件传输词条：菜单项、传输窗口、接收确认 Modal、设置页与通知。

use crate::config::UiLanguage;

pub(super) fn translate(text: &str) -> Option<String> {
    for (source, translated) in [
        // 菜单与窗口标题
        ("Send to Phone", "发送到手机"),
        ("Send to Phone - Bennu", "发送到手机 - Bennu"),
        // 设置分类与通用按钮
        ("Transfer", "传输"),
        ("Choose...", "选择..."),
        // 传输窗口
        ("Preparing download link...", "正在准备下载链接..."),
        ("Waiting for a phone to scan...", "等待手机扫码..."),
        ("Connected. Sending files...", "已连接，正在发送..."),
        ("Transfer finished", "传输完成"),
        ("Transfer timed out", "连接已超时关闭"),
        ("Could not start download service", "下载服务启动失败"),
        ("Nearby LocalSend devices", "附近的 LocalSend 设备"),
        ("No nearby devices found", "未发现附近设备"),
        ("Send", "发送"),
        (
            "Choose an address your phone can reach",
            "选择手机可访问的本机地址",
        ),
        ("Choose another address", "重新选择地址"),
        // 接收确认 Modal
        ("Incoming files", "收到文件"),
        ("Remember this device", "记住此设备"),
        ("Accept", "接受"),
        ("Decline", "拒绝"),
        // 设置页
        ("Receive directory", "接收目录"),
        ("Device name", "设备名称"),
        ("Trusted devices", "信任设备"),
        ("No trusted devices", "暂无信任设备"),
        (
            "Files received from trusted devices are accepted automatically",
            "信任设备发来的文件会自动接收",
        ),
        ("Remove", "移除"),
        ("Default: ~/Downloads", "默认：~/Downloads"),
        // 通知
        ("Files received", "文件接收完成"),
    ] {
        if text == source {
            return Some(translated.to_owned());
        }
    }

    // 动态拼接词条的前缀规则：构造方先用英文 key 拼串，这里按序还原。
    if let Some(url) = text.strip_prefix("Scan the code or open the link on your phone: ") {
        return Some(format!("在手机上扫码或打开链接：{url}"));
    }
    if let Some(alias) = text.strip_prefix("Send to ") {
        if let Some((alias, error)) = alias.split_once(" failed: ") {
            return Some(format!("发送到 {alias} 失败：{error}"));
        }
    }
    if let Some(alias) = text.strip_prefix("Sending to ") {
        if let Some(alias) = alias.strip_suffix("...") {
            return Some(format!("正在发送到 {alias}..."));
        }
    }
    if let Some(alias) = text.strip_prefix("Sent to ") {
        return Some(format!("已发送到 {alias}"));
    }
    if let Some(error) = text.strip_prefix("Could not start transfer service: ") {
        return Some(format!("传输服务启动失败：{error}"));
    }
    None
}

/// 传输窗口的动态进度行：文件序号 + 字节 + 速度（构造时产出成品文案）。
pub(super) fn qr_progress_line(
    language: UiLanguage,
    bytes: u64,
    speed_bytes_per_second: u64,
    finished: bool,
) -> String {
    let sent = crate::view::transfer_window::format_byte_size(bytes);
    if finished {
        return match language {
            UiLanguage::English => format!("Sent {sent}"),
            UiLanguage::Chinese => format!("已发送 {sent}"),
        };
    }
    let speed = crate::view::transfer_window::format_byte_size(speed_bytes_per_second);
    match language {
        UiLanguage::English => format!("Sent {sent} ({speed}/s)"),
        UiLanguage::Chinese => format!("已发送 {sent}（{speed}/秒）"),
    }
}

/// 直推动态行：正在发送第 i/N 个文件。
pub(super) fn direct_send_progress_line(
    language: UiLanguage,
    alias: &str,
    file_name: &str,
    file_index: usize,
    total_files: usize,
    bytes_sent: u64,
    file_size: u64,
) -> String {
    let sent = crate::view::transfer_window::format_byte_size(bytes_sent);
    let size = crate::view::transfer_window::format_byte_size(file_size);
    match language {
        UiLanguage::English => format!(
            "Sending to {alias}: {file_name} ({file_index}/{total_files}, {sent} of {size})"
        ),
        UiLanguage::Chinese => format!(
            "正在发送到 {alias}：{file_name}（第 {file_index}/{total_files} 个，已传 {sent}/共 {size}）"
        ),
    }
}

/// 接收确认正文：设备名 + 文件数（构造时产出成品文案）。
pub(super) fn incoming_transfer_summary(language: UiLanguage, alias: &str, count: usize) -> String {
    match language {
        UiLanguage::English => format!("{alias} wants to send {count} item(s)"),
        UiLanguage::Chinese => format!("{alias} 想要发送 {count} 个文件"),
    }
}

/// 接收完成通知正文：成功数 + 失败数。
pub(super) fn receive_finished_body(
    language: UiLanguage,
    received: usize,
    failed: usize,
) -> String {
    if failed == 0 {
        return match language {
            UiLanguage::English => format!("Received {received} file(s)."),
            UiLanguage::Chinese => format!("已接收 {received} 个文件。"),
        };
    }
    match language {
        UiLanguage::English => format!("Received {received} file(s), {failed} failed."),
        UiLanguage::Chinese => format!("已接收 {received} 个文件，{failed} 个失败。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::translate;

    #[test]
    fn translates_transfer_copy_to_chinese() {
        for (source, translated) in [
            ("Send to Phone", "发送到手机"),
            ("Transfer", "传输"),
            ("Incoming files", "收到文件"),
            ("Remember this device", "记住此设备"),
            ("Nearby LocalSend devices", "附近的 LocalSend 设备"),
        ] {
            assert_eq!(
                translate(UiLanguage::Chinese, source),
                translated,
                "missing entry: {source}"
            );
            // 英文原文必须原样返回（可读性回归）。
            assert_eq!(translate(UiLanguage::English, source), source);
        }
    }

    #[test]
    fn translates_dynamic_transfer_lines() {
        assert_eq!(
            translate(UiLanguage::Chinese, "Sent to Pixel"),
            "已发送到 Pixel"
        );
        assert_eq!(
            translate(UiLanguage::Chinese, "Sending to Pixel..."),
            "正在发送到 Pixel..."
        );
        assert_eq!(
            translate(
                UiLanguage::Chinese,
                "Send to Pixel failed: connection refused"
            ),
            "发送到 Pixel 失败：connection refused"
        );
        assert_eq!(
            translate(
                UiLanguage::Chinese,
                "Scan the code or open the link on your phone: http://192.168.1.5:1/token"
            ),
            "在手机上扫码或打开链接：http://192.168.1.5:1/token"
        );
        assert_eq!(
            translate(
                UiLanguage::Chinese,
                "Could not start transfer service: boom"
            ),
            "传输服务启动失败：boom"
        );
    }

    #[test]
    fn incoming_summary_and_receive_body_follow_language() {
        assert_eq!(
            incoming_transfer_summary(UiLanguage::Chinese, "Pixel", 3),
            "Pixel 想要发送 3 个文件"
        );
        assert_eq!(
            incoming_transfer_summary(UiLanguage::English, "Pixel", 1),
            "Pixel wants to send 1 item(s)"
        );
        assert_eq!(
            receive_finished_body(UiLanguage::Chinese, 2, 0),
            "已接收 2 个文件。"
        );
        assert_eq!(
            receive_finished_body(UiLanguage::Chinese, 2, 1),
            "已接收 2 个文件，1 个失败。"
        );
    }
}
