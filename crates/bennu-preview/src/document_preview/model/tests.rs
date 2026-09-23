use super::super::resources::{
    page_render_plan, select_render_set, ReadyDocumentPage, ReadyDocumentPageCache,
    RenderCandidate, MAX_DOCUMENT_PAGE_EDGE, MAX_DOCUMENT_PAGE_PIXELS, MAX_READY_DOCUMENT_PAGES,
    MAX_READY_DOCUMENT_RGBA_BYTES,
};
use super::*;

fn page(width: f64, height: f64) -> DocumentPageSize {
    DocumentPageSize::from_crop_box(width, height, 0).unwrap()
}

fn preview_with_pages(pages: Vec<DocumentPageSize>) -> PagedDocumentPreview {
    let workspace = Arc::new(
        DocumentPreviewWorkspace::create_for_pdf(PathBuf::from("/tmp/document.pdf")).unwrap(),
    );
    PagedDocumentPreview::new(
        PreparedDocumentPreview {
            key: DocumentPreviewRequestKey {
                source_path: PathBuf::from("/tmp/document.pdf"),
                document_generation: 7,
            },
            workspace,
            pages,
        },
        CancellationToken::new(),
        720.0,
        440.0,
    )
    .unwrap()
}

fn ready_result(request: &DocumentPageRenderRequest, rgba_bytes: u64) -> DocumentPageRenderOutcome {
    DocumentPageRenderOutcome::Ready(DocumentPageRenderResult {
        key: request.key.clone(),
        handle: image::Handle::from_bytes(vec![1, 2, 3]),
        width: request.plan.width,
        height: request.plan.height,
        rgba_bytes,
    })
}

#[test]
fn cumulative_layout_height_overflow_is_rejected() {
    let page_width = document_page_width(720.0);
    let finite_display_height = f32::MAX as f64 * 0.75;
    let huge_page =
        DocumentPageSize::from_crop_box(1.0, finite_display_height / f64::from(page_width), 0)
            .unwrap();
    let computed_height = (f64::from(page_width) * huge_page.height / huge_page.width) as f32;
    assert!(computed_height.is_finite());

    let workspace = Arc::new(
        DocumentPreviewWorkspace::create_for_pdf(PathBuf::from("/tmp/overflow.pdf")).unwrap(),
    );
    let error = PagedDocumentPreview::new(
        PreparedDocumentPreview {
            key: DocumentPreviewRequestKey {
                source_path: PathBuf::from("/tmp/overflow.pdf"),
                document_generation: 8,
            },
            workspace,
            pages: vec![huge_page; 2],
        },
        CancellationToken::new(),
        720.0,
        440.0,
    )
    .unwrap_err();

    assert_eq!(error, "PDF document layout height overflowed");
}

#[test]
fn wanted_window_prefetches_one_page_at_start_middle_and_end() {
    let pages = vec![page(600.0, 300.0); 8];
    let mut preview = preview_with_pages(pages);

    assert_eq!(preview.wanted_pages(), &[0, 1, 2]);

    let middle_key = preview.viewport_key();
    assert!(preview.update_viewport(&middle_key, 1200.0, 300.0));
    assert_eq!(preview.wanted_pages(), &[2, 3, 4, 5]);

    let end_key = preview.viewport_key();
    assert!(preview.update_viewport(&end_key, preview.content_height() - 300.0, 300.0));
    assert_eq!(preview.wanted_pages(), &[6, 7]);
}

#[test]
fn jumping_to_last_page_replaces_queue_without_requesting_middle_pages() {
    let pages = vec![page(600.0, 100.0); 100];
    let mut preview = preview_with_pages(pages);
    let initial = preview.drain_render_requests(2);
    assert!(initial.iter().all(|request| request.key.page_index <= 4));

    let key = preview.viewport_key();
    assert!(preview.update_viewport(&key, preview.content_height() - 300.0, 300.0));
    assert!(preview
        .queued_pages()
        .iter()
        .all(|page_index| *page_index >= 95));

    for request in initial {
        assert!(preview.accept_page_outcome(DocumentPageRenderOutcome::Cancelled(request.key)));
    }
    let last = preview.drain_render_requests(2);
    assert!(last.iter().all(|request| request.key.page_index >= 95));
}

#[test]
fn render_capacity_prefers_visible_nearest_page_deterministically() {
    let selected = select_render_set(vec![
        RenderCandidate {
            page_index: 4,
            estimated_rgba_bytes: 40 * 1024 * 1024,
            intersects_viewport: true,
            center_distance: 80.0,
        },
        RenderCandidate {
            page_index: 5,
            estimated_rgba_bytes: 40 * 1024 * 1024,
            intersects_viewport: true,
            center_distance: 20.0,
        },
        RenderCandidate {
            page_index: 3,
            estimated_rgba_bytes: 1024,
            intersects_viewport: false,
            center_distance: 1.0,
        },
    ]);

    assert_eq!(selected, vec![5, 3]);
}

#[test]
fn ready_cache_enforces_page_and_rgba_limits_with_historical_lru() {
    let mut cache = ReadyDocumentPageCache::default();
    for page_index in 0..7 {
        cache.insert(
            page_index,
            ReadyDocumentPage {
                handle: image::Handle::from_bytes(vec![page_index as u8]),
                rgba_bytes: 8 * 1024 * 1024,
            },
        );
    }
    cache.reserve_for_active(&[(6, 8 * 1024 * 1024)]);

    assert_eq!(cache.pages.len(), 6);
    assert!(!cache.pages.contains_key(&0));
    assert!(cache.pages.contains_key(&6));

    cache.insert(
        7,
        ReadyDocumentPage {
            handle: image::Handle::from_bytes(vec![7]),
            rgba_bytes: 32 * 1024 * 1024,
        },
    );
    cache.reserve_for_active(&[(7, 32 * 1024 * 1024)]);
    assert!(cache.rgba_bytes <= MAX_READY_DOCUMENT_RGBA_BYTES);
    assert!(cache.pages.len() <= MAX_READY_DOCUMENT_PAGES);
    assert!(cache.pages.contains_key(&7));
}

