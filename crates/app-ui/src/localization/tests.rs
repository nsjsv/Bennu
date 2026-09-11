use super::*;

#[test]
fn detects_chinese_from_lang_prefix() {
    let language = detect_system_language_with(|key| match key {
        "LANG" => Some("zh_CN.UTF-8".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::Chinese);
}

#[test]
fn detects_chinese_from_language_fallback() {
    let language = detect_system_language_with(|key| match key {
        "LANGUAGE" => Some("en_US:zh_CN".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::Chinese);
}

#[test]
fn defaults_to_english_for_non_chinese_locale() {
    let language = detect_system_language_with(|key| match key {
        "LANG" => Some("en_US.UTF-8".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::English);
}

#[test]
fn translates_known_static_text() {
    for text in [
        "Search",
        "Any type",
        "Any content",
        "Spreadsheets",
        "More",
        "Yesterday",
        "Past year",
        "Name & content",
        "Name only",
        "Current folder",
        "All indexed locations",
        "Indexed locations",
        "Excluded locations",
        "Loading indexed locations...",
        "Home is indexed by default. Add another location here.",
        "Loading excluded locations...",
        "No additional locations are excluded.",
        "Excluded",
        "Applying search location changes...",
        "Retry",
        "Waiting for status",
        "Remove location",
        "Absolute path",
        "Choose directory",
        "Select a search location",
        "Select",
        "the selected location is not a local directory",
        "Search locations must be absolute paths",
        "Home",
        "Reset filters",
        "Content indexing is unavailable; matching file names only.",
        "Past 30 days",
        "Close search",
        "Scroll to load more results",
        "Logs",
        "Appearance",
        "About",
        "Version",
        "License",
        "GitHub repository",
        "Report issues or browse the source code in your browser.",
        "Bennu is free software released under the GNU General Public License, version 3 or later.",
        "Files",
        "Startup location",
        "Service and Index",
        "Index File Contents",
        "Discrete GPU",
        "Audio preview",
        "Text preview is not ready",
        "No pending conflicts",
        "File",
        "Symbolic Link",
        "No compression",
        "Open Terminal Here",
        "Task",
        "Items",
        "Status",
        "Progress",
        "Indeterminate",
        "Processing...",
        "Copy Details",
        "Clear",
        "Clear Finished",
    ] {
        assert_ne!(translate(UiLanguage::Chinese, text), text);
    }
}

#[test]
fn translates_known_dynamic_text() {
    for (text, expected) in [
        (
            "Progress: 12,345 items scanned",
            "进度：已扫描 12,345 个项目",
        ),
        ("Indexed: 98,765 items", "已索引：98,765 个项目"),
        (
            "Index maintenance: Degraded (watch gap)",
            "索引维护：已降级（watch gap）",
        ),
        ("Search in all indexed locations", "搜索范围：所有索引位置"),
        (
            "Storage changed: configured search root now resolves to a different filesystem",
            "存储已变化：配置的搜索根目录现在指向另一文件系统",
        ),
        ("Search in /home/test", "搜索范围：/home/test"),
        (
            "Partial results after inspecting 50000 entries",
            "部分结果，已检查 50000 个条目",
        ),
        (
            "Search endpoint unavailable: socket closed",
            "搜索端点不可用：socket closed",
        ),
        (
            "Maximum content extraction: 8 MiB",
            "最大内容提取大小：8 MiB",
        ),
        ("Volume 75%", "音量 75%"),
        // 拖拽动作胶囊:动作词+落点目录名。
        ("Move to project", "移动到project"),
        ("Copy to 备份", "复制到备份"),
        ("Create link to /mnt/data", "创建链接到/mnt/data"),
        ("Move to Trash", "移到回收站"),
    ] {
        assert_eq!(translate(UiLanguage::Chinese, text), expected);
    }
}
