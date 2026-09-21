pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Indexed locations" => "索引位置",
        "Excluded locations" => "排除位置",
        "Loading indexed locations..." => "正在加载索引位置...",
        "Home is indexed by default. Add another location here." => {
            "主目录默认已建立索引，可在此添加其他位置。"
        }
        "Loading excluded locations..." => "正在加载排除位置...",
        "No additional locations are excluded." => "没有额外排除的位置。",
        "Excluded" => "已排除",
        "Applying search location changes..." => "正在应用搜索位置更改...",
        "Retry" => "重试",
        "Waiting for status" => "正在等待状态",
        "Remove location" => "移除位置",
        "Absolute path" => "绝对路径",
        "Choose directory" => "选择目录",
        "All indexed locations" => "所有索引位置",
        "Search in all indexed locations" => "搜索范围：所有索引位置",
        "Select a search location" => "选择搜索位置",
        "Select" => "选择",
        "the selected location is not a local directory" => "所选位置不是本地目录",
        "Search locations must be absolute paths" => "搜索位置必须使用绝对路径",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_owned());
    }

    if let Some((unavailable_message, error)) =
        text.split_once("; could not verify search exclusions: ")
    {
        return Some(format!(
            "{}；无法验证搜索排除项：{error}",
            translate_search_failure(unavailable_message)
        ));
    }
    if let Some((unavailable_message, message)) =
        text.split_once("; effective search path rules are invalid: ")
    {
        return Some(format!(
            "{}；有效搜索路径规则无效：{}",
            translate_search_failure(unavailable_message),
            translate_search_path_detail(message)
        ));
    }
    if let Some(message) = text.strip_prefix("Unavailable: ") {
        return Some(format!("不可用：{}", translate_search_path_detail(message)));
    }
    if let Some(message) = text.strip_prefix("Storage changed: ") {
        return Some(format!(
            "存储已变化：{}",
            translate_search_path_detail(message)
        ));
    }
    if let Some(message) = text.strip_prefix("invalid search service configuration: ") {
        return Some(format!(
            "搜索服务配置无效：{}",
            translate_search_path_detail(message)
        ));
    }
    None
}

fn translate_search_path_detail(message: &str) -> String {
    let exact = match message {
        "configured search root is not a no-follow directory" => {
            "配置的搜索根目录不是实体目录（不会跟随符号链接）"
        }
        "configured search root now resolves to a different filesystem" => {
            "配置的搜索根目录现在指向另一文件系统"
        }
        "storage identity has not been confirmed" => "尚未确认存储身份",
        "the Home search root is implicit and cannot be added again" => {
            "主目录是隐式搜索根目录，不能重复添加"
        }
        "unavailable search roots must be unique configured roots" => {
            "不可用的搜索根目录必须是唯一的已配置根目录"
        }
        "search path configuration revision is exhausted" => "搜索路径配置版本号已用尽",
        _ => "",
    };
    if !exact.is_empty() {
        return exact.to_owned();
    }

    if let Some(error) = message.strip_prefix("storage identity could not be verified: ") {
        return format!("无法验证存储身份：{error}");
    }
    if let Some(path) = message.strip_prefix("the same path cannot be indexed and excluded: ") {
        return format!("同一路径不能同时建立索引和排除：{path}");
    }
    if let Some(path) = message.strip_prefix("configured search path is not absolute: ") {
        return format!("配置的搜索路径不是绝对路径：{path}");
    }
    if let Some(path) = message.strip_prefix("configured search path escapes its root: ") {
        return format!("配置的搜索路径超出其根目录：{path}");
    }
    if let Some(limit) = message
        .strip_prefix("configured custom search roots exceed the ")
        .and_then(|body| body.strip_suffix(" entry limit"))
    {
        return format!("配置的自定义搜索根目录超过 {limit} 项上限");
    }
    if let Some(limit) = message
        .strip_prefix("configured search exclusions exceed the ")
        .and_then(|body| body.strip_suffix(" entry limit"))
    {
        return format!("配置的搜索排除目录超过 {limit} 项上限");
    }
    if let Some(limit) = message
        .strip_prefix("configured search paths exceed the ")
        .and_then(|body| body.strip_suffix(" byte total limit"))
    {
        return format!("配置的搜索路径总大小超过 {limit} 字节上限");
    }
    if let Some(body) = message.strip_prefix("configured search path exceeds the ") {
        if let Some((limit, path)) = body.split_once(" byte limit: ") {
            return format!("配置的搜索路径超过 {limit} 字节上限：{path}");
        }
    }
    if let Some(body) = message.strip_prefix("search path configuration revision conflict: ") {
        if let Some((expected, current)) = body
            .strip_prefix("expected ")
            .and_then(|body| body.split_once(", current "))
        {
            return format!("搜索路径配置版本冲突：预期 {expected}，当前 {current}");
        }
    }

    message.to_owned()
}

