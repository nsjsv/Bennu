//! 一个服务菜单文件 → 若干 `ServiceMenuAction`。键的意思同 KIO 的 `shouldDisplayServiceMenu`、
//! `checkTypesMatch` 和 `KDesktopFile::actions`。写错的项单独列出来，同一个文件里别的项照常用。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use desktop_linux::desktop_file::DesktopFile;
use file_core::mime::MimeDatabase;

use super::super::definition::ServiceMenuKey;
use super::super::type_filter::{TypeFilter, TypePattern};
use super::exec::ExecTemplate;
use super::{ItemCount, Priority, ServiceMenuAction};

const ENTRY: &str = "Desktop Entry";
const LEGACY_MARKER: &str = "KonqPopupMenu/Plugin";
const SEPARATOR: &str = "_SEPARATOR_";

/// 文件是在哪找到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuFolder {
    /// `kio/servicemenus`
    Current,
    /// 旧的 `kservices5`：那里的 `.desktop` 不都是服务菜单。
    Legacy,
}

/// 读一个文件的结果：能用的项，和写错的项各自的原因。
#[derive(Debug, Default)]
pub(super) struct MenuFile {
    pub(super) actions: Vec<ServiceMenuAction>,
    pub(super) mistakes: Vec<String>,
}

pub(super) struct ReadContext<'a> {
    pub(super) types: &'a MimeDatabase,
    /// `zh_CN` 这样的标签；英文界面时为 None，用不带语言的键。
    pub(super) locale: Option<&'a str>,
    /// `kservicemenurc` 里关掉的动作键。
    pub(super) hidden: &'a HashSet<String>,
}

/// 整个文件用不了时返回 Err。不是给本地文件用的、不是服务菜单的旧文件，返回空的 `MenuFile`：
/// 那不算写错。
pub(super) fn read_menu_file(
    path: &Path,
    file_name: &str,
    text: &str,
    folder: MenuFolder,
    context: &ReadContext<'_>,
) -> Result<MenuFile, String> {
    let file = DesktopFile::parse(text).map_err(|error| error.to_string())?;
    if !file.has_group(ENTRY) {
        return Err(tr!("It has no [Desktop Entry] group"));
    }
    let service_types = kde_list(file.value(ENTRY, "ServiceTypes"));
    if folder == MenuFolder::Legacy && !service_types.iter().any(|kind| kind == LEGACY_MARKER) {
        return Ok(MenuFile::default());
    }
    if !shows_for_local_files(&file) {
        return Ok(MenuFile::default());
    }
    let mut mime_types = file.list(ENTRY, "MimeType");
    if mime_types.is_empty() {
        mime_types = service_types
            .into_iter()
            .filter(|kind| kind != LEGACY_MARKER)
            .collect();
    }
    let types = patterns(&mime_types, context.types);
    if types.is_empty() {
        return Err(tr!(
            "It does not say which items it is for (MimeType=), so it is never shown"
        ));
    }
    let excluded = patterns(
        &kde_list(file.value(ENTRY, "ExcludeServiceTypes")),
        context.types,
    );
    let count = ItemCount {
        exactly: kde_list(file.value(ENTRY, "X-KDE-RequiredNumberOfUrls"))
            .iter()
            .filter_map(|number| number.parse().ok())
            .collect(),
        at_least: number(&file, "X-KDE-MinNumberOfUrls"),
        at_most: number(&file, "X-KDE-MaxNumberOfUrls"),
    };
    let priority = match file
        .value(ENTRY, "X-KDE-Priority")
        .as_deref()
        .map(str::trim)
    {
        Some("TopLevel") => Priority::TopLevel,
        Some("Important") => Priority::Important,
        _ => Priority::Normal,
    };
    let submenu = file
        .localized(ENTRY, "X-KDE-Submenu", context.locale)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let keys = file.list(ENTRY, "Actions");
    if keys.is_empty() {
        return Err(tr!("It has no actions (Actions=)"));
    }
    let file_name: Arc<str> = Arc::from(file_name);
    let mut menu = MenuFile::default();
    for key in keys {
        if key == SEPARATOR || context.hidden.contains(&key) {
            continue;
        }
        let group = format!("Desktop Action {key}");
        let command = if file.has_group(&group) {
            match file.kde_value(&group, "Exec") {
                Some(exec) if !exec.trim().is_empty() => {
                    ExecTemplate::parse(&exec).map_err(|mistake| mistake.0)
                }
                _ => Err(tr!("It has no Exec line")),
            }
        } else {
            Err(tr!("The group is missing"))
        };
        let command = match command {
            Ok(command) => command,
            Err(reason) => {
                menu.mistakes.push(format!("[{group}] {reason}"));
                continue;
            }
        };
        let name = file
            .localized(&group, "Name", context.locale)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| key.clone());
        menu.actions.push(ServiceMenuAction {
            key: ServiceMenuKey {
                file: Arc::clone(&file_name),
                action: Arc::from(key.as_str()),
            },
            name,
            icon: file.value(&group, "Icon").unwrap_or_default(),
            file: path.to_path_buf(),
            priority,
            submenu: submenu.clone(),
            types: TypeFilter::Patterns(types.clone()),
            excluded: TypeFilter::Patterns(excluded.clone()),
            count: count.clone(),
            command,
        });
    }
    Ok(menu)
}

/// Bennu 列出的都是本地文件（KIO 的 `file` 协议）。只要别的协议、或者排除了 `file` 的，
/// 不是给这里用的。
fn shows_for_local_files(file: &DesktopFile) -> bool {
    if let Some(protocol) = file.value(ENTRY, "X-KDE-Protocol") {
        let protocol = protocol.trim();
        return match protocol.strip_prefix('!') {
            Some(excluded) => excluded != "file",
            None => protocol == "file",
        };
    }
    match file.value(ENTRY, "X-KDE-Protocols") {
        Some(protocols) => kde_list(Some(protocols))
            .iter()
            .any(|protocol| protocol == "file"),
        None => true,
    }
}

/// KConfig 的列表用 `,` 分隔；有人照 XDG 的习惯写 `;`，一起认。
fn kde_list(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split([',', ';'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn number(file: &DesktopFile, key: &str) -> Option<usize> {
    file.value(ENTRY, key)?.trim().parse().ok()
}

/// 认不出的类型丢掉：KIO 也只是匹配不上，不算写错。
fn patterns(texts: &[String], types: &MimeDatabase) -> Vec<TypePattern> {
    texts
        .iter()
        .filter_map(|text| TypePattern::parse(text, types))
        .collect()
}

#[cfg(test)]
#[path = "menu_file_tests.rs"]
mod tests;
