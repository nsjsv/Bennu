use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use bennu_preview::formatting::file_kind_label;
use bennu_preview::info_overlay::InfoRow;
use file_core::{FileKind, VolumeUsage};
use iced::widget::{button, column, container, row, Button, Column};
use iced::{Alignment, Element, Length};

use crate::formatting::{format_file_size, format_system_time, item_count_label};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesKindDetails, FilePropertiesLoadState,
    FilePropertiesLookup, FilePropertiesMessage, FilePropertiesOwner,
    FilePropertiesPermissionAccess, FilePropertiesPermissionClass, FilePropertiesPermissionUpdate,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesSnapshot,
    FilePropertiesState, Message, ScrollbarRegion,
};
use bennu_preview::entry_icons::file_entry_icon_symbol;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::settings_group::{
    action_row, control_row, label_row, meter_row, muted_setting_text, notice_row, settings_card,
    settings_group, value_row, RowNote, SettingsRow, SETTINGS_GROUP_SPACING,
};
use super::{auxiliary_window_message, themed_icon, IconTone};
use bennu_preview::window_shell::{single_window, HeaderBar, WindowShell};
use bennu_theme::controls::{icon_button, toggle_button_style, tooltip, tooltip_bubble};
use bennu_theme::typography::{BOLD, TITLE_SIZE};

const PROPERTIES_ICON_SIZE: f32 = 44.0;
const REFRESH_ICON_SIZE: f32 = 16.0;

/// 一页从上到下看完：加载中、读不了、单选、多选都是同一个外壳，打开时布局不跳。
pub(crate) fn view_properties_window<'a>(
    properties: Option<&'a FilePropertiesState>,
    shell: WindowShell<'a, Message>,
) -> Element<'a, Message> {
    let body = match properties {
        Some(properties) => properties_body(properties),
        None => auxiliary_window_message(tr!("No properties are available.")),
    };
    let header = HeaderBar::new()
        .title(tr!("Properties"))
        .end(refresh_button(properties));
    single_window(header, body, &shell)
}

/// 平时跟着磁盘上的变化自己更新；按一下全部重读。权限还在写时按不了。
fn refresh_button(properties: Option<&FilePropertiesState>) -> Element<'static, Message> {
    let refresh = properties
        .filter(|properties| !properties.permission_update.is_in_progress())
        .map(|_| Message::FileProperties(FilePropertiesMessage::RefreshRequested));
    tooltip(
        icon_button(themed_icon(
            IconSymbol::Refresh,
            IconTone::Normal,
            REFRESH_ICON_SIZE,
        ))
        .on_press_maybe(refresh),
        tooltip_bubble(tr!("Refresh")),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

fn properties_body(properties: &FilePropertiesState) -> Element<'_, Message> {
    match &properties.load_state {
        FilePropertiesLoadState::Loading => properties_notice(settings_card(vec![label_row(
            tr!("Loading properties..."),
            Some(RowNote::Hint(properties_target_label(properties))),
        )])),
        FilePropertiesLoadState::LoadingAggregate(snapshot) => properties_page(
            aggregate_information(snapshot).push(settings_card(vec![notice_row(RowNote::Hint(
                tr!("Calculating..."),
            ))])),
        ),
        FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) => {
            properties_page(single_information(properties, snapshot))
        }
        FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Aggregate(snapshot)) => {
            properties_page(aggregate_information(snapshot).push(permissions_section(
                &properties.permission_update,
                snapshot.permissions,
                None,
                false,
            )))
        }
        FilePropertiesLoadState::Failed(error) => properties_notice(settings_card(vec![
            label_row(
                tr!("Could not load properties"),
                Some(RowNote::Hint(properties_target_label(properties))),
            ),
            notice_row(RowNote::Error(error.clone())),
        ])),
    }
}

fn properties_target_label(properties: &FilePropertiesState) -> String {
    match properties.targets.single_path() {
        Some(path) => display_path(path),
        None => item_count_label(properties.targets.paths().len()),
    }
}

