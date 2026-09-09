use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use iced::widget::{button, checkbox, column, container, image, row, Button, Space, Svg};
use iced::{Alignment, Element, Length, Theme};

use crate::appearance::context_menu_style;
use crate::formatting::{format_file_size, format_middle_ellipsized_text, format_system_time};
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::model::{
    Message, TransferConflictChoice, TransferConflictItem, TransferConflictMetadata,
    TransferConflictState,
};
use crate::thumbnail_cache::{
    request_for_transfer_conflict_path, ThumbnailCache, TRANSFER_CONFLICT_THUMBNAIL_EDGE,
};
use crate::typography::readable_text;

use super::option_controls::secondary_action_button_style;
use super::{themed_icon, IconTone};

const TRANSFER_CONFLICT_PANEL_WIDTH: f32 = 560.0;
const TRANSFER_CONFLICT_PANEL_WIDTH_WITH_MERGE: f32 = 700.0;
const TRANSFER_CONFLICT_PATH_MAX_CHARS: usize = 32;
const TRANSFER_CONFLICT_NAME_MAX_CHARS: usize = 48;
const TRANSFER_CONFLICT_THUMBNAIL_SIZE: f32 = 48.0;

pub(super) fn transfer_conflict_panel<'a>(
    state: &'a TransferConflictState,
    thumbnail_cache: &'a ThumbnailCache,
) -> Element<'a, Message> {
    let Some(conflict) = state.current_conflict() else {
        return container(readable_text("No pending conflicts").size(14))
            .padding(14)
            .width(Length::Fixed(TRANSFER_CONFLICT_PANEL_WIDTH))
            .style(context_menu_style)
            .into();
    };

    let title = row![
        conflict_icon(conflict),
        readable_text(conflict_title(conflict))
            .size(18)
            .width(Length::Fill),
        conflict_count_label(state),
    ]
    .spacing(8)
    .align_y(Alignment::Start);

    let source_section = conflict_file_section(
        ConflictFileSectionKind::Existing,
        &conflict.target_metadata,
        &conflict.target,
        thumbnail_cache,
    );
    let incoming_section = conflict_file_section(
        ConflictFileSectionKind::Incoming,
        &conflict.source_metadata,
        &conflict.source,
        thumbnail_cache,
    );

    let apply_to_all = checkbox(state.apply_to_all)
        .label(crate::localization::translate_current(
            "Apply this action to all files and folders",
        ))
        .on_toggle(|_| Message::TransferConflictApplyToAllToggled)
        .size(18)
        .text_size(14)
        .spacing(8);

    // 「合并」只在源与目标都是目录时提供(与冲突项 can_merge 同源)。
    let merge_offered = conflict.can_merge();
    let mut actions = row![
        conflict_action_button("Cancel", Message::TransferConflictCancelRequested),
        Space::new().width(Length::Fill),
        conflict_action_button(
            "Skip",
            Message::TransferConflictChoiceSelected(TransferConflictChoice::Skip),
        ),
        Space::new().width(Length::Fill),
        conflict_action_button(
            "Rename",
            Message::TransferConflictChoiceSelected(TransferConflictChoice::Rename),
        ),
        Space::new().width(Length::Fill),
        conflict_action_button(
            "Keep Both",
            Message::TransferConflictChoiceSelected(TransferConflictChoice::KeepBoth),
        ),
        Space::new().width(Length::Fill),
        conflict_action_button(
            "Replace",
            Message::TransferConflictChoiceSelected(TransferConflictChoice::Replace),
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if merge_offered {
        actions = actions
            .push(Space::new().width(Length::Fill))
            .push(conflict_action_button(
                "Merge",
                Message::TransferConflictChoiceSelected(TransferConflictChoice::Merge),
            ));
    }

    let content = column![
        title,
        source_section,
        incoming_section,
        apply_to_all,
        actions,
    ]
    .spacing(13)
    .width(Length::Fill);

    let panel_width = if merge_offered {
        TRANSFER_CONFLICT_PANEL_WIDTH_WITH_MERGE
    } else {
        TRANSFER_CONFLICT_PANEL_WIDTH
    };
    container(content)
        .padding([14, 12])
        .width(Length::Fixed(panel_width))
        .style(context_menu_style)
        .into()
}

fn conflict_title(conflict: &TransferConflictItem) -> String {
    let file_name = conflict
        .target
        .file_name()
        .or_else(|| conflict.source.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_owned());
    let file_name = format_middle_ellipsized_text(&file_name, TRANSFER_CONFLICT_NAME_MAX_CHARS);
    if crate::localization::current_language_is_chinese() {
        format!("此文件夹已包含名为“{file_name}”的文件。")
    } else {
        format!("This folder already contains a file named \"{file_name}\".")
    }
}

fn conflict_count_label(state: &TransferConflictState) -> Element<'_, Message> {
    if state.conflicts.len() <= 1 {
        return Space::new().into();
    }

    readable_text(format!(
        "{} / {}",
        state.current_index + 1,
        state.conflicts.len()
    ))
    .size(12)
    .into()
}

