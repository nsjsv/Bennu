use std::sync::Arc;

use file_core::FileKind;
use iced::advanced::widget::operation::Scrollable as ScrollableOperation;
use iced::futures::StreamExt;
use iced::widget::scrollable;
use iced::{Rectangle, Vector};
use iced_runtime::Action;
use tempfile::tempdir;

use super::*;
use crate::config;
use crate::document_preview::{
    DocumentPageRenderResult, DocumentPageRequestKey, DocumentPageSize, DocumentPageView,
    DocumentPreviewFormat, DocumentPreviewWorkspace, PreparedDocumentPreview,
};

#[derive(Default)]
struct RecordedScrollOffset {
    y: Option<f32>,
}

impl ScrollableOperation for RecordedScrollOffset {
    fn snap_to(&mut self, _offset: scrollable::RelativeOffset<Option<f32>>) {}

    fn scroll_to(&mut self, offset: scrollable::AbsoluteOffset<Option<f32>>) {
        self.y = offset.y;
    }

    fn scroll_by(
        &mut self,
        _offset: scrollable::AbsoluteOffset,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
    ) {
    }
}

fn prepare_current_document(browser: &mut FileBrowser, source: PathBuf, page_count: usize) {
    drop(browser.start_document_preview(source.clone()));
    let key = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .key
        .clone();
    let workspace = Arc::new(DocumentPreviewWorkspace::create_for_pdf(source).unwrap());
    let prepared = PreparedDocumentPreview {
        key,
        workspace,
        pages: vec![DocumentPageSize::from_crop_box(600.0, 800.0, 0).unwrap(); page_count],
    };
    drop(browser.accept_document_preview_prepared(DocumentPrepareOutcome::Ready(prepared)));
}

#[tokio::test]
async fn prepared_document_resets_stable_scrollable_before_page_commands() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("reset.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.start_document_preview(source.clone()));
    let key = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .key
        .clone();
    let prepared = PreparedDocumentPreview {
        key,
        workspace: Arc::new(DocumentPreviewWorkspace::create_for_pdf(source).unwrap()),
        pages: vec![DocumentPageSize::from_crop_box(600.0, 800.0, 0).unwrap(); 4],
    };

    let task = browser.accept_document_preview_prepared(DocumentPrepareOutcome::Ready(prepared));
    let mut stream = iced_runtime::task::into_stream(task).expect("prepared document task");
    let action = stream
        .next()
        .await
        .expect("scroll action before page commands");
    let Action::Widget(mut operation) = action else {
        panic!("first prepared-document action must reset the scrollable");
    };
    let mut recorded = RecordedScrollOffset::default();
    let id = smooth_scroll_id(&ScrollbarRegion::PreviewDocument);
    operation.scrollable(
        Some(&id.into()),
        Rectangle::default(),
        Rectangle::default(),
        Vector::default(),
        &mut recorded,
    );

    assert_eq!(recorded.y, Some(0.0));
    assert_eq!(
        browser
            .active_document_preview_mut()
            .unwrap()
            .viewport_offset(),
        0.0
    );
}

#[test]
fn stale_and_invalid_scrolls_do_not_change_scrollbar_state() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("stale-scroll.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    prepare_current_document(&mut browser, source, 6);
    drop(browser.start_scrollbar_reveal(ScrollbarRegion::Sidebar));

    let mut stale_key = browser
        .active_document_preview_mut()
        .unwrap()
        .viewport_key();
    stale_key.layout_generation = stale_key.layout_generation.wrapping_add(1);
    let content_height = browser
        .active_document_preview_mut()
        .unwrap()
        .content_height();
    drop(
        browser.handle_document_preview_message(DocumentPreviewMessage::Scrolled {
            key: stale_key,
            offset_y: 100.0,
            viewport_height: 400.0,
            content_height,
        }),
    );
    assert!(
        browser
            .scrollbar_visibility_for(&ScrollbarRegion::Sidebar)
            .opacity()
            > 0.0
    );

    let current_key = browser
        .active_document_preview_mut()
        .unwrap()
        .viewport_key();
    drop(
        browser.handle_document_preview_message(DocumentPreviewMessage::Scrolled {
            key: current_key,
            offset_y: f32::NAN,
            viewport_height: 400.0,
            content_height,
        }),
    );
    assert!(
        browser
            .scrollbar_visibility_for(&ScrollbarRegion::Sidebar)
            .opacity()
            > 0.0
    );
    assert_eq!(
        browser.scrollbar_visibility_for(&ScrollbarRegion::PreviewDocument),
        crate::model::ScrollbarVisibility::Hidden
    );
}

#[test]
fn office_document_dispatch_enters_the_typed_document_pipeline() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/tmp/report.DOCX");

    drop(browser.open_preview_for_resolved_path(source.clone(), FileKind::File));

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Loading(ref path)) if path == &source
    ));
    assert_eq!(
        browser
            .pending_document_preview
            .as_ref()
            .unwrap()
            .key
            .source_path,
        source
    );
    assert!(browser.text_preview_document.is_none());
}