fn translate_search_failure(message: &str) -> String {
    match message {
        "index is starting" => "索引正在启动".to_owned(),
        "not ready" => "索引尚未就绪".to_owned(),
        _ => message
            .strip_prefix("Search root is unavailable: ")
            .map(|path| format!("搜索根目录不可用：{path}"))
            .or_else(|| {
                message
                    .strip_prefix("Indexed queries unavailable: ")
                    .map(|error| format!("索引查询不可用：{error}"))
            })
            .unwrap_or_else(|| message.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{translate, translate_search_failure, translate_search_path_detail};

    #[test]
    fn translates_search_path_labels_and_statuses() {
        assert_eq!(translate("Indexed locations").as_deref(), Some("索引位置"));
        assert_eq!(
            translate("Search in all indexed locations").as_deref(),
            Some("搜索范围：所有索引位置")
        );
        assert_eq!(
            translate(
                "Storage changed: configured search root now resolves to a different filesystem"
            )
            .as_deref(),
            Some("存储已变化：配置的搜索根目录现在指向另一文件系统")
        );
    }

    #[test]
    fn translates_every_search_path_detail_format() {
        let cases = [
            (
                "configured search root is not a no-follow directory",
                "配置的搜索根目录不是实体目录（不会跟随符号链接）",
            ),
            (
                "configured search root now resolves to a different filesystem",
                "配置的搜索根目录现在指向另一文件系统",
            ),
            (
                "storage identity has not been confirmed",
                "尚未确认存储身份",
            ),
            (
                "the Home search root is implicit and cannot be added again",
                "主目录是隐式搜索根目录，不能重复添加",
            ),
            (
                "unavailable search roots must be unique configured roots",
                "不可用的搜索根目录必须是唯一的已配置根目录",
            ),
            (
                "search path configuration revision is exhausted",
                "搜索路径配置版本号已用尽",
            ),
            (
                "storage identity could not be verified: mount table unavailable",
                "无法验证存储身份：mount table unavailable",
            ),
            (
                "the same path cannot be indexed and excluded: /mnt/archive",
                "同一路径不能同时建立索引和排除：/mnt/archive",
            ),
            (
                "configured search path is not absolute: archive",
                "配置的搜索路径不是绝对路径：archive",
            ),
            (
                "configured search path escapes its root: /mnt/archive",
                "配置的搜索路径超出其根目录：/mnt/archive",
            ),
            (
                "configured custom search roots exceed the 32 entry limit",
                "配置的自定义搜索根目录超过 32 项上限",
            ),
            (
                "configured search exclusions exceed the 128 entry limit",
                "配置的搜索排除目录超过 128 项上限",
            ),
            (
                "configured search paths exceed the 65536 byte total limit",
                "配置的搜索路径总大小超过 65536 字节上限",
            ),
            (
                "configured search path exceeds the 4096 byte limit: /mnt/archive",
                "配置的搜索路径超过 4096 字节上限：/mnt/archive",
            ),
            (
                "search path configuration revision conflict: expected 2, current 3",
                "搜索路径配置版本冲突：预期 2，当前 3",
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(translate_search_path_detail(source), expected, "{source}");
        }
    }

    #[test]
    fn translates_search_failure_formats_without_losing_details() {
        let cases = [
            ("index is starting", "索引正在启动"),
            ("not ready", "索引尚未就绪"),
            (
                "Search root is unavailable: /mnt/archive",
                "搜索根目录不可用：/mnt/archive",
            ),
            (
                "Indexed queries unavailable: socket closed",
                "索引查询不可用：socket closed",
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(translate_search_failure(source), expected, "{source}");
        }

        assert_eq!(
            translate("index is starting; could not verify search exclusions: socket closed")
                .as_deref(),
            Some("索引正在启动；无法验证搜索排除项：socket closed")
        );
        assert_eq!(
            translate("Search root is unavailable: /mnt/archive; effective search path rules are invalid: configured search path is not absolute: archive")
                .as_deref(),
            Some("搜索根目录不可用：/mnt/archive；有效搜索路径规则无效：配置的搜索路径不是绝对路径：archive")
        );
    }

    #[test]
    fn translates_configuration_errors_without_losing_paths() {
        assert_eq!(
            translate("invalid search service configuration: the same path cannot be indexed and excluded: /mnt/archive")
                .as_deref(),
            Some("搜索服务配置无效：同一路径不能同时建立索引和排除：/mnt/archive")
        );
        assert_eq!(
            translate(
                "Unavailable: storage identity could not be verified: mount table unavailable"
            )
            .as_deref(),
            Some("不可用：无法验证存储身份：mount table unavailable")
        );
    }
}