#[derive(Debug, Clone, Copy)]
enum ConflictFileSectionKind {
    Existing,
    Incoming,
}

fn conflict_file_section<'a>(
    section_kind: ConflictFileSectionKind,
    metadata: &TransferConflictMetadata,
    path: &Path,
    thumbnail_cache: &'a ThumbnailCache,
) -> Element<'a, Message> {
    column![
        readable_text(conflict_section_title(section_kind, path)).size(15),
        row![
            conflict_thumbnail(metadata, path, thumbnail_cache),
            column![
                readable_text(conflict_size_label(metadata)).size(14),
                readable_text(conflict_modified_label(metadata.modified)).size(14),
            ]
            .spacing(3)
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([0, 8]),
    ]
    .spacing(8)
    .into()
}

fn conflict_section_title(section_kind: ConflictFileSectionKind, path: &Path) -> String {
    let directory = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let directory = format_middle_ellipsized_text(&directory, TRANSFER_CONFLICT_PATH_MAX_CHARS);

    match section_kind {
        ConflictFileSectionKind::Existing => {
            if crate::localization::current_language_is_chinese() {
                format!("替换“{directory}”中的现有文件")
            } else {
                format!("Replace the existing file in \"{directory}\"")
            }
        }
        ConflictFileSectionKind::Incoming => {
            if crate::localization::current_language_is_chinese() {
                "使用以下文件？".to_owned()
            } else {
                "Use the following file?".to_owned()
            }
        }
    }
}

fn conflict_size_label(metadata: &TransferConflictMetadata) -> String {
    if metadata.is_directory {
        if crate::localization::current_language_is_chinese() {
            "大小：文件夹".to_owned()
        } else {
            "Size: folder".to_owned()
        }
    } else {
        if crate::localization::current_language_is_chinese() {
            format!(
                "大小：{}（{} 字节）",
                format_file_size(metadata.len),
                metadata.len
            )
        } else {
            format!(
                "Size: {} ({} bytes)",
                format_file_size(metadata.len),
                metadata.len
            )
        }
    }
}

fn conflict_modified_label(modified: Option<SystemTime>) -> String {
    modified
        .map(|time| {
            if crate::localization::current_language_is_chinese() {
                format!("修改时间：{}", format_system_time(time))
            } else {
                format!("Modified: {}", format_system_time(time))
            }
        })
        .unwrap_or_else(|| {
            if crate::localization::current_language_is_chinese() {
                "修改时间：未知".to_owned()
            } else {
                "Modified: unknown".to_owned()
            }
        })
}

fn conflict_icon(conflict: &TransferConflictItem) -> Svg<'static, Theme> {
    themed_icon(
        conflict_icon_symbol(&conflict.target_metadata, &conflict.target),
        IconTone::Normal,
        18.0,
    )
}

fn conflict_thumbnail(
    metadata: &TransferConflictMetadata,
    path: &Path,
    thumbnail_cache: &ThumbnailCache,
) -> Element<'static, Message> {
    if let Some(thumbnail) =
        request_for_transfer_conflict_path(path, metadata, TRANSFER_CONFLICT_THUMBNAIL_EDGE)
            .and_then(|request| thumbnail_cache.ready_for_request(&request))
    {
        return container(
            image::Image::new(thumbnail.handle.clone())
                .width(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
                .height(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE)),
        )
        .width(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
        .height(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
        .into();
    }

    container(themed_icon(
        conflict_icon_symbol(metadata, path),
        IconTone::Normal,
        22.0,
    ))
    .width(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
    .height(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
    .center_x(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
    .center_y(Length::Fixed(TRANSFER_CONFLICT_THUMBNAIL_SIZE))
    .into()
}

fn conflict_icon_symbol(metadata: &TransferConflictMetadata, path: &Path) -> IconSymbol {
    if metadata.is_directory {
        IconSymbol::Folder
    } else {
        file_entry_icon_symbol(
            file_core::FileKind::File,
            path.file_name().unwrap_or_else(|| OsStr::new("")),
        )
    }
}

fn conflict_action_button(label: &'static str, message: Message) -> Button<'static, Message> {
    button(
        container(readable_text(label).size(14))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .on_press(message)
    .padding(0)
    .width(Length::Fixed(98.0))
    .height(Length::Fixed(32.0))
    .style(secondary_action_button_style())
}
