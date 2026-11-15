use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::widget::image;

use iced::mouse;
use iced::Point;

use crate::app::FileBrowser;
use crate::config::ui_thread_startup_config;
use crate::model::{
    ImagePreviewContent, ImagePreviewViewport, Message, PreviewContent,
    PreviewImageViewportMessage, PreviewMessage, PreviewState,
};

fn browser_with_image(path: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let mut metadata = EntryMetadata::default();
    metadata.len = 1;
    browser.tab_mut().entries = vec![DirectoryEntry::new(
        path.to_path_buf(),
        FileKind::File,
        metadata,
        false,
        false,
        false,
    )]
    .into();
    browser
}

fn raster_result() -> bennu_preview::original_image_preview::OriginalImagePreview {
    bennu_preview::original_image_preview::OriginalImagePreview::Raster {
        raster_handle: image::Handle::from_rgba(4, 3, vec![0; 48]),
        placeholder_handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
        width: 4,
        height: 3,
        pixel_width: 4,
        has_alpha: false,
    }
}

fn thumbnail_content(path: PathBuf, handle: image::Handle) -> PreviewContent {
    PreviewContent::Image(ImagePreviewContent::Thumbnail {
        path,
        handle,
        width: 1,
        height: 1,
        max_edge: 512,
    })
}

#[test]
fn original_result_replaces_loading_preview_with_raster_content() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();

    let command =
        browser.accept_original_image_preview(path.clone(), generation, Ok(raster_result()));

    assert!(matches!(
        &browser.preview_engine.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                width: 4,
                height: 3,
                ..
            }
        )))
    ));
    assert!(browser.preview_engine.preview_window.is_some());
    assert!(command.units() > 0);
}

#[test]
fn original_result_replaces_thumbnail_placeholder() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    let thumbnail_handle = image::Handle::from_rgba(1, 1, vec![1, 2, 3, 255]);
    let thumbnail_id = thumbnail_handle.id();
    browser.preview_engine.preview = Some(PreviewState::Ready(thumbnail_content(
        path.clone(),
        thumbnail_handle,
    )));
    let preview_window = iced::window::Id::unique();
    browser.preview_engine.preview_window = Some(preview_window);
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();

    let command =
        browser.accept_original_image_preview(path.clone(), generation, Ok(raster_result()));

    assert!(matches!(
        &browser.preview_engine.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                placeholder_handle,
                ..
            }
        ))) if placeholder_handle.id() == thumbnail_id
    ));
    assert_eq!(browser.preview_engine.preview_window, Some(preview_window));
    // 窗口已开，不再开窗；只有把键盘交给图片（缩放键）的那一下。
    assert_eq!(command.units(), 1);
}

#[test]
fn stale_original_result_is_rejected_after_preview_clear() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
    let stale_generation = browser
        .preview_engine
        .next_original_image_preview_generation();
    browser.preview_engine.clear_preview();
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));

    drop(browser.accept_original_image_preview(path, stale_generation, Ok(raster_result())));

    assert!(matches!(
        browser.preview_engine.preview,
        Some(PreviewState::Loading(_))
    ));
}

#[test]
fn original_failure_preserves_path_for_retry() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();

    let command = browser.accept_original_image_preview(
        path.clone(),
        generation,
        Err("could not decode original image".to_owned()),
    );

    assert!(matches!(
        &browser.preview_engine.preview,
        Some(PreviewState::ImageError {
            path: current,
            error: message,
        }) if current == &path && message.contains("decode")
    ));
    assert!(browser.preview_engine.preview_window.is_some());
    assert!(command.units() > 0);

    drop(browser.retry_image_preview(path.clone()));
    assert!(matches!(
        &browser.preview_engine.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
}

#[test]
fn original_result_cannot_replace_existing_original_content() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::OriginalRaster {
            path: path.clone(),
            raster_handle: image::Handle::from_rgba(2, 2, vec![255; 16]),
            placeholder_handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 2,
            height: 2,
            pixel_width: 2,
            has_alpha: false,
        },
    )));
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();

    drop(browser.accept_original_image_preview(path, generation, Ok(raster_result())));

    assert!(matches!(
        browser.preview_engine.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                width: 2,
                height: 2,
                ..
            }
        )))
    ));
}

