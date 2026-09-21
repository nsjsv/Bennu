use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use file_core::FileKind;
use iced::widget::{button, column, container, row, Button, Column, Space};
use iced::{Alignment, Element, Length};

use crate::appearance::{
    app_content_style, context_menu_button_style, context_menu_style, preview_panel_style,
    selected_sidebar_item_style,
};
use crate::formatting::{format_file_size, format_system_time};
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesCategory, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesLoadState, FilePropertiesMessage,
    FilePropertiesPermissionAccess, FilePropertiesPermissionClass, FilePropertiesPermissionUpdate,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesSnapshot,
    FilePropertiesState, Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility,
};
use crate::typography::{localized_text, readable_text};

use super::auxiliary_window_layout::{
    auxiliary_detail_scroller, auxiliary_sidebar, auxiliary_sidebar_button, auxiliary_split_window,
};
use super::toggle_switch::switch_control;
use super::{auxiliary_window_message, themed_icon, IconTone};

const PROPERTIES_ICON_SIZE: f32 = 44.0;
const PROPERTIES_LABEL_WIDTH: f32 = 128.0;
const PERMISSION_PRIVILEGE_WIDTH: f32 = 184.0;

pub(crate) fn view_properties_window(
    properties: Option<&FilePropertiesState>,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let Some(properties) = properties else {
        return auxiliary_window_message("No properties are available.");
    };

    let target_label = properties_target_label(properties);
    match &properties.load_state {
        FilePropertiesLoadState::Loading => properties_loading_view(target_label),
        FilePropertiesLoadState::LoadingAggregate(snapshot) => {
            aggregate_properties_loading_view(snapshot, scrollbar_visibility, scrollbar_viewport)
        }
        FilePropertiesLoadState::Loaded(presentation) => properties_split_view(
            properties,
            presentation,
            scrollbar_visibility,
            scrollbar_viewport,
        ),
        FilePropertiesLoadState::Failed(error) => properties_error_view(target_label, error),
    }
}

fn properties_target_label(properties: &FilePropertiesState) -> String {
    match properties.targets.single_path() {
        Some(path) => display_path(path),
        None => crate::localization::translate_current(&format!(
            "{} items",
            properties.targets.paths().len()
        )),
    }
}

