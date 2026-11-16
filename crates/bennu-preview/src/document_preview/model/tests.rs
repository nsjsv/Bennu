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
        1.0,
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
        1.0,
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
    assert!(preview.update_viewport(&middle_key, 0.0, 1200.0, 300.0));
    assert_eq!(preview.wanted_pages(), &[2, 3, 4, 5]);

    let end_key = preview.viewport_key();
    assert!(preview.update_viewport(&end_key, 0.0, preview.content_height() - 300.0, 300.0));
    assert_eq!(preview.wanted_pages(), &[6, 7]);
}

#[test]
fn jumping_to_last_page_replaces_queue_without_requesting_middle_pages() {
    let pages = vec![page(600.0, 100.0); 100];
    let mut preview = preview_with_pages(pages);
    let initial = preview.drain_render_requests(2);
    assert!(initial.iter().all(|request| request.key.page_index <= 4));

    let key = preview.viewport_key();
    assert!(preview.update_viewport(&key, 0.0, preview.content_height() - 300.0, 300.0));
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
    assert!(preview.update_viewport(&scroll_key, 0.0, 1100.0, 300.0));
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

    assert!(preview.update_viewport(&old_key, 0.0, 0.0, 420.0));
    assert_eq!(preview.render_key(), old_render);
    assert_ne!(preview.viewport_key(), old_key);
    assert!(!preview.update_viewport(&old_key, 0.0, 0.0, 440.0));
}

#[test]
fn height_resize_invalidates_old_viewport_key_without_changing_render_key() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 3]);
    let old_viewport = preview.viewport_key();
    let old_render = preview.render_key();

    preview.resize(720.0, 620.0).unwrap();

    assert_eq!(preview.render_key(), old_render);
    assert!(!preview.update_viewport(&old_viewport, 0.0, 100.0, 300.0));
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
    preview.update_viewport(&key, 0.0, preview.content_height() - 200.0, 200.0);

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

#[test]
fn zoom_keeps_the_page_in_view_and_old_renders_stand_in_until_new_ones_arrive() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 10]);
    let request = preview.drain_render_requests(1).remove(0);
    assert!(preview.accept_page_outcome(ready_result(&request, request.plan.estimated_rgba_bytes)));
    let fit_width = preview.page_width();
    let old_render = preview.render_key();

    preview.set_zoom(2.0).unwrap();
    assert_eq!(preview.zoom(), 2.0);
    assert_eq!(preview.page_width(), fit_width * 2.0);
    assert!(preview.content_width() > preview.page_width());
    assert!(preview.centered_horizontal_offset() > 0.0);
    // 渲染宽度换了一档：新页还没渲染，旧的那张先拉伸着顶上。
    assert_ne!(preview.render_key(), old_render);
    assert!(matches!(
        preview.page_view(request.key.page_index),
        DocumentPageView::Ready(_)
    ));
    // 夹在范围内，回到铺满宽度。
    preview.set_zoom(100.0).unwrap();
    assert_eq!(preview.zoom(), DOCUMENT_MAX_ZOOM);
    preview.set_zoom(1.0).unwrap();
    assert_eq!(preview.page_width(), fit_width);
}

#[test]
fn page_turns_and_typed_page_numbers_land_on_page_tops() {
    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 5]);
    assert_eq!(preview.page_count(), 5);
    assert_eq!(preview.current_page(), 0);
    assert_eq!(preview.page_after_turn(DocumentPageTurn::Next), 1);
    assert_eq!(preview.page_after_turn(DocumentPageTurn::Previous), 0);
    assert_eq!(preview.page_after_turn(DocumentPageTurn::Last), 4);

    let offset = preview.scroll_to_page(2);
    assert_eq!(preview.viewport_offset(), offset);
    assert_eq!(preview.page_after_turn(DocumentPageTurn::Previous), 1);
    // 在页中间往前翻先回这页页头。
    let key = preview.viewport_key();
    assert!(preview.update_viewport(&key, 0.0, offset + 100.0, 440.0));
    assert_eq!(preview.page_after_turn(DocumentPageTurn::Previous), 2);

    preview.edit_page_number("4a".to_owned());
    assert_eq!(preview.page_number_draft(), Some("4"));
    assert_eq!(preview.take_page_number(), Some(3));
    assert_eq!(preview.page_number_draft(), None);
    preview.edit_page_number("99".to_owned());
    assert_eq!(preview.take_page_number(), Some(4));
    preview.edit_page_number("0".to_owned());
    assert_eq!(preview.take_page_number(), None);
}

