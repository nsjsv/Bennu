use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use bennu_preview::formatting::file_kind_label;
use file_core::FileKind;
use iced::widget::{button, column, container, row, Button, Column};
use iced::{Alignment, Element, Length};

use crate::formatting::{format_file_size, format_system_time, item_count_label};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesCategory, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesLoadState, FilePropertiesMessage,
    FilePropertiesPermissionAccess, FilePropertiesPermissionClass, FilePropertiesPermissionUpdate,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesSnapshot,
    FilePropertiesState, Message, ScrollbarRegion,
};
use bennu_preview::entry_icons::file_entry_icon_symbol;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::auxiliary_window_layout::{
    auxiliary_detail_scroller, auxiliary_sidebar_button, auxiliary_sidebar_list,
    AUXILIARY_SIDEBAR_WIDTH,
};
use super::settings_group::{
    action_row, control_row, label_row, muted_setting_text, notice_row, settings_card, value_row,
    RowNote, SETTINGS_GROUP_SPACING,
};
use super::{auxiliary_window_message, themed_icon, IconTone};
use bennu_preview::window_shell::{single_window, split_window, HeaderBar, SplitPane, WindowShell};
use bennu_theme::controls::toggle_button_style;
use bennu_theme::typography::{BOLD, TITLE_SIZE};

const PROPERTIES_ICON_SIZE: f32 = 44.0;

pub(crate) fn view_properties_window<'a>(
    properties: Option<&'a FilePropertiesState>,
    shell: WindowShell<'a, Message>,
) -> Element<'a, Message> {
    let single = |body: Element<'a, Message>| {
        single_window(HeaderBar::new().title(tr!("Properties")), body, &shell)
    };
    let Some(properties) = properties else {
        return single(auxiliary_window_message(tr!(
            "No properties are available."
        )));
    };

    let target_label = properties_target_label(properties);
    match &properties.load_state {
        FilePropertiesLoadState::Loading => single(properties_loading_view(target_label)),
        FilePropertiesLoadState::LoadingAggregate(snapshot) => {
            single(aggregate_properties_loading_view(snapshot))
        }
        FilePropertiesLoadState::Loaded(presentation) => {
            properties_split_view(properties, presentation, &shell)
        }
        FilePropertiesLoadState::Failed(error) => {
            single(properties_error_view(target_label, error))
        }
    }
}

fn properties_target_label(properties: &FilePropertiesState) -> String {
    match properties.targets.single_path() {
        Some(path) => display_path(path),
        None => item_count_label(properties.targets.paths().len()),
    }
}

/// 还没读完、读不了：和读完后同一个页边距，一张卡片里一行说明。
fn properties_loading_view(target_label: String) -> Element<'static, Message> {
    properties_notice(settings_card(vec![label_row(
        tr!("Loading properties..."),
        Some(RowNote::Hint(target_label)),
    )]))
}

fn aggregate_properties_loading_view(
    snapshot: &FilePropertiesAggregateSnapshot,
) -> Element<'_, Message> {
    properties_detail_scroller(aggregate_information(snapshot).push(settings_card(vec![
        notice_row(RowNote::Hint(tr!("Calculating..."))),
    ])))
}

fn properties_error_view<'a>(target_label: String, error: &'a str) -> Element<'a, Message> {
    properties_notice(settings_card(vec![
        label_row(
            tr!("Could not load properties"),
            Some(RowNote::Hint(target_label)),
        ),
        notice_row(RowNote::Error(error.to_owned())),
    ]))
}

fn properties_notice(card: Element<'_, Message>) -> Element<'_, Message> {
    container(card).padding(18).width(Length::Fill).into()
}

fn properties_split_view<'a>(
    properties: &'a FilePropertiesState,
    presentation: &'a FilePropertiesPresentation,
    shell: &WindowShell<'a, Message>,
) -> Element<'a, Message> {
    let selected = properties.selected_category;
    let detail = match selected {
        FilePropertiesCategory::Information => match presentation {
            FilePropertiesPresentation::Single(snapshot) => properties_information_detail(snapshot),
            FilePropertiesPresentation::Aggregate(snapshot) => {
                aggregate_properties_information_detail(snapshot)
            }
        },
        FilePropertiesCategory::Permissions => {
            properties_permissions_detail(properties, presentation)
        }
    };
    let sidebar = SplitPane {
        header: HeaderBar::new().title(tr!("Properties")),
        body: auxiliary_sidebar_list(
            FilePropertiesCategory::ALL
                .into_iter()
                .map(|category| properties_category_button(category, selected).into())
                .collect(),
        ),
    };
    let content = SplitPane {
        header: HeaderBar::new().title(selected.label()),
        body: detail,
    };
    split_window(sidebar, content, AUXILIARY_SIDEBAR_WIDTH, shell)
}

