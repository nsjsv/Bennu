use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use iced::widget::{column, container, image, row, Space};
use iced::{Alignment, Element, Length};

use super::dialog::{self, Dialog};
use super::{themed_icon, IconTone};
use crate::formatting::{format_file_size, format_middle_ellipsized_text, format_system_time};
use crate::model::{
    Message, TransferConflictChoice, TransferConflictItem, TransferConflictMetadata,
    TransferConflictState,
};
use crate::thumbnail_cache::{
    request_for_transfer_conflict_path, ThumbnailCache, TRANSFER_CONFLICT_THUMBNAIL_EDGE,
};
use bennu_preview::entry_icons::file_entry_icon_symbol;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::check_box;
use bennu_theme::controls::normal_button;
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::{BOLD, HEADING_SIZE};

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
        return Dialog::new(
            tr!("No pending conflicts"),
            Space::new().height(Length::Fixed(0.0)),
        )
        .into();
    };

    let body = dialog::fields(vec![
        conflict_file_section(
            ConflictFileSectionKind::Existing,
            &conflict.target_metadata,
            &conflict.target,
            thumbnail_cache,
        ),
        conflict_file_section(
            ConflictFileSectionKind::Incoming,
            &conflict.source_metadata,
            &conflict.source,
            thumbnail_cache,
        ),
        check_box(state.apply_to_all)
            .label(tr!("Apply this action to all files and folders"))
            .on_toggle(|_| Message::TransferConflictApplyToAllToggled)
            .into(),
    ]);

    // 「合并」只在源与目标都是目录时提供(与冲突项 can_merge 同源)。
    let merge_offered = conflict.can_merge();
    let choice = |label: String, choice: TransferConflictChoice| -> Element<'a, Message> {
        normal_button(readable_text(label))
            .on_press(Message::TransferConflictChoiceSelected(choice))
            .into()
    };
    let mut actions = vec![
        normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::TransferConflictCancelRequested)
            .into(),
        choice(tr!("Skip"), TransferConflictChoice::Skip),
        choice(tr!("Keep Both"), TransferConflictChoice::KeepBoth),
        choice(tr!("Replace"), TransferConflictChoice::Replace),
    ];
    if merge_offered {
        actions.push(choice(tr!("Merge"), TransferConflictChoice::Merge));
    }

    let mut dialog = Dialog::new(conflict_title(conflict), body)
        .actions(actions)
        .width(if merge_offered {
            TRANSFER_CONFLICT_PANEL_WIDTH_WITH_MERGE
        } else {
            TRANSFER_CONFLICT_PANEL_WIDTH
        });
    // 还有别的冲突时，标题下面写“第几个 / 一共几个”。
    if state.conflicts.len() > 1 {
        dialog = dialog.subtitle(format!(
            "{} / {}",
            state.current_index + 1,
            state.conflicts.len()
        ));
    }
    dialog.into()
}

fn conflict_title(conflict: &TransferConflictItem) -> String {
    let file_name = conflict
        .target
        .file_name()
        .or_else(|| conflict.source.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_owned());
    tr!(
        "This folder already contains a file named \"%1\".",
        format_middle_ellipsized_text(&file_name, TRANSFER_CONFLICT_NAME_MAX_CHARS)
    )
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
        readable_text(conflict_section_title(section_kind, path))
            .size(HEADING_SIZE)
            .font(BOLD),
        row![
            conflict_thumbnail(metadata, path, thumbnail_cache),
            column![
                readable_text(conflict_size_label(metadata)),
                readable_text(conflict_modified_label(metadata.modified)),
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
            tr!("Replace the existing file in \"%1\"", directory)
        }
        ConflictFileSectionKind::Incoming => tr!("Use the following file?"),
    }
}

fn conflict_size_label(metadata: &TransferConflictMetadata) -> String {
    if metadata.is_directory {
        tr!("Size: folder")
    } else {
        tr!(
            "Size: %1 (%2 bytes)",
            format_file_size(metadata.len),
            metadata.len
        )
    }
}

fn conflict_modified_label(modified: Option<SystemTime>) -> String {
    modified
        .map(|time| tr!("Modified: %1", format_system_time(time)))
        .unwrap_or_else(|| tr!("Modified: unknown"))
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