fn properties_loading_view(target_label: String) -> Element<'static, Message> {
    let content = column![
        readable_text("Loading properties...").size(16),
        readable_text(target_label).size(13)
    ]
    .spacing(8)
    .padding(24)
    .width(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

fn aggregate_properties_loading_view(
    snapshot: &FilePropertiesAggregateSnapshot,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let content = column![
        readable_text("Calculating...").size(13),
        aggregate_properties_information_detail(snapshot, scrollbar_visibility, scrollbar_viewport),
    ]
    .spacing(8)
    .padding(16)
    .width(Length::Fill)
    .height(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

fn properties_error_view<'a>(target_label: String, error: &'a str) -> Element<'a, Message> {
    let content = column![
        readable_text("Could not load properties").size(16),
        readable_text(target_label).size(13),
        localized_text(error).size(13)
    ]
    .spacing(8)
    .padding(24)
    .width(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

fn properties_split_view<'a>(
    properties: &'a FilePropertiesState,
    presentation: &'a FilePropertiesPresentation,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    let sidebar = properties_category_sidebar(properties.selected_category);
    let detail = match properties.selected_category {
        FilePropertiesCategory::Information => match presentation {
            FilePropertiesPresentation::Single(snapshot) => {
                properties_information_detail(snapshot, scrollbar_visibility, scrollbar_viewport)
            }
            FilePropertiesPresentation::Aggregate(snapshot) => {
                aggregate_properties_information_detail(
                    snapshot,
                    scrollbar_visibility,
                    scrollbar_viewport,
                )
            }
        },
        FilePropertiesCategory::Permissions => properties_permissions_detail(
            properties,
            presentation,
            scrollbar_visibility,
            scrollbar_viewport,
        ),
    };

    auxiliary_split_window(sidebar, detail)
}

fn properties_category_sidebar(selected: FilePropertiesCategory) -> Element<'static, Message> {
    let mut categories = Column::new()
        .spacing(6)
        .push(readable_text("Properties").size(18))
        .push(Space::new().height(Length::Fixed(6.0)));

    for category in FilePropertiesCategory::ALL {
        categories = categories.push(properties_category_button(category, selected));
    }

    auxiliary_sidebar(categories)
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

fn properties_information_detail(
    snapshot: &FilePropertiesSnapshot,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let icon = file_entry_icon_symbol(snapshot.kind, snapshot.name.as_os_str());
    let header = row![
        themed_icon(icon, IconTone::Normal, PROPERTIES_ICON_SIZE),
        column![
            readable_text(display_name(&snapshot.name)).size(18),
            localized_text(snapshot.type_label.clone()).size(13)
        ]
        .spacing(4)
        .width(Length::Fill)
    ]
    .spacing(14)
    .align_y(Alignment::Center);

    let mut details = Column::new()
        .spacing(10)
        .push(property_row("Name", display_name(&snapshot.name)))
        .push(property_row(
            "Type",
            crate::localization::translate_current(&snapshot.type_label),
        ))
        .push(property_row("Location", display_path(&snapshot.location)))
        .push(property_row(
            "Created",
            display_optional_time(snapshot.created),
        ))
        .push(property_row(
            "Modified",
            display_optional_time(snapshot.modified),
        ))
        .push(property_row(
            "Accessed",
            display_optional_time(snapshot.accessed),
        ))
        .push(property_row("Size", display_size(snapshot.size_bytes)))
        .push(property_row(
            "Size on disk",
            display_size(snapshot.disk_size_bytes),
        ));

    match &snapshot.directory_contents {
        FilePropertiesDirectoryContentsState::NotDirectory => {}
        FilePropertiesDirectoryContentsState::Loading(contents) => {
            let label = contents
                .as_ref()
                .map(display_contents)
                .unwrap_or_else(|| crate::localization::translate_current("Calculating..."));
            details = details.push(property_row("Contents", label));
        }
        FilePropertiesDirectoryContentsState::Loaded(contents) => {
            details = details.push(property_row("Contents", display_contents(contents)));
        }
        FilePropertiesDirectoryContentsState::Failed(error) => {
            details = details.push(property_row(
                "Contents",
                format!(
                    "{} ({error})",
                    crate::localization::translate_current("Unavailable")
                ),
            ));
        }
    }

    properties_detail_scroller(
        column![header, details].spacing(22).width(Length::Fill),
        scrollbar_visibility,
        scrollbar_viewport,
    )
}

fn aggregate_properties_information_detail(
    snapshot: &FilePropertiesAggregateSnapshot,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let header = row![
        themed_icon(IconSymbol::File, IconTone::Normal, PROPERTIES_ICON_SIZE),
        column![
            localized_text(format!("{} items", snapshot.target_count)).size(18),
            readable_text(aggregate_type_label(snapshot)).size(13)
        ]
        .spacing(4)
        .width(Length::Fill)
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    let location = snapshot
        .common_parent
        .as_deref()
        .map(display_path)
        .unwrap_or_else(|| crate::localization::translate_current("Multiple locations"));
    let counts = aggregate_target_counts(snapshot);
    let contents = aggregate_contents_counts(snapshot);
    let details = column![
        property_row("Selected", snapshot.target_count.to_string()),
        property_row("Types", counts),
        property_row("Location", location),
        property_row("Created", aggregate_optional_time(snapshot.common_created)),
        property_row(
            "Modified",
            aggregate_optional_time(snapshot.common_modified)
        ),
        property_row(
            "Accessed",
            aggregate_optional_time(snapshot.common_accessed)
        ),
        property_row("Size", display_size(snapshot.total_size_bytes)),
        property_row("Size on disk", display_size(snapshot.total_disk_size_bytes)),
        property_row("Contents", contents),
    ]
    .spacing(10);

    properties_detail_scroller(
        column![header, details].spacing(22).width(Length::Fill),
        scrollbar_visibility,
        scrollbar_viewport,
    )
}

fn aggregate_target_counts(snapshot: &FilePropertiesAggregateSnapshot) -> String {
    if crate::localization::current_language_is_chinese() {
        format!(
            "{} 个文件，{} 个文件夹，{} 个链接，{} 个其它项目",
            snapshot.file_count,
            snapshot.directory_count,
            snapshot.symlink_count,
            snapshot.other_count
        )
    } else {
        format!(
            "{} files, {} folders, {} links, {} other",
            snapshot.file_count,
            snapshot.directory_count,
            snapshot.symlink_count,
            snapshot.other_count
        )
    }
}

fn aggregate_contents_counts(snapshot: &FilePropertiesAggregateSnapshot) -> String {
    if crate::localization::current_language_is_chinese() {
        format!(
            "{} 个文件，{} 个文件夹",
            snapshot.recursive_contents.file_count, snapshot.recursive_contents.directory_count
        )
    } else {
        format!(
            "{} files, {} folders",
            snapshot.recursive_contents.file_count, snapshot.recursive_contents.directory_count
        )
    }
}

fn aggregate_type_label(snapshot: &FilePropertiesAggregateSnapshot) -> &'static str {
    match snapshot.common_kind {
        Some(FileKind::Directory) => "Folders",
        Some(FileKind::File) => "Files",
        Some(FileKind::Symlink) => "Symbolic Links",
        Some(FileKind::Other) => "Other Items",
        None => "Multiple Item Types",
    }
}

fn aggregate_optional_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time)
        .unwrap_or_else(|| crate::localization::translate_current("Mixed or unavailable"))
}