fn properties_category_button(
    category: FilePropertiesCategory,
    selected: FilePropertiesCategory,
) -> Button<'static, Message> {
    auxiliary_sidebar_button(
        category.label(),
        category == selected,
        Message::FileProperties(FilePropertiesMessage::CategorySelected(category)),
    )
}

/// 大图标 + 名字 + 一行灰字（类型）；下面的卡片不再重复名字和类型。
fn properties_heading(
    icon: IconSymbol,
    title: String,
    subtitle: String,
) -> Element<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, PROPERTIES_ICON_SIZE),
        column![
            readable_text(title).size(TITLE_SIZE).font(BOLD),
            muted_setting_text(subtitle),
        ]
        .spacing(4)
        .width(Length::Fill)
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .into()
}

fn properties_information_detail(snapshot: &FilePropertiesSnapshot) -> Element<'_, Message> {
    let mut place = vec![
        value_row(tr!("Location"), display_path(&snapshot.location)),
        value_row(tr!("Size"), display_size(snapshot.size_bytes)),
        value_row(tr!("Size on disk"), display_size(snapshot.disk_size_bytes)),
    ];
    let contents = match &snapshot.directory_contents {
        FilePropertiesDirectoryContentsState::NotDirectory => None,
        FilePropertiesDirectoryContentsState::Loading(contents) => Some(
            contents
                .as_ref()
                .map(display_contents)
                .unwrap_or_else(|| tr!("Calculating...")),
        ),
        FilePropertiesDirectoryContentsState::Loaded(contents) => Some(display_contents(contents)),
        FilePropertiesDirectoryContentsState::Failed(error) => Some(tr!("Unavailable (%1)", error)),
    };
    if let Some(contents) = contents {
        place.push(value_row(tr!("Contents"), contents));
    }

    properties_detail_scroller(column![
        properties_heading(
            file_entry_icon_symbol(snapshot.kind, snapshot.name.as_os_str()),
            display_name(&snapshot.name),
            file_kind_label(snapshot.kind),
        ),
        settings_card(place),
        settings_card(vec![
            value_row(tr!("Created"), display_optional_time(snapshot.created)),
            value_row(tr!("Modified"), display_optional_time(snapshot.modified)),
            value_row(tr!("Accessed"), display_optional_time(snapshot.accessed)),
        ]),
    ])
}

fn aggregate_properties_information_detail(
    snapshot: &FilePropertiesAggregateSnapshot,
) -> Element<'_, Message> {
    properties_detail_scroller(aggregate_information(snapshot))
}

/// 多选：标题是“N 项”，卡片里是各类各几个、在哪、总大小，时间只在大家都一样时有。
fn aggregate_information(snapshot: &FilePropertiesAggregateSnapshot) -> Column<'_, Message> {
    let location = snapshot
        .common_parent
        .as_deref()
        .map(display_path)
        .unwrap_or_else(|| tr!("Multiple locations"));
    let counts = tr!(
        "%1 files, %2 folders, %3 links, %4 other",
        snapshot.file_count,
        snapshot.directory_count,
        snapshot.symlink_count,
        snapshot.other_count
    );
    let contents = with_unreadable_contents_note(
        tr!(
            "%1 files, %2 folders",
            snapshot.recursive_contents.file_count,
            snapshot.recursive_contents.directory_count
        ),
        snapshot.recursive_contents.unreadable_entries,
    );
    column![
        properties_heading(
            IconSymbol::File,
            item_count_label(snapshot.target_count),
            aggregate_type_label(snapshot),
        ),
        settings_card(vec![
            value_row(tr!("Types"), counts),
            value_row(tr!("Location"), location),
            value_row(tr!("Size"), display_size(snapshot.total_size_bytes)),
            value_row(
                tr!("Size on disk"),
                display_size(snapshot.total_disk_size_bytes)
            ),
            value_row(tr!("Contents"), contents),
        ]),
        settings_card(vec![
            value_row(
                tr!("Created"),
                aggregate_optional_time(snapshot.common_created)
            ),
            value_row(
                tr!("Modified"),
                aggregate_optional_time(snapshot.common_modified)
            ),
            value_row(
                tr!("Accessed"),
                aggregate_optional_time(snapshot.common_accessed)
            ),
        ]),
    ]
}