/// 还没读完、读不了：和读完后同一个页边距，一张卡片里一行说明。
fn properties_notice(card: Element<'_, Message>) -> Element<'_, Message> {
    container(card).padding(18).width(Length::Fill).into()
}

fn properties_page(content: Column<'_, Message>) -> Element<'_, Message> {
    auxiliary_detail_scroller(
        content.spacing(SETTINGS_GROUP_SPACING).width(Length::Fill),
        ScrollbarRegion::Properties,
    )
}

/// 大图标 + 名字 + 一行灰字。
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

/// 标题；基本信息；详细信息（图片、音视频）；磁盘；时间；权限。
fn single_information<'a>(
    properties: &'a FilePropertiesState,
    snapshot: &'a FilePropertiesSnapshot,
) -> Column<'a, Message> {
    let kind = snapshot.details.kind();
    let mut page = column![
        properties_heading(
            file_entry_icon_symbol(kind, snapshot.name.as_os_str()),
            display_name(&snapshot.name),
            heading_subtitle(snapshot),
        ),
        settings_card(basic_rows(snapshot)),
    ];
    if let FilePropertiesKindDetails::File(file) = &snapshot.details {
        if let Some(details) = file.media_details.as_ref().and_then(media_details_group) {
            page = page.push(details);
        }
    }
    page.push(volume_group(&snapshot.volume))
        .push(times_card(snapshot))
        .push(permissions_section(
            &properties.permission_update,
            snapshot.permissions,
            Some(&snapshot.owner),
            kind == FileKind::Directory,
        ))
}

/// 普通文件「PNG 图像 · 2.1 MB」，其余是种类名。
fn heading_subtitle(snapshot: &FilePropertiesSnapshot) -> String {
    match &snapshot.details {
        FilePropertiesKindDetails::File(file) => format!(
            "{} · {}",
            file.description
                .clone()
                .unwrap_or_else(|| file_kind_label(FileKind::File)),
            format_file_size(snapshot.size_bytes)
        ),
        details => file_kind_label(details.kind()),
    }
}

fn basic_rows(snapshot: &FilePropertiesSnapshot) -> Vec<SettingsRow<'static>> {
    let mut rows = vec![
        value_row(tr!("Type"), type_text(&snapshot.details)),
        value_row(tr!("Size"), display_size(snapshot.size_bytes)),
        value_row(tr!("Size on disk"), display_size(snapshot.disk_size_bytes)),
    ];
    if let FilePropertiesKindDetails::Directory(contents) = &snapshot.details {
        rows.push(value_row(tr!("Contents"), contents_text(contents)));
    }
    rows.push(value_row(tr!("Location"), display_path(&snapshot.location)));
    match &snapshot.details {
        FilePropertiesKindDetails::Symlink(target) => rows.push(value_row(
            tr!("Points to"),
            match target {
                Ok(target) => display_path(target),
                Err(error) => tr!("Unavailable (%1)", error),
            },
        )),
        FilePropertiesKindDetails::File(file) => rows.push(value_row(
            tr!("Open with"),
            default_application_text(&file.default_application),
        )),
        FilePropertiesKindDetails::Directory(_) | FilePropertiesKindDetails::Other => {}
    }
    rows
}

/// 普通文件「PNG 图像（image/png）」；类型库没有类型名的只写类型；其余是种类名。
fn type_text(details: &FilePropertiesKindDetails) -> String {
    match details {
        FilePropertiesKindDetails::File(file) => match &file.description {
            Some(description) => tr!("%1 (%2)", description, file.mime_type),
            None => file.mime_type.clone(),
        },
        details => file_kind_label(details.kind()),
    }
}

fn contents_text(contents: &FilePropertiesDirectoryContentsState) -> String {
    match contents {
        FilePropertiesDirectoryContentsState::Loading(contents) => contents
            .as_ref()
            .map(display_contents)
            .unwrap_or_else(|| tr!("Calculating...")),
        FilePropertiesDirectoryContentsState::Loaded(contents)
        | FilePropertiesDirectoryContentsState::Refreshing(contents) => display_contents(contents),
        FilePropertiesDirectoryContentsState::Failed(error) => tr!("Unavailable (%1)", error),
    }
}

fn default_application_text(application: &FilePropertiesLookup<Option<String>>) -> String {
    match application {
        FilePropertiesLookup::Pending => tr!("Reading..."),
        FilePropertiesLookup::Ready(Some(name)) => name.clone(),
        FilePropertiesLookup::Ready(None) => tr!("No default application"),
        FilePropertiesLookup::Failed(error) => tr!("Unavailable (%1)", error),
    }
}

/// 读完是空的（名字像视频、内容不是的 `.ts`）就没有这一组。
fn media_details_group(
    details: &FilePropertiesLookup<Vec<InfoRow>>,
) -> Option<Element<'static, Message>> {
    let rows = match details {
        FilePropertiesLookup::Pending => vec![notice_row(RowNote::Hint(tr!("Reading...")))],
        FilePropertiesLookup::Ready(rows) if rows.is_empty() => return None,
        FilePropertiesLookup::Ready(rows) => rows
            .iter()
            .map(|row| value_row(row.label.clone(), row.value.clone()))
            .collect(),
        FilePropertiesLookup::Failed(error) => vec![notice_row(RowNote::Error(error.clone()))],
    };
    Some(settings_group(tr!("Details"), None, rows))
}

