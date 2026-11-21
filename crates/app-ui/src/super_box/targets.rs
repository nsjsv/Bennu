//! 新建模式里，框里的字指的是什么。只看字，不读盘：名字有没有被占用由调用方按已经列出的
//! 条目回答。路径模式的字和地址栏一个意思，见 `bennu_theme::address_bar::typed_path`。

use file_core::validate_entry_name;

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
