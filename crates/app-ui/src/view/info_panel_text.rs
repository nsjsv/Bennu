//! 信息面板此刻要显示的文字：一个标题加一列「标签 + 值」。
//!
//! 值是可选中的控件，它的内容是面板状态（选区在里面），不能在每次画面重建时现拼。所以文字
//! 在这里算好一次，状态拿它和手上的比，变了才换（`InfoPanelRows::reconcile`）；画面只读状态。

use super::info_panel::info_field_label;
use super::property_text::{
    display_optional_time, display_path, display_permissions, display_size, type_text,
};
use crate::app::info_panel::{EntryFacts, InfoTarget};
use crate::app::FileBrowser;
use crate::formatting::{format_file_size, format_system_time};
use crate::info_panel::{InfoField, InfoPanelPreferences};
use crate::model::{FilePropertiesKindDetails, FilePropertiesSnapshot};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct InfoPanelText {
    /// 条目的名字或「N 项」；没有目标时是 `None`，画面显示提示语。
    pub(crate) title: Option<String>,
    pub(crate) entries: Vec<InfoEntry>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InfoEntry {
    pub(crate) label: String,
    pub(crate) value: String,
}

impl InfoEntry {
    fn new(label: String, value: String) -> Self {
        Self { label, value }
    }
}

pub(crate) fn info_panel_text(browser: &FileBrowser) -> InfoPanelText {
    let fields = &browser.user_config().info_panel;
    match browser.info_panel.target() {
        None => InfoPanelText::default(),
        Some(InfoTarget::Selection) => selection_text(browser),
        Some(InfoTarget::Entry(path)) => entry_text(path, browser.info_panel.facts(), fields),
        Some(InfoTarget::Folder(path)) => {
            let counts = browser.info_panel_folder_counts();
            let mut text = entry_text(path, browser.info_panel.facts(), fields);
            text.entries
                .push(InfoEntry::new(tr!("Items"), counts.items.to_string()));
            if fields.shows(InfoField::Size) {
                let total = counts
                    .visible_files_bytes
                    .map(format_file_size)
                    .unwrap_or_else(|| "—".to_owned());
                text.entries.push(InfoEntry::new(tr!("Files total"), total));
            }
            text
        }
    }
}

fn selection_text(browser: &FileBrowser) -> InfoPanelText {
    let summary = browser.info_panel_selection().unwrap_or_default();
    InfoPanelText {
        title: Some(tr!(
            "%1 items",
            summary.directory_count + summary.file_count
        )),
        entries: vec![
            InfoEntry::new(tr!("Folders"), summary.directory_count.to_string()),
            InfoEntry::new(tr!("Files"), summary.file_count.to_string()),
            InfoEntry::new(tr!("Size"), format_file_size(summary.file_total_bytes)),
        ],
    }
}

/// 名字总在；其余字段读到了、用户没关掉、对这种条目有意义才显示。读不到不报错。
fn entry_text(
    path: &std::path::Path,
    facts: Option<&EntryFacts>,
    fields: &InfoPanelPreferences,
) -> InfoPanelText {
    let snapshot = facts.and_then(|facts| facts.snapshot.as_ref()?.as_ref().ok());
    let name = snapshot
        .map(|snapshot| snapshot.name.to_string_lossy().into_owned())
        .or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut text = InfoPanelText {
        title: Some(name),
        entries: Vec::new(),
    };
    let Some(snapshot) = snapshot else {
        return text;
    };
    for field in InfoField::ALL {
        if !fields.shows(field) {
            continue;
        }
        match field {
            InfoField::Details => {
                if let Some(Ok(details)) = facts.and_then(|facts| facts.details.as_ref()) {
                    for detail in details {
                        text.entries
                            .push(InfoEntry::new(detail.label.clone(), detail.value.clone()));
                    }
                }
            }
            InfoField::Pages => {
                if let Some(pages) = facts.and_then(|facts| facts.pages) {
                    text.entries
                        .push(InfoEntry::new(info_field_label(field), pages.to_string()));
                }
            }
            field => {
                if let Some(value) = snapshot_value(snapshot, field) {
                    text.entries
                        .push(InfoEntry::new(info_field_label(field), value));
                }
            }
        }
    }
    text
}

/// 快照里这一项的写法（跟属性窗口一样）；对这种条目没有意义时是 None。文件夹大小要递归统计，
/// 这里不算（要看去属性窗口）。
fn snapshot_value(snapshot: &FilePropertiesSnapshot, field: InfoField) -> Option<String> {
    let is_directory = matches!(snapshot.details, FilePropertiesKindDetails::Directory(_));
    match field {
        InfoField::Type => Some(type_text(&snapshot.details)),
        InfoField::Size => (!is_directory).then(|| display_size(snapshot.size_bytes)),
        InfoField::DiskSize => (!is_directory).then(|| display_size(snapshot.disk_size_bytes)),
        InfoField::Location => Some(display_path(&snapshot.location)),
        InfoField::LinkTarget => match &snapshot.details {
            FilePropertiesKindDetails::Symlink(Ok(target)) => Some(display_path(target)),
            _ => None,
        },
        InfoField::Modified => Some(display_optional_time(snapshot.modified)),
        // 文件系统不记创建时间时不显示这一行（跟属性窗口一样）。
        InfoField::Created => snapshot.created.map(format_system_time),
        InfoField::Accessed => Some(display_optional_time(snapshot.accessed)),
        InfoField::StatusChanged => Some(display_optional_time(snapshot.changed)),
        InfoField::Permissions => snapshot.permissions.map(display_permissions),
        InfoField::Owner => Some(tr!(
            "%1 (group %2)",
            snapshot.owner.user,
            snapshot.owner.group
        )),
        InfoField::Details | InfoField::Pages => None,
    }
}
