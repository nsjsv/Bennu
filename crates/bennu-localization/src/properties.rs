pub(super) fn translate(text: &str) -> Option<String> {
    for (source, translated) in [
        ("Folders", "文件夹"),
        ("Symbolic Links", "符号链接"),
        ("Other Items", "其它项目"),
        ("Multiple Item Types", "多种项目类型"),
        ("Multiple locations", "多个位置"),
        ("Mixed or unavailable", "值不一致或不可用"),
        (
            "Saving permissions for selected items...",
            "正在保存所选项目的权限...",
        ),
    ] {
        if text == source {
            return Some(translated.to_owned());
        }
    }

    if let Some(count) = text
        .strip_prefix("Updated permissions for ")
        .and_then(|body| body.strip_suffix(" items."))
        .and_then(|count| count.parse::<usize>().ok())
    {
        return Some(format!("已更新 {count} 个项目的权限。"));
    }

    let body = text.strip_prefix("Updated ")?;
    let (succeeded, body) = body.split_once(" items; ")?;
    let (failed, details) = body.split_once(" items failed. ")?;
    let succeeded = succeeded.parse::<usize>().ok()?;
    let failed = failed.parse::<usize>().ok()?;
    Some(format!(
        "已更新 {succeeded} 个项目；{failed} 个项目失败。{details}"
    ))
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn translates_aggregate_labels_and_permission_outcomes() {
        assert_eq!(translate("Multiple locations").as_deref(), Some("多个位置"));
        assert_eq!(
            translate("Updated permissions for 2 items.").as_deref(),
            Some("已更新 2 个项目的权限。")
        );
        assert_eq!(
            translate("Updated 1 items; 2 items failed. details").as_deref(),
            Some("已更新 1 个项目；2 个项目失败。details")
        );
    }
}