#[test]
fn clear_preview_drops_original_image_state() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::OriginalRaster {
            path: path.clone(),
            raster_handle: image::Handle::from_rgba(16, 16, vec![7; 1024]),
            placeholder_handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 16,
            height: 16,
            pixel_width: 16,
            has_alpha: false,
        },
    )));

    browser.preview_engine.clear_preview();

    assert!(browser.preview_engine.preview.is_none());
}

#[test]
fn clear_preview_drops_pending_original_image_request() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();
    drop(browser.accept_image_preview_dimensions(
        path,
        generation,
        Ok(bennu_preview::original_image_preview::ImageProbe {
            width: 320,
            height: 240,
            animated: false,
        }),
    ));
    assert!(browser
        .preview_engine
        .pending_preview_thumbnail_display
        .is_some());
    let cancellation = browser
        .preview_engine
        .original_image_preview_cancel
        .clone()
        .expect("pending original image request cancellation");

    browser.preview_engine.clear_preview();

    assert!(browser
        .preview_engine
        .pending_preview_thumbnail_display
        .is_none());
    assert!(cancellation.is_cancelled());
    assert!(browser
        .preview_engine
        .original_image_preview_cancel
        .is_none());
}

#[test]
fn new_original_request_drops_previous_pending_request() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser
        .preview_engine
        .next_original_image_preview_generation();
    drop(browser.accept_image_preview_dimensions(
        path,
        generation,
        Ok(bennu_preview::original_image_preview::ImageProbe {
            width: 320,
            height: 240,
            animated: false,
        }),
    ));
    assert!(browser
        .preview_engine
        .pending_preview_thumbnail_display
        .is_some());
    let previous_cancellation = browser
        .preview_engine
        .original_image_preview_cancel
        .clone()
        .expect("pending original image request cancellation");

    browser
        .preview_engine
        .next_original_image_preview_generation();

    assert!(browser
        .preview_engine
        .pending_preview_thumbnail_display
        .is_none());
    assert!(previous_cancellation.is_cancelled());
    assert!(browser
        .preview_engine
        .original_image_preview_cancel
        .as_ref()
        .is_some_and(|cancellation| !cancellation.is_cancelled()));
}

fn directory_entry_at(path: &Path) -> DirectoryEntry {
    let mut metadata = EntryMetadata::default();
    metadata.len = 1;
    DirectoryEntry::new(
        path.to_path_buf(),
        FileKind::Directory,
        metadata,
        false,
        false,
        false,
    )
}

fn file_entry_at(path: &Path) -> DirectoryEntry {
    let mut metadata = EntryMetadata::default();
    metadata.len = 1;
    DirectoryEntry::new(
        path.to_path_buf(),
        FileKind::File,
        metadata,
        false,
        false,
        false,
    )
}

fn directory_preview_content(root: &Path, names: &[(&str, FileKind)]) -> PreviewContent {
    let listing = names
        .iter()
        .map(|(name, kind)| {
            let path = root.join(name);
            match kind {
                FileKind::Directory => directory_entry_at(&path),
                _ => file_entry_at(&path),
            }
        })
        .collect();
    PreviewContent::Folder(Box::new(bennu_preview::folder_preview::FolderPreview::new(
        root.to_path_buf(),
        listing,
        file_core::ScanOptions::default(),
    )))
}

fn ready_folder(browser: &FileBrowser) -> &bennu_preview::folder_preview::FolderPreview {
    match &browser.preview_engine.preview {
        Some(PreviewState::Ready(PreviewContent::Folder(folder))) => folder,
        _ => panic!("folder preview expected"),
    }
}

fn is_expanded(node: &bennu_preview::folder_preview::FolderNode) -> bool {
    node.folder.as_ref().is_some_and(|state| state.expanded)
}