fn properties_permissions_detail<'a>(
    properties: &'a FilePropertiesState,
    presentation: &'a FilePropertiesPresentation,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    let displayed_permissions =
        displayed_permissions(presentation.permissions(), &properties.permission_update);
    let mut content = Column::new()
        .spacing(18)
        .push(permissions_header(displayed_permissions));

    if let Some(current_permissions) = displayed_permissions {
        content = content.push(permissions_people_panel(
            current_permissions,
            &properties.permission_update,
        ));
        if matches!(
            presentation,
            FilePropertiesPresentation::Single(snapshot)
                if snapshot.kind == FileKind::Directory
        ) {
            content = content.push(apply_permissions_to_enclosed_items_button(
                &properties.permission_update,
            ));
        }
    } else {
        content = content.push(permissions_unavailable_panel());
    }

    if let Some(status) = permissions_status_panel(&properties.permission_update) {
        content = content.push(status);
    }

    properties_detail_scroller(
        content.width(Length::Fill),
        scrollbar_visibility,
        scrollbar_viewport,
    )
}

fn permissions_header(permissions: Option<FilePropertiesPermissions>) -> Element<'static, Message> {
    let title = column![
        readable_text("Sharing & Permissions").size(20),
        readable_text(
            "Choose who can read, write, or execute this item. Changes save immediately."
        )
        .size(12)
    ]
    .spacing(4)
    .width(Length::Fill);

    let mut header = row![title]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if let Some(current_permissions) = permissions {
        header = header.push(permissions_mode_badge(current_permissions));
    }

    header.into()
}

fn properties_detail_scroller<'a>(
    content: Column<'a, Message>,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    auxiliary_detail_scroller(
        content,
        ScrollbarRegion::Properties,
        scrollbar_visibility,
        scrollbar_viewport,
        Message::PropertiesScrolled,
    )
}

fn displayed_permissions(
    permissions: Option<FilePropertiesPermissions>,
    update: &FilePropertiesPermissionUpdate,
) -> Option<FilePropertiesPermissions> {
    update.pending_permissions().or(permissions)
}

fn permissions_mode_badge(permissions: FilePropertiesPermissions) -> Element<'static, Message> {
    column![
        readable_text("Current mode").size(11),
        readable_text(display_permissions(permissions)).size(12)
    ]
    .spacing(2)
    .align_x(Alignment::End)
    .width(Length::Fixed(PERMISSION_PRIVILEGE_WIDTH))
    .into()
}