/// 挂载点和文件系统；用量条是已用的比例，字写还能用多少。
fn volume_group(volume: &Result<VolumeUsage, String>) -> Element<'static, Message> {
    let rows = match volume {
        Ok(volume) => vec![
            value_row(
                tr!("Mounted on"),
                format!("{} · {}", display_path(&volume.mount_point), volume.fs_type),
            ),
            meter_row(
                tr!("Free space"),
                tr!(
                    "%1 free of %2",
                    format_file_size(volume.available_bytes),
                    format_file_size(volume.total_bytes)
                ),
                used_fraction(volume),
            ),
        ],
        Err(error) => vec![notice_row(RowNote::Error(tr!(
            "Could not read disk usage: %1",
            error
        )))],
    };
    settings_group(tr!("Disk"), None, rows)
}

fn used_fraction(volume: &VolumeUsage) -> f32 {
    if volume.total_bytes == 0 {
        return 0.0;
    }
    let used = volume.total_bytes.saturating_sub(volume.available_bytes);
    (used as f64 / volume.total_bytes as f64) as f32
}

fn times_card(snapshot: &FilePropertiesSnapshot) -> Element<'static, Message> {
    settings_card(
        time_rows(snapshot)
            .into_iter()
            .map(|(label, value)| value_row(label, value))
            .collect(),
    )
}

/// 文件系统不记创建时间（老式 ext4、网络和共享目录）时不显示这一行，不写「不可用」：
/// 这是文件系统的事，不是这个文件出了问题。状态改变时间（ctime）放最后。
fn time_rows(snapshot: &FilePropertiesSnapshot) -> Vec<(String, String)> {
    let created = snapshot
        .created
        .map(|created| (tr!("Created"), format_system_time(created)));
    created
        .into_iter()
        .chain([
            (tr!("Modified"), display_optional_time(snapshot.modified)),
            (tr!("Accessed"), display_optional_time(snapshot.accessed)),
            (
                tr!("Status Changed"),
                display_optional_time(snapshot.changed),
            ),
        ])
        .collect()
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
            value_row(
                tr!("Status Changed"),
                aggregate_optional_time(snapshot.common_changed)
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

/// 权限：所有者、用户组、其他人三行，每行右边“读 / 写 / 执行”三个开关按钮，点一下立即保存；
/// 最后一行是现在的模式。单选时副标题是实际的用户名、组名。文件夹多一行“应用到里面的所有项目”。
fn permissions_section<'a>(
    update: &'a FilePropertiesPermissionUpdate,
    permissions: Option<FilePropertiesPermissions>,
    owner: Option<&FilePropertiesOwner>,
    is_directory: bool,
) -> Element<'a, Message> {
    let mut section = Column::new().spacing(SETTINGS_GROUP_SPACING);
    match displayed_permissions(permissions, update) {
        Some(permissions) => {
            let mut rows: Vec<_> = FilePropertiesPermissionClass::ALL
                .into_iter()
                .map(|class| {
                    control_row(
                        permission_subject_label(class),
                        Some(RowNote::Hint(permission_subject_note(class, owner))),
                        permission_access_buttons(class, permissions, update),
                    )
                })
                .collect();
            rows.push(value_row(
                tr!("Current mode"),
                display_permissions(permissions),
            ));
            section = section.push(settings_group(tr!("Permissions"), None, rows));
            if is_directory {
                section = section.push(settings_card(vec![action_row(
                    tr!("Apply to Enclosed Items"),
                    None,
                    (!update.is_in_progress()).then_some(Message::FileProperties(
                        FilePropertiesMessage::ApplyPermissionsToEnclosedItems,
                    )),
                )]));
            }
        }
        None => {
            section = section.push(settings_group(
                tr!("Permissions"),
                None,
                vec![label_row(
                    tr!("Permissions are read-only"),
                    Some(RowNote::Hint(tr!(
                        "Permission editing is unavailable for this item."
                    ))),
                )],
            ));
        }
    }

    if let Some(status) = permissions_status(update) {
        section = section.push(settings_card(vec![notice_row(status)]));
    }
    section.into()
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
        FilePropertiesPermissionClass::Group => tr!("User group"),
        FilePropertiesPermissionClass::Others => tr!("Others"),
    }
}

