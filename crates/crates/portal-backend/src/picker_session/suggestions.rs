//! 地址栏路径补全数据源：语义照搬主软件 `commands.rs` 的
//! `load_path_suggestions` / `suggestion_directory_and_prefix` /
//! `file_name_starts_with`——仅目录、字节前缀匹配、排序、截断 6 条。
//! 纯异步函数供 main 层 `Task::perform` 调用；会话侧只发请求与防陈旧
//! 回填，不碰文件系统。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 补全建议条数上限：与主软件 PATH_SUGGESTION_LIMIT 同值，浮层不超屏。
pub(crate) const PATH_SUGGESTION_LIMIT: usize = 6;

/// 输入防抖时长：停笔 120ms 才发起目录读取（主软件同值）。会话产出
/// `SessionEffect::StabilizeAddressInput`，main 层翻译时消费此常量。
pub(crate) const PATH_SUGGESTION_INPUT_STABILIZATION_DELAY: Duration = Duration::from_millis(120);

pub(crate) async fn load_path_suggestions(input: String, current_dir: PathBuf) -> Vec<PathBuf> {
    let Some((directory, prefix)) = suggestion_directory_and_prefix(&input, &current_dir) else {
        return Vec::new();
    };
    let mut reader = match tokio::fs::read_dir(directory).await {
        Ok(reader) => reader,
        Err(_) => return Vec::new(),
    };

    let mut suggestions = Vec::new();
    while let Ok(Some(dir_entry)) = reader.next_entry().await {
        let file_name = dir_entry.file_name();
        if !file_name_starts_with(&file_name, &prefix) {
            continue;
        }

        let Ok(file_type) = dir_entry.file_type().await else {
            continue;
        };
        if file_type.is_dir() {
            suggestions.push(dir_entry.path());
        }
    }

    suggestions.sort_unstable();
    suggestions.truncate(PATH_SUGGESTION_LIMIT);
    suggestions
}

// 非 UTF-8 文件名必须按字节比较：lossy 转换会把无效字节替换成 �，
// 前缀判断随之失真（主软件 unix 分支同因）。
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(unix)]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.as_bytes().starts_with(prefix.as_bytes())
}

#[cfg(not(unix))]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.to_string_lossy().starts_with(prefix)
}

/// 草稿 → (建议读取目录, 文件名前缀)：尾随分隔符 = 前缀空（列出目录
/// 全部子目录）；否则取最后一段为前缀、其父为目录。
pub(crate) fn suggestion_directory_and_prefix(
    input: &str,
    current_dir: &Path,
) -> Option<(PathBuf, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let raw_path = PathBuf::from(trimmed);
    let path = if raw_path.is_absolute() {
        raw_path
    } else {
        current_dir.join(raw_path)
    };

    if trimmed.ends_with(std::path::MAIN_SEPARATOR) {
        return Some((path, String::new()));
    }

    let prefix = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let directory = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| current_dir.to_path_buf());

    Some((directory, prefix))
}

/// 补全写入草稿时补尾分隔符：连续 Tab 补全的每一级都以分隔符收尾，
/// 下一次补全的读取目录即定位到刚选中的目录（主软件
/// completed_path_text 同构）。
pub(crate) fn completed_path_text(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_and_prefix_resolves_relative_and_absolute_drafts() {
        let current_dir = Path::new("/home/user");

        assert_eq!(suggestion_directory_and_prefix("", current_dir), None);
        assert_eq!(suggestion_directory_and_prefix("   ", current_dir), None);
        assert_eq!(
            suggestion_directory_and_prefix("doc", current_dir),
            Some((PathBuf::from("/home/user"), "doc".to_string()))
        );
        assert_eq!(
            suggestion_directory_and_prefix("/tmp/ma", current_dir),
            Some((PathBuf::from("/tmp"), "ma".to_string()))
        );
        assert_eq!(
            suggestion_directory_and_prefix("sub/ma", current_dir),
            Some((PathBuf::from("/home/user/sub"), "ma".to_string()))
        );
        // 尾随分隔符 = 前缀空：列出该目录全部子目录。
        assert_eq!(
            suggestion_directory_and_prefix("/tmp/", current_dir),
            Some((PathBuf::from("/tmp"), String::new()))
        );
    }

    #[test]
    fn byte_prefix_matching_covers_empty_and_partial_names() {
        assert!(file_name_starts_with(std::ffi::OsStr::new("abc"), ""));
        assert!(file_name_starts_with(std::ffi::OsStr::new("abc"), "ab"));
        assert!(!file_name_starts_with(std::ffi::OsStr::new("abc"), "ABC"));
        assert!(!file_name_starts_with(std::ffi::OsStr::new("bc"), "abc"));
    }

    #[tokio::test]
    async fn suggestions_keep_directories_only_and_sort() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("b-dir")).unwrap();
        std::fs::create_dir(workspace.path().join("a-dir")).unwrap();
        std::fs::write(workspace.path().join("a-file"), b"x").unwrap();
        std::fs::create_dir(workspace.path().join("other")).unwrap();

        let suggestions =
            load_path_suggestions(String::new(), workspace.path().to_path_buf()).await;
        // 空草稿不发起读取。
        assert!(suggestions.is_empty());

        let suggestions = load_path_suggestions(
            workspace.path().to_string_lossy().into_owned() + "/",
            workspace.path().to_path_buf(),
        )
        .await;
        assert_eq!(
            suggestions,
            vec![
                workspace.path().join("a-dir"),
                workspace.path().join("b-dir"),
                workspace.path().join("other"),
            ]
        );
    }

    #[tokio::test]
    async fn suggestions_are_truncated_to_limit() {
        let workspace = tempfile::tempdir().unwrap();
        for index in 0..(PATH_SUGGESTION_LIMIT + 2) {
            std::fs::create_dir(workspace.path().join(format!("dir-{index:02}"))).unwrap();
        }

        let suggestions = load_path_suggestions(
            workspace.path().to_string_lossy().into_owned() + "/",
            workspace.path().to_path_buf(),
        )
        .await;

        assert_eq!(suggestions.len(), PATH_SUGGESTION_LIMIT);
    }

    #[test]
    fn completed_text_appends_separator_once() {
        assert_eq!(
            completed_path_text(Path::new("/tmp/docs")),
            format!("/tmp/docs{}", std::path::MAIN_SEPARATOR)
        );
        // 根路径已带分隔符，不能重复。
        assert_eq!(completed_path_text(Path::new("/")), "/");
    }
}
