//! 路径模式和新建模式里，框里的字指的是什么。只看字，不读盘：路径在不在由后台检查，
//! 名字有没有被占用由调用方按已经列出的条目回答。

use std::path::{Path, PathBuf};

use file_core::validate_entry_name;

/// `~` 是主目录，相对路径接在当前文件夹后面。空的、或者是相对路径却没有当前文件夹（回收站、
/// 搜索结果里）时为 None。
pub(crate) fn jump_path(text: &str, current_dir: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    let path = Path::new(text);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    current_dir.map(|current_dir| current_dir.join(path))
}

/// 新建什么：结尾带 `/` 是文件夹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NewEntry {
    File(String),
    Folder(String),
}

/// 名字不合法、或者当前文件夹里已经有这个名字时，交回给用户看的原因。
pub(crate) fn read_new_entry(text: &str, taken: impl Fn(&str) -> bool) -> Result<NewEntry, String> {
    let text = text.trim();
    let (name, entry): (&str, fn(String) -> NewEntry) = match text.strip_suffix('/') {
        Some(name) => (name.trim_end(), NewEntry::Folder),
        None => (text, NewEntry::File),
    };
    if name.is_empty() {
        return Err(tr!("Type a name; end it with / to make a folder"));
    }
    if validate_entry_name(name.as_ref()).is_err() {
        return Err(tr!("“%1” cannot be used as a name", name));
    }
    if taken(name) {
        return Err(tr!("There is already an item named “%1” here", name));
    }
    Ok(entry(name.to_owned()))
}
