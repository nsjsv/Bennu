pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Search - Bennu" => Some("搜索 - Bennu"),
        "Search" => Some("搜索"),
        "Searching..." => Some("正在搜索..."),
        "No search results" => Some("没有搜索结果"),
        "Any type" => Some("任意类型"),
        "Spreadsheets" => Some("表格"),
        "Video" => Some("视频"),
        "Images" => Some("图片"),
        "Text" => Some("文本"),
        "Documents" => Some("文档"),
        "Folders" => Some("文件夹"),
        "Audio" => Some("音频"),
        "PDF" => Some("PDF"),
        "Files" => Some("文件"),
        "Archives" => Some("压缩包"),
        "Links" => Some("链接"),
        "Custom" => Some("自定义"),
        "e.g. pdf, docx" => Some("例如 pdf、docx"),
        "Any content" => Some("任意内容"),
        "More" => Some("更多"),
        "Any time" => Some("任意时间"),
        "Today" => Some("今天"),
        "Yesterday" => Some("昨天"),
        "Past 7 days" => Some("近 7 天"),
        "Past 30 days" => Some("近 30 天"),
        "Past year" => Some("近一年"),
        "This year" => Some("今年"),
        "Name & content" => Some("文件名与内容"),
        "Name only" => Some("仅文件名"),
        "Regex" => Some("正则"),
        "Regex mode matches file names only." => Some("正则模式仅匹配文件名。"),
        "Current folder" => Some("当前文件夹"),
        "Reset filters" => Some("重置筛选"),
        "Content indexing is unavailable; matching file names only." => {
            Some("内容索引暂不可用，当前仅匹配文件名。")
        }
        "Clear search text" => Some("清空搜索文字"),
        "Recent searches" => Some("最近搜索"),
        "Clear search history" => Some("清空搜索历史"),
        "Remove from search history" => Some("从搜索历史移除"),
        "Close search" => Some("关闭搜索"),
        "Open folder" | "Open Containing Folder" => Some("打开所在目录"),
        "Scroll to load more results" => Some("滚动以加载更多结果"),
        "Search is available from a local folder" => Some("请从本地文件夹开始搜索"),
        "Search is unavailable for remote folders" => Some("远程文件夹暂不支持搜索"),
        "The result has no containing directory" => Some("该结果没有所在目录"),
        "Search Service" => Some("搜索服务"),
        "Service and Index" => Some("服务与索引"),
        "Search endpoint connected" => Some("搜索端点已连接"),
        "Search endpoint is starting…" => Some("搜索端点正在启动…"),
        "Service: starting" => Some("服务：正在启动"),
        "Service: ready" => Some("服务：就绪"),
        "Service: shutting down" => Some("服务：正在关闭"),
        "Indexed queries: available" => Some("索引查询：可用"),
        "Index File Contents" => Some("索引文件内容"),
        _ => None,
    };
    if let Some(translated) = exact {
        return Some(translated.to_owned());
    }

    if let Some(path) = text.strip_prefix("Search in ") {
        return Some(format!("搜索范围：{path}"));
    }
    if let Some(path) = text.strip_prefix("Search root is unavailable: ") {
        return Some(format!("搜索根目录不可用：{path}"));
    }
    if let Some(count) = text
        .strip_prefix("Partial results after inspecting ")
        .and_then(|body| body.strip_suffix(" entries"))
    {
        return Some(format!("部分结果，已检查 {count} 个条目"));
    }
    if let Some(path) = text.strip_prefix("Containing directory is unavailable: ") {
        return Some(format!("所在目录不可用：{path}"));
    }
    if let Some(error) = text.strip_prefix("Search endpoint unavailable: ") {
        return Some(format!("搜索端点不可用：{error}"));
    }
    if let Some(error) = text.strip_prefix("Indexed queries unavailable: ") {
        return Some(format!("索引查询不可用：{error}"));
    }
    None
}