fn permissions_people_panel(
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Element<'static, Message> {
    let mut rows = Column::new().spacing(12).push(permissions_table_header());

    for class in FilePropertiesPermissionClass::ALL {
        rows = rows.push(permission_person_row(class, permissions, update));
    }

    container(rows)
        .padding([12, 14])
        .width(Length::Fill)
        .style(preview_panel_style)
        .into()
}

fn permissions_table_header() -> Element<'static, Message> {
    row![
        readable_text("Name").size(11).width(Length::Fill),
        readable_text("Privilege")
            .size(11)
            .width(Length::Fixed(PERMISSION_PRIVILEGE_WIDTH))
    ]
    .spacing(12)
    .padding([0, 2])
    .into()
}

fn permission_person_row(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Element<'static, Message> {
    let subject = column![
        readable_text(permission_subject_label(class)).size(13),
        readable_text(permission_subject_description(class)).size(11)
    ]
    .spacing(2)
    .width(Length::Fill);

    let content = column![
        row![subject, permission_privilege_badge(class, permissions),]
            .spacing(12)
            .align_y(Alignment::Center),
        permission_access_controls(class, permissions, update)
    ]
    .spacing(8);

    container(content)
        .padding([2, 0])
        .width(Length::Fill)
        .into()
}

fn permission_privilege_badge(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
) -> Element<'static, Message> {
    container(
        column![
            readable_text(permission_privilege_label(class, permissions)).size(12),
            readable_text(permission_symbolic_segment(class, permissions)).size(11)
        ]
        .spacing(2),
    )
    .padding([6, 10])
    .width(Length::Fixed(PERMISSION_PRIVILEGE_WIDTH))
    .style(context_menu_style)
    .into()
}

fn permission_access_controls(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Element<'static, Message> {
    let mut controls = row![].spacing(6).align_y(Alignment::Center);

    for access in FilePropertiesPermissionAccess::ALL {
        controls = controls.push(permission_access_button(class, access, permissions, update));
    }

    controls.width(Length::Fill).into()
}

fn permission_access_button(
    class: FilePropertiesPermissionClass,
    access: FilePropertiesPermissionAccess,
    permissions: FilePropertiesPermissions,
    update: &FilePropertiesPermissionUpdate,
) -> Button<'static, Message> {
    let is_enabled = permissions.contains(class, access);
    let label = row![
        readable_text(access.label()).size(12).width(Length::Fill),
        switch_control(is_enabled),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let label = container(label).padding([6, 8]).width(Length::Fill);
    let label = if is_enabled {
        label.style(selected_sidebar_item_style)
    } else {
        label
    };

    let button = button(label)
        .width(Length::FillPortion(1))
        .style(context_menu_button_style());

    if update.is_in_progress() {
        button
    } else {
        button.on_press(Message::FileProperties(
            FilePropertiesMessage::PermissionToggled(class, access),
        ))
    }
}

fn apply_permissions_to_enclosed_items_button(
    update: &FilePropertiesPermissionUpdate,
) -> Element<'static, Message> {
    let label = container(readable_text("Apply to Enclosed Items").size(13))
        .padding([8, 12])
        .width(Length::Fill);
    let button = button(label)
        .width(Length::Fill)
        .style(context_menu_button_style());

    if update.is_in_progress() {
        button.into()
    } else {
        button
            .on_press(Message::FileProperties(
                FilePropertiesMessage::ApplyPermissionsToEnclosedItems,
            ))
            .into()
    }
}

fn permissions_unavailable_panel() -> Element<'static, Message> {
    container(
        column![
            readable_text("Permissions are read-only").size(14),
            readable_text("Permission editing is unavailable for this item.").size(12)
        ]
        .spacing(4),
    )
    .padding([10, 12])
    .width(Length::Fill)
    .style(preview_panel_style)
    .into()
}

