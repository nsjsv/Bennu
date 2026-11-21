//! 地址栏里打的字和路径之间的换算：提交、补全建议、补全写回都只走这里。主程序地址栏、
//! 文件选择器地址栏、超级框的路径模式共用，`~`、`..` 在哪里打都是一个意思。

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

/// 补全建议最多几条：浮层不超出屏幕。
pub const PATH_SUGGESTION_LIMIT: usize = 6;

/// 停笔这么久才去读目录给建议，边打边读白费。
pub const PATH_SUGGESTION_INPUT_STABILIZATION_DELAY: Duration = Duration::from_millis(120);

/// 打的字指的路径：`~`、`~/…` 是主目录，相对路径接在 `current_dir` 后面，`.` 和 `..`
/// 按字面化简（不看磁盘、不跟符号链接，和 shell 里 `cd ..` 一样；根上的 `..` 还是根）。
/// 从终端、浏览器复制来的本机 `file:///…` 地址解开百分号转义照样认。空的，或者是相对路径
/// 却没有当前文件夹时为 None。
pub fn typed_path(text: &str, current_dir: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("file://") {
        // 别的机器上的（`file://host/…`）不是这台电脑的路径，不认。
        let path = url::Url::parse(text).ok()?.to_file_path().ok()?;
        return Some(without_dots(&path));
    }
    let path = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ if Path::new(text).is_absolute() => PathBuf::from(text),
        _ => current_dir?.join(text),
    };
    Some(without_dots(&path))
}

fn without_dots(path: &Path) -> PathBuf {
    let mut plain = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                plain.pop();
            }
            component => plain.push(component),
        }
    }
    plain
}

/// 补全选中的目录写回输入框：结尾带分隔符，接着补就是在它里面找。
pub fn completed_path_text(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

/// 最后一个 `/` 之前是要读的目录，之后是名字开头；只列目录，按名字排，最多
/// `PATH_SUGGESTION_LIMIT` 条。
pub async fn load_path_suggestions(
    draft: String,
    current_dir: PathBuf,
    home: PathBuf,
) -> Vec<PathBuf> {
    let Some((directory, prefix)) = suggestion_directory_and_prefix(&draft, &current_dir, &home)
    else {
        return Vec::new();
    };
    let Ok(mut reader) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };

    let mut suggestions = Vec::new();
    while let Ok(Some(dir_entry)) = reader.next_entry().await {
        if !file_name_starts_with(&dir_entry.file_name(), &prefix) {
            continue;
        }
        if dir_entry
            .file_type()
            .await
            .is_ok_and(|file_type| file_type.is_dir())
        {
            suggestions.push(dir_entry.path());
        }
    }

    suggestions.sort_unstable();
    suggestions.truncate(PATH_SUGGESTION_LIMIT);
    suggestions
}

fn suggestion_directory_and_prefix(
    draft: &str,
    current_dir: &Path,
    home: &Path,
) -> Option<(PathBuf, String)> {
    let draft = draft.trim();
    if draft.is_empty() {
        return None;
    }
    let (directory, prefix) = match draft.rfind('/') {
        Some(slash) => (
            typed_path(&draft[..=slash], Some(current_dir), home)?,
            &draft[slash + 1..],
        ),
        None => (current_dir.to_path_buf(), draft),
    };
    Some((directory, prefix.to_owned()))
}

// 非 UTF-8 的名字按字节比：转成字符串会把坏字节换成 �，开头就对不上了。
#[cfg(unix)]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.as_bytes().starts_with(prefix.as_bytes())
}

