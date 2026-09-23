pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Index status: Not initialized" => "索引状态：未初始化",
        "Index status: Starting" => "索引状态：正在启动",
        "Index status: Indexing" => "索引状态：正在索引",
        "Index status: Checking" => "索引状态：正在检查",
        "Index status: Crawling" => "索引状态：正在扫描",
        "Index status: Applying" => "索引状态：正在应用",
        "Index status: Complete" => "索引状态：已完成",
        "Index status: Failed" => "索引状态：失败",
        "Index maintenance: Healthy" => "索引维护：正常",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_owned());
    }

    for (prefix, suffix, translated_prefix, translated_suffix) in [
        ("Progress: ", " items scanned", "进度：已扫描 ", " 个项目"),
        ("Checked: ", " items", "已检查：", " 个项目"),
        ("Changed: ", " items", "有变化：", " 个项目"),
        ("Scanned: ", " items", "已扫描：", " 个项目"),
        ("Pending changes: ", "", "待应用变更：", ""),
        ("Indexed: ", " items", "已索引：", " 个项目"),
    ] {
        if let Some(value) = text
            .strip_prefix(prefix)
            .and_then(|body| body.strip_suffix(suffix))
        {
            return Some(format!("{translated_prefix}{value}{translated_suffix}"));
        }
    }
    if let Some(scope) = text.strip_prefix("Scope: ") {
        return Some(format!("范围：{scope}"));
    }
    if let Some(error) = text.strip_prefix("Index error: ") {
        return Some(format!("索引错误：{error}"));
    }
    if let Some(error) = text
        .strip_prefix("Index maintenance: Degraded (")
        .and_then(|body| body.strip_suffix(')'))
    {
        return Some(format!("索引维护：已降级（{error}）"));
    }
    if let Some(error) = text
        .strip_prefix("Index maintenance: Error (")
        .and_then(|body| body.strip_suffix(')'))
    {
        return Some(format!("索引维护：错误（{error}）"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn translates_every_index_phase_and_detail_shape() {
        for (text, expected) in [
            ("Index status: Checking", "索引状态：正在检查"),
            ("Checked: 12 items", "已检查：12 个项目"),
            ("Changed: 3 items", "有变化：3 个项目"),
            ("Index status: Crawling", "索引状态：正在扫描"),
            ("Scanned: 456 items", "已扫描：456 个项目"),
            ("Scope: /home/user", "范围：/home/user"),
            ("Index status: Applying", "索引状态：正在应用"),
            ("Pending changes: 17", "待应用变更：17"),
        ] {
            assert_eq!(translate(text).as_deref(), Some(expected));
        }
    }
}