fn permissions_status_panel(
    update: &FilePropertiesPermissionUpdate,
) -> Option<Element<'static, Message>> {
    let message = match update {
        FilePropertiesPermissionUpdate::Idle => return None,
        FilePropertiesPermissionUpdate::SavingCurrentItem { .. } => {
            "Saving permissions...".to_owned()
        }
        FilePropertiesPermissionUpdate::SavingTargetSet { .. } => {
            "Saving permissions for selected items...".to_owned()
        }
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. } => {
            "Applying permissions to enclosed items...".to_owned()
        }
        FilePropertiesPermissionUpdate::TargetSetCompleted {
            succeeded_count,
            failures,
        } => {
            if failures.is_empty() {
                format!("Updated permissions for {succeeded_count} items.")
            } else {
                format!(
                    "Updated {succeeded_count} items; {} items failed. {}",
                    failures.len(),
                    failures
                        .iter()
                        .map(|failure| format!("{:?}: {}", failure.path, failure.error))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        }
        FilePropertiesPermissionUpdate::Failed(error) => {
            format!("Could not update permissions: {error}")
        }
    };

    Some(
        container(localized_text(message).size(12))
            .padding([8, 12])
            .width(Length::Fill)
            .style(preview_panel_style)
            .into(),
    )
}

fn permission_subject_label(class: FilePropertiesPermissionClass) -> &'static str {
    match class {
        FilePropertiesPermissionClass::Owner => "Owner",
        FilePropertiesPermissionClass::Group => "Group",
        FilePropertiesPermissionClass::Others => "Everyone",
    }
}

fn permission_subject_description(class: FilePropertiesPermissionClass) -> &'static str {
    match class {
        FilePropertiesPermissionClass::Owner => "Primary user",
        FilePropertiesPermissionClass::Group => "Assigned group",
        FilePropertiesPermissionClass::Others => "Other users",
    }
}

fn permission_privilege_label(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
) -> &'static str {
    match (
        permissions.contains(class, FilePropertiesPermissionAccess::Read),
        permissions.contains(class, FilePropertiesPermissionAccess::Write),
        permissions.contains(class, FilePropertiesPermissionAccess::Execute),
    ) {
        (true, true, true) => "Read, Write & Execute",
        (true, true, false) => "Read & Write",
        (true, false, true) => "Read & Execute",
        (true, false, false) => "Read only",
        (false, true, true) => "Write & Execute",
        (false, true, false) => "Write only",
        (false, false, true) => "Execute only",
        (false, false, false) => "No access",
    }
}

fn permission_symbolic_segment(
    class: FilePropertiesPermissionClass,
    permissions: FilePropertiesPermissions,
) -> String {
    FilePropertiesPermissionAccess::ALL
        .into_iter()
        .zip(['r', 'w', 'x'])
        .map(|(access, enabled_char)| {
            if permissions.contains(class, access) {
                enabled_char
            } else {
                '-'
            }
        })
        .collect()
}

fn property_row(label: &'static str, value: String) -> Element<'static, Message> {
    row![
        readable_text(label).width(Length::Fixed(PROPERTIES_LABEL_WIDTH)),
        readable_text(value).width(Length::Fill)
    ]
    .spacing(12)
    .align_y(Alignment::Start)
    .into()
}

fn display_name(name: &OsStr) -> String {
    name.to_string_lossy().into_owned()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn display_optional_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time)
        .unwrap_or_else(|| crate::localization::translate_current("Unavailable"))
}

fn display_size(bytes: u64) -> String {
    if crate::localization::current_language_is_chinese() {
        format!("{}（{bytes} 字节）", format_file_size(bytes))
    } else {
        format!("{} ({bytes} bytes)", format_file_size(bytes))
    }
}

fn display_contents(contents: &FilePropertiesDirectoryContents) -> String {
    if crate::localization::current_language_is_chinese() {
        format!(
            "{} 个文件，{} 个文件夹，总计 {}",
            contents.file_count,
            contents.directory_count,
            format_file_size(contents.total_size_bytes)
        )
    } else {
        format!(
            "{} files, {} folders, total {}",
            contents.file_count,
            contents.directory_count,
            format_file_size(contents.total_size_bytes)
        )
    }
}

fn display_permissions(permissions: FilePropertiesPermissions) -> String {
    format!(
        "{} ({})",
        permissions.symbolic_string(),
        permissions.octal_string()
    )
}
