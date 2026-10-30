use iced::widget::{button, column, container, row, text_input};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::appearance::context_menu_button_style;
use crate::config;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;

use super::settings_group::{info_setting_row, toggle_setting_row};

pub(super) fn network_thumbnails_row(browser: &FileBrowser) -> Element<'static, Message> {
    toggle_setting_row(
        tr!("Remote List Thumbnails"),
        Some(tr!(
            "When enabled, images and videos on remote locations may use more data."
        )),
        browser.remote_list_thumbnail_downloads_enabled(),
        Message::NetworkListThumbnailDownloadsToggled,
    )
}

pub(super) fn preview_size_limit_rows(browser: &FileBrowser) -> Element<'_, Message> {
    let mut rows = column![].spacing(3);
    for (kind_index, kind) in config::PreviewFileSizeKind::ALL.iter().enumerate() {
        rows = rows.push(preview_size_limit_row(browser, kind_index, *kind));
    }
    rows.push(preview_directory_expand_levels_row(browser))
        .into()
}

fn preview_size_limit_row(
    browser: &FileBrowser,
    kind_index: usize,
    kind: config::PreviewFileSizeKind,
) -> Element<'_, Message> {
    let label = match kind {
        config::PreviewFileSizeKind::Text => tr!("Max Text Preview"),
        config::PreviewFileSizeKind::Image => tr!("Max Image Preview"),
        config::PreviewFileSizeKind::Video => tr!("Max Video Preview"),
        config::PreviewFileSizeKind::Audio => tr!("Max Audio Preview"),
        config::PreviewFileSizeKind::Archive => tr!("Max Archive Preview"),
        config::PreviewFileSizeKind::Document => tr!("Max Document Preview"),
        config::PreviewFileSizeKind::Sqlite => tr!("Max SQLite Preview"),
    };
    let input = text_input("0", &browser.preview_size_limit_mib_inputs[kind_index])
        .on_input(move |value| Message::PreviewSizeLimitInputChanged(kind_index, value))
        .on_submit(Message::PreviewSizeLimitInputCommitted(kind_index))
        .padding([6, 8])
        .size(12)
        .width(Length::Fixed(120.0));
    let save = button(container(readable_text(tr!("Save")).size(12)).padding([6, 10]))
        .on_press(Message::PreviewSizeLimitInputCommitted(kind_index))
        .style(context_menu_button_style());

    let mut content = column![row![
        readable_text(label).size(12).width(Length::Fill),
        input,
        readable_text("MiB".to_owned()).size(12),
        save,
    ]
    .spacing(8)
    .align_y(Alignment::Center)]
    .spacing(3);

    if browser.user_config().preview_size_limits.limit(kind) == 0 {
        content = content.push(
            readable_text(tr!(
                "Size is unlimited: previewing very large files may use a lot of memory."
            ))
            .size(11)
            .width(Length::Fill),
        );
    }
    if let Some(error) = &browser.preview_size_limit_mib_errors[kind_index] {
        content = content.push(readable_text(error.clone()).size(11).width(Length::Fill));
    }

    info_setting_row(content.into())
}

fn preview_directory_expand_levels_row(browser: &FileBrowser) -> Element<'_, Message> {
    let input = text_input("1", &browser.preview_directory_expand_levels_input)
        .on_input(Message::PreviewDirectoryExpandLevelsInputChanged)
        .on_submit(Message::PreviewDirectoryExpandLevelsInputCommitted)
        .padding([6, 8])
        .size(12)
        .width(Length::Fixed(120.0));
    let save = button(container(readable_text(tr!("Save")).size(12)).padding([6, 10]))
        .on_press(Message::PreviewDirectoryExpandLevelsInputCommitted)
        .style(context_menu_button_style());

    let mut content = column![row![
        readable_text(tr!("Preview Expand Levels"))
            .size(12)
            .width(Length::Fill),
        input,
        readable_text(tr!("levels")).size(12),
        save,
    ]
    .spacing(8)
    .align_y(Alignment::Center)]
    .spacing(3);

    if let Some(error) = &browser.preview_directory_expand_levels_error {
        content = content.push(readable_text(error.clone()).size(11).width(Length::Fill));
    }

    info_setting_row(content.into())
}