fn children(
    node: &bennu_preview::folder_preview::FolderNode,
) -> &[bennu_preview::folder_preview::FolderNode] {
    match &node.folder.as_ref().expect("folder").listing {
        bennu_preview::folder_preview::FolderListing::Loaded(children) => children,
        _ => panic!("listed folder expected"),
    }
}

fn folder_listed(browser: &mut FileBrowser, directory: PathBuf, listing: Vec<DirectoryEntry>) {
    let generation = browser.preview_engine.folder_preview_generation;
    drop(
        browser.update(Message::Preview(PreviewMessage::FolderPreview(
            bennu_preview::folder_preview::FolderPreviewMessage::Listed {
                generation,
                directory,
                outcome: Ok(listing),
            },
        ))),
    );
}

#[test]
fn directory_preview_auto_expands_root_directories_by_default() {
    let root = PathBuf::from("/workspace");
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.preview_engine.preview = Some(PreviewState::Loading(root.clone()));

    let command = browser.accept_preview(
        root.clone(),
        Ok(directory_preview_content(
            &root,
            &[("src", FileKind::Directory), ("README.md", FileKind::File)],
        )),
    );

    let entries = ready_folder(&browser).entries();
    assert!(is_expanded(&entries[0]));
    assert!(!is_expanded(&entries[1]));
    assert!(command.units() > 0);
}

#[test]
fn directory_preview_without_auto_expansion_stays_collapsed() {
    let root = PathBuf::from("/workspace");
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.user_config.preview_directory_expand_levels = 0;
    browser.preview_engine.preview = Some(PreviewState::Loading(root.clone()));

    drop(browser.accept_preview(
        root.clone(),
        Ok(directory_preview_content(
            &root,
            &[("src", FileKind::Directory)],
        )),
    ));

    assert!(!is_expanded(&ready_folder(&browser).entries()[0]));
}

/// 展开层数在打开时定下：这次会话里往下读到的文件夹都按它展开，之后改设置不影响。
#[test]
fn directory_preview_children_expand_up_to_configured_levels() {
    let root = PathBuf::from("/workspace");
    let src_dir = root.join("src");
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.user_config.preview_directory_expand_levels = 2;
    browser.preview_engine.preview = Some(PreviewState::Loading(root.clone()));
    drop(browser.accept_preview(
        root.clone(),
        Ok(directory_preview_content(
            &root,
            &[("src", FileKind::Directory)],
        )),
    ));

    folder_listed(
        &mut browser,
        src_dir.clone(),
        vec![
            directory_entry_at(&src_dir.join("app")),
            file_entry_at(&src_dir.join("main.rs")),
        ],
    );
    let src = &ready_folder(&browser).entries()[0];
    assert!(is_expanded(&children(src)[0]));

    browser.user_config.preview_directory_expand_levels = 1;
    folder_listed(
        &mut browser,
        src_dir.join("app"),
        vec![directory_entry_at(&src_dir.join("app").join("deep"))],
    );
    let src = &ready_folder(&browser).entries()[0];
    let deep = &children(&children(src)[0])[0];
    assert_eq!(deep.name, "deep");
    assert!(!is_expanded(deep));
}

#[test]
fn opening_a_new_preview_session_resets_the_image_viewport() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview_image_viewport.scale = 4.0;
    browser.preview_engine.preview_image_viewport.offset = Point::new(40.0, -20.0);

    drop(browser.open_preview_for_resolved_path(path.clone(), FileKind::File));

    assert_eq!(
        browser.preview_engine.preview_image_viewport,
        ImagePreviewViewport::default()
    );
}

#[test]
fn image_viewport_messages_are_ignored_without_a_ready_image_preview() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Loading(path));
    browser.preview_engine.preview_image_viewport.panning = true;

    drop(
        browser.update(Message::Preview(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::Zoomed(mouse::ScrollDelta::Lines { x: 0.0, y: 3.0 }),
        ))),
    );

    assert_eq!(browser.preview_engine.preview_image_viewport.scale, 1.0);
}

