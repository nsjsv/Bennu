use crate::app::FileBrowser;
use crate::config;
use crate::model::Message;

use super::settings_group::{entry_row, switch_row, EntryRow, RowNote, SettingsRow};

pub(super) fn network_thumbnails_row(browser: &FileBrowser) -> SettingsRow<'static> {
    switch_row(
        tr!("Remote List Thumbnails"),
        Some(RowNote::Hint(tr!(
            "When enabled, images and videos on remote locations may use more data."
        ))),
        browser.remote_list_thumbnail_downloads_enabled(),
        Message::NetworkListThumbnailDownloadsToggled,
    )
}

pub(super) fn preview_size_limit_rows(browser: &FileBrowser) -> Vec<SettingsRow<'_>> {
    let mut rows: Vec<SettingsRow<'_>> = config::PreviewFileSizeKind::ALL
        .iter()
        .enumerate()
        .map(|(kind_index, kind)| preview_size_limit_row(browser, kind_index, *kind))
        .collect();
    rows.push(preview_directory_expand_levels_row(browser));
    rows
}

fn preview_size_limit_row(
    browser: &FileBrowser,
    kind_index: usize,
    kind: config::PreviewFileSizeKind,
) -> SettingsRow<'_> {
    let title = match kind {
        config::PreviewFileSizeKind::Text => tr!("Max Text Preview"),
        config::PreviewFileSizeKind::Image => tr!("Max Image Preview"),
        config::PreviewFileSizeKind::Video => tr!("Max Video Preview"),
        config::PreviewFileSizeKind::Audio => tr!("Max Audio Preview"),
        config::PreviewFileSizeKind::Archive => tr!("Max Archive Preview"),
        config::PreviewFileSizeKind::Document => tr!("Max Document Preview"),
        config::PreviewFileSizeKind::Sqlite => tr!("Max SQLite Preview"),
    };
    let saved_limit = browser.user_config().preview_size_limits.limit(kind);
    let note = match &browser.preview_size_limit_mib_errors[kind_index] {
        Some(error) => Some(RowNote::Error(error.clone())),
        None if saved_limit == 0 => Some(RowNote::Hint(tr!(
            "Size is unlimited: previewing very large files may use a lot of memory."
        ))),
        None => None,
    };
    entry_row(EntryRow {
        title,
        saved: config::preview_size_limit_mib(saved_limit).to_string(),
        draft: &browser.preview_size_limit_mib_inputs[kind_index],
        placeholder: "0".to_owned(),
        on_input: Box::new(move |value| Message::PreviewSizeLimitInputChanged(kind_index, value)),
        submit: Message::PreviewSizeLimitInputCommitted(kind_index),
        note,
        unit: Some("MiB".to_owned()),
        trailing: Vec::new(),
    })
}

fn preview_directory_expand_levels_row(browser: &FileBrowser) -> SettingsRow<'_> {
    entry_row(EntryRow {
        title: tr!("Preview Expand Levels"),
        saved: browser
            .user_config()
            .preview_directory_expand_levels
            .to_string(),
        draft: &browser.preview_directory_expand_levels_input,
        placeholder: "1".to_owned(),
        on_input: Box::new(Message::PreviewDirectoryExpandLevelsInputChanged),
        submit: Message::PreviewDirectoryExpandLevelsInputCommitted,
        note: browser
            .preview_directory_expand_levels_error
            .clone()
            .map(RowNote::Error),
        unit: Some(tr!("levels")),
        trailing: Vec::new(),
    })
}