#[test]
fn office_prepare_failure_stays_local_and_never_falls_back_to_text() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/tmp/broken.docx");
    drop(browser.start_document_preview(source));
    let key = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .key
        .clone();

    drop(
        browser.accept_document_preview_prepared(DocumentPrepareOutcome::Failed(
            key,
            "Could not convert Office document: broken fixture".to_owned(),
        )),
    );

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Error(ref error)) if error.contains("Office document")
    ));
    assert!(browser.text_preview_document.is_none());
    assert!(browser.current_error().is_none());
}

#[test]
fn same_path_reopen_rejects_old_prepare_generation() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("same.pdf");
    std::fs::write(&source, b"pdf").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.start_document_preview(source.clone()));
    let first = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .key
        .clone();
    browser.clear_preview();
    drop(browser.start_document_preview(source.clone()));
    let second = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .key
        .clone();
    assert_ne!(first.document_generation, second.document_generation);

    drop(
        browser.accept_document_preview_prepared(DocumentPrepareOutcome::Failed(
            first,
            "old failure".to_owned(),
        )),
    );
    assert!(matches!(
        browser.preview,
        Some(PreviewState::Loading(ref path)) if path == &source
    ));
    assert_eq!(
        browser.pending_document_preview.as_ref().unwrap().key,
        second
    );
}

#[test]
fn clear_preview_cancels_pending_and_ready_document_tokens() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("cancel.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.start_document_preview(source.clone()));
    let pending_token = browser
        .pending_document_preview
        .as_ref()
        .unwrap()
        .cancellation
        .clone();
    browser.clear_preview();
    assert!(pending_token.is_cancelled());

    prepare_current_document(&mut browser, source, 2);
    let ready_token = browser
        .active_document_preview_mut()
        .unwrap()
        .document_cancellation();
    browser.clear_preview();
    assert!(ready_token.is_cancelled());
}

#[test]
fn stale_page_and_scroll_are_rejected_after_width_bucket_resize() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("resize.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    prepare_current_document(&mut browser, source.clone(), 8);

    let old_viewport_key = browser
        .active_document_preview_mut()
        .unwrap()
        .viewport_key();
    let old_page_key = DocumentPageRequestKey {
        render: old_viewport_key.render.clone(),
        page_index: 0,
    };
    browser.preview_size.width = 1100.0;
    drop(browser.resize_document_preview());
    let resized = browser.active_document_preview_mut().unwrap();
    assert_ne!(resized.render_key(), old_page_key.render);
    let offset_after_resize = resized.viewport_offset();
    let content_height = resized.content_height();

    drop(
        browser.accept_document_page_rendered(DocumentPageRenderOutcome::Ready(
            DocumentPageRenderResult {
                key: old_page_key,
                handle: iced::widget::image::Handle::from_bytes(vec![1]),
                width: 1,
                height: 1,
                rgba_bytes: 4,
            },
        )),
    );
    drop(browser.handle_document_preview_scrolled(
        old_viewport_key,
        offset_after_resize + 100.0,
        400.0,
        content_height,
    ));

    let document = browser.active_document_preview_mut().unwrap();
    assert_eq!(document.viewport_offset(), offset_after_resize);
    assert!(!matches!(document.page_view(0), DocumentPageView::Ready(_)));
}

#[test]
fn repeated_same_size_resize_keeps_document_viewport_key() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("same-size.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    prepare_current_document(&mut browser, source, 4);
    let old_key = browser
        .active_document_preview_mut()
        .unwrap()
        .viewport_key();

    drop(browser.resize_document_preview());

    assert_eq!(
        browser
            .active_document_preview_mut()
            .unwrap()
            .viewport_key(),
        old_key
    );
}

#[test]
fn height_resize_advances_only_layout_generation_and_rejects_old_widget_event() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("height.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    prepare_current_document(&mut browser, source, 12);

    let old_key = browser
        .active_document_preview_mut()
        .unwrap()
        .viewport_key();
    browser.preview_size.height += 120.0;
    drop(browser.resize_document_preview());
    let document = browser.active_document_preview_mut().unwrap();
    let new_key = document.viewport_key();
    let stable_offset = document.viewport_offset();
    let content_height = document.content_height();
    assert_eq!(old_key.render, new_key.render);
    assert_ne!(old_key.layout_generation, new_key.layout_generation);

    drop(browser.handle_document_preview_scrolled(
        old_key,
        stable_offset + 200.0,
        500.0,
        content_height,
    ));
    assert_eq!(
        browser
            .active_document_preview_mut()
            .unwrap()
            .viewport_offset(),
        stable_offset
    );
}

#[test]
fn page_failure_stays_local_and_does_not_touch_global_error() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("failure.pdf");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    prepare_current_document(&mut browser, source, 2);
    let key = {
        let document = browser.active_document_preview_mut().unwrap();
        DocumentPageRequestKey {
            render: document.render_key(),
            page_index: document.current_render_pages()[0],
        }
    };

    drop(
        browser.accept_document_page_rendered(DocumentPageRenderOutcome::Failed(
            key,
            "Could not render PDF page: broken page".to_owned(),
        )),
    );

    assert!(browser.current_error().is_none());
    assert!(matches!(
        browser.active_document_preview_mut().unwrap().page_view(0),
        DocumentPageView::Error(_)
    ));
}
