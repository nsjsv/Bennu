use iced::widget::{column, container, mouse_area, row, scrollable, text, Column};
use iced::{Element, Length};

use super::property_text::{
    display_optional_time, display_path, display_permissions, display_size, type_text,
};
use crate::app::info_panel::{EntryFacts, InfoPanelMessage, InfoTarget};
use crate::app::right_preview_panel::PANEL_CONTENT_PADDING;
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::{format_file_size, format_middle_ellipsized_text, format_system_time};
use crate::info_panel::{InfoField, InfoPanelPreferences};
use crate::model::{FilePropertiesKindDetails, FilePropertiesSnapshot, Message};
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};

/// 标签那一列的宽度；值在右边，长了折行。
const LABEL_WIDTH: f32 = 84.0;
const TITLE_MAX_CHARS: usize = 64;

pub(crate) fn info_field_label(field: InfoField) -> String {
    match field {
        InfoField::Type => tr!("Type"),
        InfoField::Size => tr!("Size"),
        InfoField::DiskSize => tr!("Size on disk"),
        InfoField::Location => tr!("Location"),
        InfoField::LinkTarget => tr!("Points to"),
        InfoField::Modified => tr!("Modified"),
        InfoField::Created => tr!("Created"),
        InfoField::Accessed => tr!("Accessed"),
        InfoField::StatusChanged => tr!("Status Changed"),
        InfoField::Permissions => tr!("Permissions"),
        InfoField::Owner => tr!("Owner"),
        InfoField::Details => tr!("Details"),
        InfoField::Pages => tr!("Pages"),
    }
}

/// 信息面板：单个条目是名字 + 字段表；多选是个数和大小；没选中是当前文件夹。右键多一段
/// 「显示的信息」（面板里自己吃掉右键，外面的停靠区菜单收不到）。
pub(super) fn info_panel_content(browser: &FileBrowser) -> Element<'_, Message> {
    let fields = &browser.user_config().info_panel;
    let body = match browser.info_panel.target() {
        None => Column::new().push(readable_text(tr!("Select an item to see its details"))),
        Some(InfoTarget::Selection) => selection_rows(browser),
        Some(InfoTarget::Entry(path)) => entry_rows(path, browser.info_panel.facts(), fields),
        Some(InfoTarget::Folder(path)) => {
            let counts = browser.info_panel_folder_counts();
            let mut rows = entry_rows(path, browser.info_panel.facts(), fields);
            rows = rows.push(info_row(tr!("Items"), counts.items.to_string()));
            if fields.shows(InfoField::Size) {
                let total = counts
                    .visible_files_bytes
                    .map(format_file_size)
                    .unwrap_or_else(|| "—".to_owned());
                rows = rows.push(info_row(tr!("Files total"), total));
            }
            rows
        }
    };
    let content = container(scrollable(body.spacing(6).width(Length::Fill)))
        .padding(iced::Padding {
            top: 8.0,
            bottom: 8.0,
            left: PANEL_CONTENT_PADDING,
            right: PANEL_CONTENT_PADDING,
        })
        .width(Length::Fill)
        .height(Length::Fill);
    mouse_area(content)
        .on_right_press(Message::InfoPanel(InfoPanelMessage::MenuRequested))
        .into()
}

fn selection_rows(browser: &FileBrowser) -> Column<'static, Message> {
    let summary = browser.info_panel_selection().unwrap_or_default();
    column![
        title(tr!(
            "%1 items",
            summary.directory_count + summary.file_count
        )),
        info_row(tr!("Folders"), summary.directory_count.to_string()),
        info_row(tr!("Files"), summary.file_count.to_string()),
        info_row(tr!("Size"), format_file_size(summary.file_total_bytes)),
    ]
}

/// 名字总在；其余字段读到了、用户没关掉、对这种条目有意义才显示。读不到不报错。
fn entry_rows(
    path: &std::path::Path,
    facts: Option<&EntryFacts>,
    fields: &InfoPanelPreferences,
) -> Column<'static, Message> {
    let snapshot = facts.and_then(|facts| facts.snapshot.as_ref()?.as_ref().ok());
    let name = snapshot
        .map(|snapshot| snapshot.name.to_string_lossy().into_owned())
        .or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut rows = Column::new().push(title(name));
    let Some(snapshot) = snapshot else {
        return rows;
    };
    for field in InfoField::ALL {
        if !fields.shows(field) {
            continue;
        }
        match field {
            InfoField::Details => {
                if let Some(Ok(details)) = facts.and_then(|facts| facts.details.as_ref()) {
                    for detail in details {
                        rows = rows.push(info_row(detail.label.clone(), detail.value.clone()));
                    }
                }
            }
            InfoField::Pages => {
                if let Some(pages) = facts.and_then(|facts| facts.pages) {
                    rows = rows.push(info_row(info_field_label(field), pages.to_string()));
                }
            }
            field => {
                if let Some(value) = snapshot_value(snapshot, field) {
                    rows = rows.push(info_row(info_field_label(field), value));
                }
            }
        }
    }
    rows
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

fn title(name: String) -> Element<'static, Message> {
    text(format_middle_ellipsized_text(&name, TITLE_MAX_CHARS))
        .font(BOLD)
        .into()
}

fn info_row(label: String, value: String) -> Element<'static, Message> {
    row![
        container(readable_text(label).size(CAPTION_SIZE).style(|theme| {
            iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }
        }))
        .width(Length::Fixed(LABEL_WIDTH)),
        text(value).width(Length::Fill),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}
