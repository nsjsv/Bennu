pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Restart Index Service" => Some("重启索引服务"),
        "Gracefully stop and restart the managed service." => {
            Some("温和停止并重启系统管理的服务。")
        }
        "Force Restart" => Some("强制重启"),
        "Use only when the managed service is unresponsive." => Some("仅在服务无响应时使用。"),
        "Click Again to Force Restart" => Some("再次点击以强制重启"),
        "Current indexing work will stop. Index data and settings will be kept." => {
            Some("当前索引工作会中断，但索引数据和设置不会被删除。")
        }
        "Restarting index service..." => Some("正在重启索引服务..."),
        "Force restarting index service..." => Some("正在强制重启索引服务..."),
        "Index service restarted successfully." => Some("索引服务已重启。"),
        "Index service force restarted successfully." => Some("索引服务已强制重启。"),
        _ => None,
    };
    if let Some(translated) = exact {
        return Some(translated.to_owned());
    }
    if let Some(error) = text.strip_prefix("Could not restart index service: ") {
        return Some(format!("无法重启索引服务：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not force restart index service: ") {
        return Some(format!("无法强制重启索引服务：{error}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn translates_recovery_controls_and_outcomes() {
        for text in [
            "Restart Index Service",
            "Force Restart",
            "Click Again to Force Restart",
            "Current indexing work will stop. Index data and settings will be kept.",
            "Restarting index service...",
            "Index service force restarted successfully.",
        ] {
            assert_ne!(translate(text).as_deref(), Some(text));
        }
        assert_eq!(
            translate("Could not force restart index service: permission denied").as_deref(),
            Some("无法强制重启索引服务：permission denied")
        );
    }
}
