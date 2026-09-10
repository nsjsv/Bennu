pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Content indexing" => "内容索引",
        "Status overview" => "状态总览",
        "Index progress" => "索引进度",
        "Recent issues" => "近期问题",
        "Service recovery" => "服务恢复",
        "Checking the search service. Status will update automatically." => {
            "正在检查搜索服务，状态会自动更新。"
        }
        "Service" => "服务",
        "Indexed search" => "索引搜索",
        "Index maintenance" => "索引维护",
        "Waiting for service status" => "正在等待服务状态",
        "Not initialized" => "尚未初始化",
        "Starting" => "正在启动",
        "Running" => "运行正常",
        "Needs attention" => "需要注意",
        "Unavailable" => "不可用",
        "Stopping" => "正在停止",
        "Available" => "可用",
        "Temporarily unavailable" => "暂时不可用",
        "Healthy" => "正常",
        "Checking" => "正在检查",
        "Rechecking" => "正在重新确认",
        "Index progress will appear after the service responds." => {
            "服务响应后会显示索引进度。"
        }
        "The index has not been initialized yet." => "索引尚未初始化。",
        "Index phase" => "索引阶段",
        "Indexed items" => "已索引项目",
        "Checked items" => "已检查项目",
        "Changed items" => "有变化的项目",
        "Scanned items" => "已扫描项目",
        "Current location" => "当前位置",
        "Pending changes" => "待应用变更",
        "Checking existing files" => "正在检查现有文件",
        "Scanning files" => "正在扫描文件",
        "Applying changes" => "正在应用变更",
        "Up to date" => "已是最新",
        "Failed" => "失败",
        "No search service issues detected during this app session." => {
            "本次应用运行期间未检测到搜索服务问题。"
        }
        "Current issue" => "当前问题",
        "Recovered" => "已恢复",
        "Occurred once" => "发生 1 次",
        "First" => "首次",
        "Latest" => "最近",
        "Show technical details" => "查看技术详情",
        "Hide technical details" => "收起技术详情",
        "Copy technical details" => "复制技术详情",
        "Index service restart failed. See Recent Issues for technical details." => {
            "索引服务重启失败，请在“近期问题”中查看技术详情。"
        }
        "Search service response is delayed" => "搜索服务响应较慢",
        "Cannot reach the search service" => "无法连接搜索服务",
        "Search service is changing" => "搜索服务正在切换状态",
        "Cannot verify the search service" => "无法验证搜索服务",
        "Search service connection is inconsistent" => "搜索服务连接状态不一致",
        "Search service components do not match" => "搜索服务组件版本不匹配",
        "Could not restart the search service" => "无法重启搜索服务",
        "Search service needs attention" => "搜索服务需要注意",
        "Search service stopped working" => "搜索服务已停止工作",
        "Indexed search is temporarily unavailable" => "索引搜索暂时不可用",
        "Index update failed" => "索引更新失败",
        "Index maintenance needs attention" => "索引维护需要注意",
        "Index maintenance stopped" => "索引维护已停止",
        "Wait for automatic rechecking. If the problem continues, restart the index service." => {
            "请等待自动重新检查；如果问题持续存在，请重启索引服务。"
        }
        "Review the technical details. Restart the index service if the problem continues." => {
            "请查看技术详情；如果问题持续存在，请重启索引服务。"
        }
        "Restart the index service to establish a verified connection." => {
            "请重启索引服务以重新建立可信连接。"
        }
        "Reinstall the search components from the current Bennu package." => {
            "请从当前文件管理器软件包重新安装搜索组件。"
        }
        "Review the technical details. Use force restart only if the service remains unresponsive." => {
            "请查看技术详情；仅当服务持续无响应时使用强制重启。"
        }
        "Search may use a slower fallback while the service recovers." => {
            "服务恢复期间，搜索可能使用速度较慢的备用方式。"
        }
        "Restart the index service. Existing index data and settings will be kept." => {
            "请重启索引服务；现有索引数据和设置会保留。"
        }
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_owned());
    }

    if let Some(timestamp) = text.strip_prefix("Last confirmed: ") {
        return Some(format!("最近确认：{timestamp}"));
    }
    if let Some(count) = text
        .strip_prefix("Occurred ")
        .and_then(|body| body.strip_suffix(" times"))
    {
        return Some(format!("发生 {count} 次"));
    }
    if let Some(body) = text
        .strip_prefix("Connection is unstable. Rechecking automatically (")
        .and_then(|body| body.strip_suffix("); showing the last confirmed status."))
    {
        return Some(format!(
            "连接出现波动，正在自动重新确认（{body}）；当前显示最近一次确认的数据。"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn translates_status_groups_and_dynamic_diagnostics() {
        assert_eq!(translate("Status overview").as_deref(), Some("状态总览"));
        assert_eq!(translate("Occurred 4 times").as_deref(), Some("发生 4 次"));
        assert_eq!(
            translate("Connection is unstable. Rechecking automatically (2/3); showing the last confirmed status.").as_deref(),
            Some("连接出现波动，正在自动重新确认（2/3）；当前显示最近一次确认的数据。")
        );
    }
}