fn aggregate_type_label(snapshot: &FilePropertiesAggregateSnapshot) -> String {
    match snapshot.common_kind {
        Some(FileKind::Directory) => tr!("Folders"),
        Some(FileKind::File) => tr!("Files"),
        Some(FileKind::Symlink) => tr!("Symbolic Links"),
        Some(FileKind::Other) => tr!("Other Items"),
        None => tr!("Multiple Item Types"),
    }
}

fn aggregate_optional_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time)
        .unwrap_or_else(|| tr!("Mixed or unavailable"))
}

/// 权限：一张卡片三行（所有者、组、其他人），每行右边“读 / 写 / 执行”三个开关按钮，点一下
/// 立即保存；最后一行是现在的模式。文件夹多一行“应用到里面的所有项目”。
fn properties_permissions_detail<'a>(
    properties: &'a FilePropertiesState,
    presentation: &'a FilePropertiesPresentation,
) -> Element<'a, Message> {
    let update = &properties.permission_update;
    let mut content = column![muted_setting_text(tr!(
        "Choose who can read, write, or execute this item. Changes save immediately."
    ))];

    match displayed_permissions(presentation.permissions(), update) {
        Some(permissions) => {
            let mut rows: Vec<_> = FilePropertiesPermissionClass::ALL
                .into_iter()
                .map(|class| {
                    control_row(
                        permission_subject_label(class),
                        Some(RowNote::Hint(permission_subject_description(class))),
                        permission_access_buttons(class, permissions, update),
                    )
                })
                .collect();
            rows.push(value_row(
                tr!("Current mode"),
                display_permissions(permissions),
            ));
            content = content.push(settings_card(rows));
            if matches!(
                presentation,
                FilePropertiesPresentation::Single(snapshot)
                    if snapshot.kind == FileKind::Directory
            ) {
                content = content.push(settings_card(vec![action_row(
                    tr!("Apply to Enclosed Items"),
                    None,
                    (!update.is_in_progress()).then_some(Message::FileProperties(
                        FilePropertiesMessage::ApplyPermissionsToEnclosedItems,
                    )),
                )]));
            }
        }
        None => {
            content = content.push(settings_card(vec![label_row(
                tr!("Permissions are read-only"),
                Some(RowNote::Hint(tr!(
                    "Permission editing is unavailable for this item."
                ))),
            )]));
        }
    }

    if let Some(status) = permissions_status(update) {
        content = content.push(settings_card(vec![notice_row(status)]));
    }

    properties_detail_scroller(content)
}

fn properties_detail_scroller<'a>(content: Column<'a, Message>) -> Element<'a, Message> {
    auxiliary_detail_scroller(
        content.spacing(SETTINGS_GROUP_SPACING).width(Length::Fill),
        ScrollbarRegion::Properties,
    )
}

fn displayed_permissions(
    permissions: Option<FilePropertiesPermissions>,
    update: &FilePropertiesPermissionUpdate,
) -> Option<FilePropertiesPermissions> {
    update.pending_permissions().or(permissions)
}

/// 读 / 写 / 执行：开着的是选中样式；正在保存时都点不了，免得两次 chmod 交错。
fn permission_access_buttons(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Element<'static, Message> {
    let mut buttons = row![].spacing(4).align_y(Alignment::Center);
    for access in FilePropertiesPermissionAccess::ALL {
        buttons = buttons.push(permission_access_button(class, access, permissions, update));
    }
    buttons.into()
}

fn permission_access_button(
    class: FilePropertiesPermissionClass,
    access: FilePropertiesPermissionAccess,
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Button<'static, Message> {
    button(readable_text(access.label()))
        .style(toggle_button_style(permissions.contains(class, access)))
        .on_press_maybe(
            (!update.is_in_progress()).then_some(Message::FileProperties(
                FilePropertiesMessage::PermissionToggled(class, access),
            )),
        )
}