/// 单选写实际的用户名、组名；多选各项的所有者可能不同，写泛称。
fn permission_subject_note(
    class: FilePropertiesPermissionClass,
    owner: Option<&FilePropertiesOwner>,
) -> String {
    match (class, owner) {
        (FilePropertiesPermissionClass::Owner, Some(owner)) => owner.user.clone(),
        (FilePropertiesPermissionClass::Group, Some(owner)) => owner.group.clone(),
        (FilePropertiesPermissionClass::Owner, None) => tr!("Primary user"),
        (FilePropertiesPermissionClass::Group, None) => tr!("Assigned group"),
        (FilePropertiesPermissionClass::Others, _) => tr!("Other users"),
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

    fn file_details(description: Option<&str>) -> FilePropertiesKindDetails {
        FilePropertiesKindDetails::File(crate::model::FilePropertiesFileDetails {
            mime_type: "image/png".to_owned(),
            description: description.map(str::to_owned),
            default_application: FilePropertiesLookup::Pending,
            media_details: None,
        })
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

    fn snapshot_with_times(created: Option<SystemTime>) -> FilePropertiesSnapshot {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        FilePropertiesSnapshot {
            name: "notes.txt".into(),
            location: "/tmp".into(),
            created,
            modified: Some(time),
            accessed: Some(time),
            changed: Some(time),
            size_bytes: 0,
            disk_size_bytes: 0,
            details: file_details(None),
            owner: crate::model::FilePropertiesOwner {
                user: "alice".to_owned(),
                group: "staff".to_owned(),
            },
            volume: Err("not measured".to_owned()),
            permissions: None,
        }
    }

    #[test]
    fn creation_time_is_left_out_when_the_file_system_has_none() {
        let labels = |created| {
            time_rows(&snapshot_with_times(created))
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        };
        let created = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1));

        assert_eq!(
            labels(created),
            [
                tr!("Created"),
                tr!("Modified"),
                tr!("Accessed"),
                tr!("Status Changed")
            ]
        );
        assert_eq!(
            labels(None),
            [tr!("Modified"), tr!("Accessed"), tr!("Status Changed")]
        );
    }

    #[test]
    fn contents_say_when_part_of_the_folder_could_not_be_read() {
        let complete = display_contents(&contents(0));
        let incomplete = display_contents(&contents(2));

        assert!(!complete.contains("unreadable") && !complete.contains("无法读取"));
        assert!(incomplete.starts_with(&complete));
        assert!(incomplete.contains("unreadable") || incomplete.contains("无法读取"));
    }

    #[test]
    fn files_show_the_type_name_and_type_others_show_their_kind() {
        assert_eq!(
            type_text(&file_details(Some("PNG image"))),
            "PNG image (image/png)"
        );
        assert_eq!(type_text(&file_details(None)), "image/png");
        assert_eq!(
            type_text(&FilePropertiesKindDetails::Symlink(Ok("/a".into()))),
            file_kind_label(FileKind::Symlink)
        );
    }

    #[test]
    fn single_items_name_their_owner_and_group_multiple_items_do_not() {
        let owner = FilePropertiesOwner {
            user: "alice".to_owned(),
            group: "staff".to_owned(),
        };
        let note = |class, owner| permission_subject_note(class, owner);

        assert_eq!(
            note(FilePropertiesPermissionClass::Owner, Some(&owner)),
            "alice"
        );
        assert_eq!(
            note(FilePropertiesPermissionClass::Group, Some(&owner)),
            "staff"
        );
        assert_eq!(
            note(FilePropertiesPermissionClass::Others, Some(&owner)),
            tr!("Other users")
        );
        assert_eq!(
            note(FilePropertiesPermissionClass::Owner, None),
            tr!("Primary user")
        );
    }

    #[test]
    fn the_default_application_row_says_what_is_known() {
        assert_eq!(
            default_application_text(&FilePropertiesLookup::Ready(Some("Viewer".to_owned()))),
            "Viewer"
        );
        assert_eq!(
            default_application_text(&FilePropertiesLookup::Ready(None)),
            tr!("No default application")
        );
        assert_eq!(
            default_application_text(&FilePropertiesLookup::Pending),
            tr!("Reading...")
        );
    }

    #[test]
    fn an_empty_media_reading_hides_the_details_group() {
        assert!(media_details_group(&FilePropertiesLookup::Ready(Vec::new())).is_none());
        assert!(media_details_group(&FilePropertiesLookup::Pending).is_some());
    }

    #[test]
    fn the_disk_bar_shows_the_used_share() {
        let volume = |total_bytes, available_bytes| VolumeUsage {
            mount_point: "/".into(),
            fs_type: "ext4".to_owned(),
            total_bytes,
            available_bytes,
        };
        assert_eq!(used_fraction(&volume(100, 40)), 0.6);
        assert_eq!(used_fraction(&volume(0, 0)), 0.0);
    }
}