fn word(text: &str, left: f32, top: f32, ends_line: bool) -> super::super::text_layer::TextWord {
    super::super::text_layer::TextWord {
        text: text.to_owned(),
        bounds: super::super::text_layer::PageRect {
            left,
            top,
            right: left + 0.1,
            bottom: top + 0.02,
        },
        ends_line,
    }
}

/// 按在空白处清掉选区；从空白处拖到字上，从离按下处最近的词选起；点在字上选这个词。
#[test]
fn pressing_on_blank_paper_clears_the_selection() {
    use super::super::text_layer::{DocumentText, PageText};
    use std::sync::Arc;

    let mut preview = preview_with_pages(vec![page(600.0, 800.0)]);
    let request = preview
        .request_text(CancellationToken::new())
        .expect("first request");
    let text = DocumentText {
        pages: vec![PageText {
            words: vec![word("alpha", 0.1, 0.1, false), word("beta", 0.3, 0.1, true)],
        }],
    };
    assert!(preview.accept_text(&request.key, Ok(Arc::new(text))));

    preview.pointer_moved(0, 0.32, 0.11);
    preview.press();
    preview.release();
    assert_eq!(preview.selected_text().as_deref(), Some("beta"));

    // 点在两个词之间的空白：选区清掉，不去选附近的词。
    preview.pointer_moved(0, 0.25, 0.11);
    preview.press();
    preview.pointer_moved(0, 0.26, 0.12);
    preview.release();
    assert_eq!(preview.selected_text(), None);

    // 从左边空白拖到第二个词上：从离按下处最近的第一个词选起。
    preview.pointer_moved(0, 0.02, 0.11);
    preview.press();
    preview.pointer_moved(0, 0.05, 0.11);
    assert!(preview.is_selecting());
    preview.pointer_moved(0, 0.35, 0.11);
    preview.release();
    assert_eq!(preview.selected_text().as_deref(), Some("alpha beta"));
}

#[test]
fn search_marks_matches_and_drag_selects_words_for_copy() {
    use super::super::text_layer::{DocumentText, PageText};
    use std::sync::Arc;

    let mut preview = preview_with_pages(vec![page(600.0, 800.0); 3]);
    let request = preview
        .request_text(CancellationToken::new())
        .expect("first request");
    assert_eq!(request.page_sizes.len(), 3);
    assert!(preview.request_text(CancellationToken::new()).is_none());

    let text = DocumentText {
        pages: vec![
            PageText {
                words: vec![word("alpha", 0.1, 0.1, false), word("beta", 0.3, 0.1, true)],
            },
            PageText::default(),
            PageText {
                words: vec![word("Beta", 0.1, 0.9, true)],
            },
        ],
    };
    preview.open_search();
    preview.set_search_query("beta".to_owned());
    // 文字还没到：没有标记，状态是「没搜完」。
    assert!(preview.page_marks(0).is_empty());
    let key = request.key.clone();
    assert!(preview.accept_text(&key, Ok(Arc::new(text))));
    assert_eq!(
        preview.search_summary().map(|summary| summary.total),
        Some(2)
    );
    let marks = preview.page_marks(0);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].0.left, 0.3);
    assert_eq!(marks[0].1, PageMarkKind::CurrentMatch);
    // 下一处在第三页靠底：滚过去。
    preview.step_search(SearchDirection::Forward);
    assert!(preview.scroll_to_current_match().is_some());
    assert_eq!(preview.page_marks(2)[0].1, PageMarkKind::CurrentMatch);

    preview.pointer_moved(0, 0.12, 0.11);
    preview.press();
    preview.pointer_moved(2, 0.15, 0.91);
    preview.release();
    assert_eq!(
        preview.selected_text().as_deref(),
        Some("alpha beta\n\nBeta")
    );
    preview.close_search();
    assert!(preview
        .page_marks(0)
        .iter()
        .all(|(_, kind)| *kind == PageMarkKind::Selection));
}