/// 保存中是灰字；失败（含部分失败）是红字。
fn permissions_status(update: &FilePropertiesPermissionUpdate) -> Option<RowNote> {
    let note = match update {
        FilePropertiesPermissionUpdate::Idle => return None,
        FilePropertiesPermissionUpdate::SavingCurrentItem { .. } => {
            RowNote::Hint(tr!("Saving permissions..."))
        }
        FilePropertiesPermissionUpdate::SavingTargetSet { .. } => {
            RowNote::Hint(tr!("Saving permissions for selected items..."))
        }
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. } => {
            RowNote::Hint(tr!("Applying permissions to enclosed items..."))
        }
        FilePropertiesPermissionUpdate::TargetSetCompleted {
            succeeded_count,
            failures,
        } => {
            if failures.is_empty() {
                RowNote::Hint(tr!("Updated permissions for %1 items.", succeeded_count))
            } else {
                RowNote::Error(tr!(
                    "Updated %1 items; %2 items failed. %3",
                    succeeded_count,
                    failures.len(),
                    failures
                        .iter()
                        .map(|failure| format!("{:?}: {}", failure.path, failure.error))
                        .collect::<Vec<_>>()
                        .join("; ")
                ))
            }
        }
        FilePropertiesPermissionUpdate::Failed(error) => {
            RowNote::Error(tr!("Could not update permissions: %1", error))
        }
    };
    Some(note)
}

fn permission_subject_label(class: FilePropertiesPermissionClass) -> String {
    match class {
        FilePropertiesPermissionClass::Owner => tr!("Owner"),
        FilePropertiesPermissionClass::Group => tr!("Group"),
        FilePropertiesPermissionClass::Others => tr!("Everyone"),
    }
}

fn permission_subject_description(class: FilePropertiesPermissionClass) -> String {
    match class {
        FilePropertiesPermissionClass::Owner => tr!("Primary user"),
        FilePropertiesPermissionClass::Group => tr!("Assigned group"),
        FilePropertiesPermissionClass::Others => tr!("Other users"),
    }
}

fn display_name(name: &OsStr) -> String {
    name.to_string_lossy().into_owned()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn display_optional_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time)
        .unwrap_or_else(|| tr!("Unavailable"))
}

fn display_size(bytes: u64) -> String {
    tr!("%1 (%2 bytes)", format_file_size(bytes), bytes)
}

fn display_contents(contents: &FilePropertiesDirectoryContents) -> String {
    let counts = tr!(
        "%1 files, %2 folders, total %3",
        contents.file_count,
        contents.directory_count,
        format_file_size(contents.total_size_bytes)
    );
    with_unreadable_contents_note(counts, contents.unreadable_entries)
}

/// 有读不了的子项时注明：数字只算了读得到的部分。
fn with_unreadable_contents_note(counts: String, unreadable_entries: u64) -> String {
    if unreadable_entries == 0 {
        counts
    } else {
        tr!("%1 (some contents unreadable)", counts)
    }
}

fn display_permissions(permissions: FilePropertiesPermissions) -> String {
    format!(
        "{} ({})",
        permissions.symbolic_string(),
        permissions.octal_string()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(unreadable_entries: u64) -> FilePropertiesDirectoryContents {
        FilePropertiesDirectoryContents {
            file_count: 3,
            directory_count: 1,
            total_size_bytes: 2048,
            total_disk_size_bytes: 4096,
            unreadable_entries,
        }
    }

    #[test]
    fn saving_permissions_is_gray_and_anything_that_failed_is_red() {
        let permissions = FilePropertiesPermissions::from_mode(0o644);
        assert!(permissions_status(&FilePropertiesPermissionUpdate::Idle).is_none());
        assert!(matches!(
            permissions_status(&FilePropertiesPermissionUpdate::SavingCurrentItem { permissions }),
            Some(RowNote::Hint(_))
        ));
        assert!(matches!(
            permissions_status(&FilePropertiesPermissionUpdate::TargetSetCompleted {
                succeeded_count: 2,
                failures: Vec::new(),
            }),
            Some(RowNote::Hint(_))
        ));
        // 部分失败也是失败。
        assert!(matches!(
            permissions_status(&FilePropertiesPermissionUpdate::TargetSetCompleted {
                succeeded_count: 1,
                failures: vec![crate::model::PermissionBatchPathFailure {
                    path: "/tmp/a".into(),
                    error: "denied".to_owned(),
                }],
            }),
            Some(RowNote::Error(_))
        ));
        assert!(matches!(
            permissions_status(&FilePropertiesPermissionUpdate::Failed("denied".to_owned())),
            Some(RowNote::Error(_))
        ));
    }

    #[test]
    fn contents_say_when_part_of_the_folder_could_not_be_read() {
        let complete = display_contents(&contents(0));
        let incomplete = display_contents(&contents(2));

        assert!(!complete.contains("unreadable") && !complete.contains("无法读取"));
        assert!(incomplete.starts_with(&complete));
        assert!(incomplete.contains("unreadable") || incomplete.contains("无法读取"));
    }
}