#[test]
fn identical_resize_keeps_layout_and_render_generations() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 5]);
    let initial_viewport = preview.viewport_key();
    let initial_offset = preview.viewport_offset();

    let restored = preview.resize(720.0, 440.0).unwrap();

    assert_eq!(preview.viewport_key(), initial_viewport);
    assert_eq!(preview.render_key(), initial_viewport.render);
    assert_eq!(restored, initial_offset);
}

#[test]
fn resize_preserves_anchor_and_separates_layout_from_render_generation() {
    let pages = vec![page(600.0, 800.0); 5];
    let mut preview = preview_with_pages(pages);
    let initial_render = preview.render_key();
    let initial_layout = preview.viewport_key().layout_generation;
    let scroll_key = preview.viewport_key();
    assert!(preview.update_viewport(&scroll_key, 1100.0, 300.0));
    let anchor_page = page_at_offset(
        &preview.page_tops,
        &preview.display_heights,
        preview.viewport_offset,
    );

    let restored = preview.resize(740.0, 500.0).unwrap();
    assert_eq!(preview.render_key(), initial_render);
    assert_ne!(preview.viewport_key().layout_generation, initial_layout);
    assert_eq!(
        page_at_offset(&preview.page_tops, &preview.display_heights, restored),
        anchor_page
    );

    let before_bucket_change = preview.render_key();
    preview.resize(1000.0, 500.0).unwrap();
    assert_ne!(preview.render_key(), before_bucket_change);
}

#[test]
fn scroll_viewport_height_change_advances_layout_generation() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 8]);
    let old_key = preview.viewport_key();
    let old_render = preview.render_key();

    assert!(preview.update_viewport(&old_key, 0.0, 420.0));
    assert_eq!(preview.render_key(), old_render);
    assert_ne!(preview.viewport_key(), old_key);
    assert!(!preview.update_viewport(&old_key, 0.0, 440.0));
}

#[test]
fn height_resize_invalidates_old_viewport_key_without_changing_render_key() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 3]);
    let old_viewport = preview.viewport_key();
    let old_render = preview.render_key();

    preview.resize(720.0, 620.0).unwrap();

    assert_eq!(preview.render_key(), old_render);
    assert!(!preview.update_viewport(&old_viewport, 100.0, 300.0));
}

#[test]
fn stale_page_result_is_rejected_and_current_failure_does_not_enter_ready_cache() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 3]);
    let request = preview.drain_render_requests(1).remove(0);
    let mut stale_key = request.key.clone();
    stale_key.render.render_generation += 1;
    assert!(
        !preview.accept_page_outcome(DocumentPageRenderOutcome::Ready(DocumentPageRenderResult {
            key: stale_key,
            handle: image::Handle::from_bytes(vec![1]),
            width: 1,
            height: 1,
            rgba_bytes: 4,
        }))
    );
    assert_eq!(preview.ready_page_count(), 0);

    assert!(
        preview.accept_page_outcome(DocumentPageRenderOutcome::Failed(
            request.key.clone(),
            "fixture failure".to_owned(),
        ))
    );
    assert_eq!(preview.ready_page_count(), 0);
    assert_eq!(preview.ready_rgba_bytes(), 0);
    assert!(matches!(
        preview.page_view(request.key.page_index),
        DocumentPageView::Error("fixture failure")
    ));
}

#[test]
fn result_for_page_that_left_render_set_is_discarded() {
    let mut preview = preview_with_pages(vec![page(600.0, 100.0); 80]);
    let request = preview.drain_render_requests(1).remove(0);
    let key = preview.viewport_key();
    preview.update_viewport(&key, preview.content_height() - 200.0, 200.0);

    assert!(preview.accept_page_outcome(ready_result(&request, request.plan.estimated_rgba_bytes)));
    assert_eq!(preview.ready_page_count(), 0);
}

#[test]
fn render_plan_caps_extreme_portrait_pages_by_height() {
    let plan = page_render_plan(page(100.0, 10_000.0), 1600).unwrap();

    assert!(matches!(plan.scale_axis, DocumentScaleAxis::Height(_)));
    assert!(plan.width <= MAX_DOCUMENT_PAGE_EDGE);
    assert!(plan.height <= MAX_DOCUMENT_PAGE_EDGE);
    assert!(u64::from(plan.width) * u64::from(plan.height) <= MAX_DOCUMENT_PAGE_PIXELS);
    assert!(plan.estimated_rgba_bytes <= MAX_READY_DOCUMENT_RGBA_BYTES);
}

#[test]
fn quarter_turn_page_inverts_poppler_axis_to_keep_effective_width_bucket() {
    let rotated = DocumentPageSize::from_crop_box(600.0, 400.0, 90).unwrap();
    let plan = page_render_plan(rotated, 768).unwrap();

    assert_eq!((rotated.width, rotated.height), (400.0, 600.0));
    assert_eq!((plan.width, plan.height), (768, 1152));
    assert_eq!(plan.scale_axis, DocumentScaleAxis::Height(768));
}