#[cfg(not(unix))]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.to_string_lossy().starts_with(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/me";

    fn typed(text: &str, current_dir: Option<&str>) -> Option<PathBuf> {
        typed_path(text, current_dir.map(Path::new), Path::new(HOME))
    }

    #[test]
    fn home_relative_and_absolute_text() {
        assert_eq!(typed("~", None), Some(PathBuf::from(HOME)));
        assert_eq!(
            typed(" ~/下载 ", None),
            Some(PathBuf::from("/home/me/下载"))
        );
        assert_eq!(typed("~/", None), Some(PathBuf::from(HOME)));
        assert_eq!(typed("/tmp", None), Some(PathBuf::from("/tmp")));
        assert_eq!(
            typed("src", Some("/work")),
            Some(PathBuf::from("/work/src"))
        );
        // 没有当前文件夹（回收站、搜索结果）时相对路径无处可接。
        assert_eq!(typed("src", None), None);
        assert_eq!(typed("   ", Some("/work")), None);
        // 只有单独的 `~` 或 `~/` 开头才是主目录，`~x` 是当前文件夹里叫这个名字的条目。
        assert_eq!(typed("~x", Some("/work")), Some(PathBuf::from("/work/~x")));
    }

    #[test]
    fn local_file_uris_are_paths() {
        assert_eq!(
            typed("file:///home/me/My%20Files/%E4%B8%8B%E8%BD%BD", None),
            Some(PathBuf::from("/home/me/My Files/下载"))
        );
        assert_eq!(
            typed("file://localhost/tmp/a/../b", None),
            Some(PathBuf::from("/tmp/b"))
        );
        assert_eq!(typed("file://server/share", None), None);
        // 没有 `//` 的是名字里带冒号的条目。
        assert_eq!(
            typed("file:notes", Some("/work")),
            Some(PathBuf::from("/work/file:notes"))
        );
    }

    #[test]
    fn dots_are_resolved_by_the_text_alone() {
        assert_eq!(typed("..", Some("/a/b")), Some(PathBuf::from("/a")));
        assert_eq!(typed("../c", Some("/a/b")), Some(PathBuf::from("/a/c")));
        assert_eq!(typed(".", Some("/a/b")), Some(PathBuf::from("/a/b")));
        assert_eq!(typed("./c/./d/", Some("/a")), Some(PathBuf::from("/a/c/d")));
        assert_eq!(typed("/a/b/../../c", None), Some(PathBuf::from("/c")));
        assert_eq!(typed("~/..", None), Some(PathBuf::from("/home")));
        assert_eq!(typed("/..", None), Some(PathBuf::from("/")));
        assert_eq!(typed("../../../..", Some("/a")), Some(PathBuf::from("/")));
    }

    #[test]
    fn suggestions_read_the_folder_before_the_last_slash() {
        let here = Path::new("/home/me/work");
        let home = Path::new(HOME);
        let split = |draft: &str| suggestion_directory_and_prefix(draft, here, home);

        assert_eq!(split(""), None);
        assert_eq!(split("   "), None);
        assert_eq!(split("doc"), Some((here.to_path_buf(), "doc".to_owned())));
        assert_eq!(
            split("/tmp/ma"),
            Some((PathBuf::from("/tmp"), "ma".to_owned()))
        );
        assert_eq!(
            split("sub/ma"),
            Some((PathBuf::from("/home/me/work/sub"), "ma".to_owned()))
        );
        assert_eq!(split("/tmp/"), Some((PathBuf::from("/tmp"), String::new())));
        assert_eq!(split("~/Do"), Some((home.to_path_buf(), "Do".to_owned())));
        assert_eq!(split("../"), Some((home.to_path_buf(), String::new())));
        assert_eq!(
            split("../work/../pi"),
            Some((home.to_path_buf(), "pi".to_owned()))
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
    async fn suggestions_list_only_folders_sorted_and_capped() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().to_path_buf();
        std::fs::create_dir(root.join("b-dir")).unwrap();
        std::fs::create_dir(root.join("a-dir")).unwrap();
        std::fs::write(root.join("a-file"), b"x").unwrap();
        for index in 0..PATH_SUGGESTION_LIMIT {
            std::fs::create_dir(root.join(format!("z-{index}"))).unwrap();
        }
        let suggest =
            |draft: &str| load_path_suggestions(draft.to_owned(), root.join("b-dir"), root.clone());

        assert!(suggest("").await.is_empty());
        assert_eq!(
            suggest("../a").await,
            vec![root.join("a-dir")],
            "files are not suggested"
        );
        assert_eq!(suggest("~/b").await, vec![root.join("b-dir")]);
        let all = suggest("~/").await;
        assert_eq!(all.len(), PATH_SUGGESTION_LIMIT);
        assert_eq!(all[..2], [root.join("a-dir"), root.join("b-dir")]);
    }

    #[test]
    fn completed_text_appends_separator_once() {
        assert_eq!(completed_path_text(Path::new("/tmp/docs")), "/tmp/docs/");
        assert_eq!(completed_path_text(Path::new("/")), "/");
    }
}