#[test]
fn zoom_and_reset_update_the_viewport_for_ready_image_previews() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);
    browser.preview_engine.preview = Some(PreviewState::Ready(thumbnail_content(
        path,
        image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
    )));

    drop(
        browser.update(Message::Preview(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::PointerMoved(Point::new(300.0, 200.0)),
        ))),
    );
    drop(
        browser.update(Message::Preview(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::Zoomed(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
        ))),
    );

    assert!(browser.preview_engine.preview_image_viewport.scale > 1.0);
    assert!(browser.preview_engine.preview_image_viewport.is_zoomed());

    drop(
        browser.update(Message::Preview(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::FitRequested,
        ))),
    );
    assert_eq!(
        browser.preview_engine.preview_image_viewport,
        ImagePreviewViewport::default()
    );
}

// 引擎经 Err 还回的变体必须都有宿主处理者，否则宿主的 unreachable 分支会 panic。
// 载荷复杂的变体（文档、远程缓存、加载结果）由各自的流程测试覆盖。
#[test]
fn every_message_the_engine_returns_has_a_host_handler() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_image(&path);

    for message in [
        PreviewMessage::SqliteTablesResizeStarted,
        PreviewMessage::MarkdownPreviewModeSelected(
            bennu_preview::text_preview::MarkdownPreviewMode::Raw,
        ),
        PreviewMessage::TextFileChanged { document: 7 },
        PreviewMessage::PreviewTreeDirectoryToggled(0),
        PreviewMessage::FolderEntryOpened {
            path: PathBuf::from("/workspace/notes.txt"),
            kind: FileKind::File,
        },
        PreviewMessage::PreviewImageViewport(PreviewImageViewportMessage::FitRequested),
        PreviewMessage::ImageStepRequested(bennu_preview::preview_message::ImageStep::Next),
        PreviewMessage::PreviewWindowPresented(iced::window::Id::unique()),
    ] {
        drop(browser.update(Message::Preview(message)));
    }
}

#[test]
fn end_action_button_cycles_the_three_main_app_modes_and_reaches_the_engine() {
    use bennu_preview::preview_config::MediaEndAction;
    let mut browser = browser_with_image(Path::new("/workspace/song.flac"));
    for expected in [
        MediaEndAction::Repeat,
        MediaEndAction::Next,
        MediaEndAction::Stop,
    ] {
        drop(browser.update(Message::Preview(PreviewMessage::MediaEndActionCycled)));
        assert_eq!(browser.user_config.media_end_action, expected);
        assert_eq!(browser.preview_engine.media_settings.end_action, expected);
    }
}

#[test]
fn next_track_request_for_a_track_no_longer_selected_does_nothing() {
    let mut browser = browser_with_image(Path::new("/workspace/photo.png"));
    let selected_before = browser.tab().selected.clone();
    drop(
        browser.update(Message::Preview(PreviewMessage::MediaNextRequested(
            PathBuf::from("/workspace/elsewhere.mp3"),
            bennu_preview::preview_loading::PreviewPathKind::Audio,
        ))),
    );
    assert_eq!(browser.tab().selected, selected_before);
    assert!(browser.preview_engine.queued_media.is_none());
}

/// 文件夹预览里双击：文件夹、能浏览的压缩包让主窗口进入；文件交给默认程序，主窗口不动。
#[test]
fn opening_from_the_folder_preview_enters_folders_and_archives_in_the_main_window() {
    let root = PathBuf::from("/workspace");
    for (path, kind, enters) in [
        (root.join("src"), FileKind::Directory, true),
        (root.join("backup.zip"), FileKind::File, true),
        (root.join("notes.txt"), FileKind::File, false),
    ] {
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        drop(browser.navigate_to(root.clone(), crate::model::NavigationMode::RecordHistory));

        drop(
            browser.update(Message::Preview(PreviewMessage::FolderEntryOpened {
                path: path.clone(),
                kind,
            })),
        );

        let expected = if enters { path } else { root.clone() };
        assert_eq!(
            browser.tab().location,
            crate::model::BrowserLocation::Directory(expected)
        );
    }
}
