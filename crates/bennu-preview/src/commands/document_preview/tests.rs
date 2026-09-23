use std::collections::BTreeMap;

use tempfile::tempdir;

use super::*;
use crate::document_preview::{DocumentPageView, PagedDocumentPreview};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fake_programs() -> PopplerPrograms {
    let launcher = fixture("fake-poppler");
    PopplerPrograms {
        pdfinfo: launcher.clone(),
        pdftoppm: launcher,
    }
}

fn fake_document_programs() -> DocumentPrograms {
    DocumentPrograms {
        poppler: fake_programs(),
        office: OfficePrograms::default(),
    }
}

fn request(path: PathBuf, max_file_bytes: u64) -> DocumentPrepareRequest {
    DocumentPrepareRequest {
        key: crate::document_preview::DocumentPreviewRequestKey {
            source_path: path,
            document_generation: 9,
        },
        max_file_bytes,
        cancellation: CancellationToken::new(),
    }
}

#[tokio::test]
async fn fake_pdfinfo_success_uses_two_strict_metadata_passes() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("valid-fake.pdf");
    tokio::fs::write(&source, b"fixture").await.unwrap();

    let outcome =
        prepare_document_with_programs(request(source, 1024), fake_document_programs()).await;
    let DocumentPrepareOutcome::Ready(prepared) = outcome else {
        panic!("expected prepared fake PDF");
    };

    assert_eq!(prepared.pages.len(), 2);
    assert_eq!(
        (prepared.pages[0].width, prepared.pages[0].height),
        (600.0, 800.0)
    );
    assert_eq!(
        (prepared.pages[1].width, prepared.pages[1].height),
        (600.0, 400.0)
    );
}

#[tokio::test]
async fn missing_and_nonzero_poppler_are_bounded_preview_failures() {
    let directory = tempdir().unwrap();
    let missing_source = directory.path().join("missing.pdf");
    tokio::fs::write(&missing_source, b"fixture").await.unwrap();
    let missing_programs = PopplerPrograms {
        pdfinfo: directory.path().join("does-not-exist"),
        pdftoppm: directory.path().join("does-not-exist"),
    };
    let missing = prepare_document_with_programs(
        request(missing_source, 1024),
        DocumentPrograms {
            poppler: missing_programs,
            office: OfficePrograms::default(),
        },
    )
    .await;
    let DocumentPrepareOutcome::Failed(_, missing_error) = missing else {
        panic!("expected missing Poppler failure");
    };
    assert!(missing_error.contains("requires Poppler"));

    let failure_source = directory.path().join("failure.pdf");
    tokio::fs::write(&failure_source, b"fixture").await.unwrap();
    let failed =
        prepare_document_with_programs(request(failure_source, 1024), fake_document_programs())
            .await;
    let DocumentPrepareOutcome::Failed(_, failure_error) = failed else {
        panic!("expected nonzero Poppler failure");
    };
    assert!(failure_error.starts_with("Could not inspect PDF: pdfinfo failed:"));
    assert!(failure_error.contains("fixture failure detail"));
    assert!(!failure_error.contains('\u{1}'));
}

#[test]
fn pdfinfo_process_failures_share_one_inspection_error_boundary() {
    for detail in [
        "pdfinfo timed out",
        "pdfinfo failed: fixture failure",
        "pdfinfo stdout exceeded the safety limit",
    ] {
        assert_eq!(
            pdf_inspection_error(detail.to_owned()),
            format!("Could not inspect PDF: {detail}")
        );
    }
    assert_eq!(
        pdf_inspection_error(POPPLER_INSTALL_ERROR.to_owned()),
        POPPLER_INSTALL_ERROR
    );
    assert_eq!(
        pdf_inspection_error("pdfinfo failed: Incorrect password".to_owned()),
        ENCRYPTED_PDF_ERROR
    );
}

#[tokio::test]
async fn child_runner_times_out_cancels_and_rejects_excess_output() {
    let directory = tempdir().unwrap();
    let programs = fake_programs();
    let cancellation = CancellationToken::new();
    let timeout_source = directory.path().join("timeout.pdf");
    tokio::fs::write(&timeout_source, b"fixture").await.unwrap();
    let timeout = run_poppler_command(
        &programs.pdfinfo,
        &[timeout_source.into_os_string()],
        "pdfinfo",
        Duration::from_millis(40),
        1024,
        1024,
        &cancellation,
        None,
    )
    .await
    .expect_err("timeout");
    assert!(timeout.contains("timed out"));

    let cancel_source = directory.path().join("timeout.pdf");
    let cancel = CancellationToken::new();
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel_signal.cancel();
    });
    let cancelled = run_poppler_command(
        &programs.pdfinfo,
        &[cancel_source.into_os_string()],
        "pdfinfo",
        Duration::from_secs(2),
        1024,
        1024,
        &cancel,
        None,
    )
    .await
    .unwrap();
    assert!(cancelled.is_none());

    let output_source = directory.path().join("output-limit.pdf");
    tokio::fs::write(&output_source, b"fixture").await.unwrap();
    let output_error = run_poppler_command(
        &programs.pdfinfo,
        &[output_source.into_os_string()],
        "pdfinfo",
        Duration::from_secs(2),
        32,
        32,
        &CancellationToken::new(),
        None,
    )
    .await
    .expect_err("excess output");
    assert!(output_error.contains("stdout exceeded"));
}

#[tokio::test]
async fn source_growth_is_rejected_before_poppler_starts() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("valid-fake.pdf");
    tokio::fs::write(&source, b"now larger than scan metadata")
        .await
        .unwrap();
    let marker = PathBuf::from(format!("{}.started", source.display()));

    let outcome =
        prepare_document_with_programs(request(source, 4), fake_document_programs()).await;
    let DocumentPrepareOutcome::Failed(_, error) = outcome else {
        panic!("expected size failure");
    };

    assert!(error.contains("File is too large to preview"));
    assert!(!marker.exists());
}

#[tokio::test]
async fn page_children_share_the_global_two_process_limit() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("page-block.pdf");
    tokio::fs::write(&source, b"fixture").await.unwrap();
    let workspace = Arc::new(DocumentPreviewWorkspace::create_for_pdf(source.clone()).unwrap());
    let render = crate::document_preview::DocumentRenderKey {
        request: crate::document_preview::DocumentPreviewRequestKey {
            source_path: source.clone(),
            document_generation: 3,
        },
        render_generation: 5,
        width_bucket: 512,
    };
    let document_cancellation = CancellationToken::new();
    let render_cancellation = CancellationToken::new();
    let requests = (0..3)
        .map(|page_index| DocumentPageRenderRequest {
            key: crate::document_preview::DocumentPageRequestKey {
                render: render.clone(),
                page_index,
            },
            workspace: workspace.clone(),
            plan: crate::document_preview::DocumentPageRenderPlan {
                width: 512,
                height: 683,
                estimated_rgba_bytes: 512 * 683 * 4,
                scale_axis: DocumentScaleAxis::Width(512),
            },
            document_cancellation: document_cancellation.clone(),
            render_cancellation: render_cancellation.clone(),
        })
        .collect::<Vec<_>>();
    let programs = fake_programs();

    let (first, second, third) = tokio::join!(
        render_document_page_inner(&requests[0], &programs),
        render_document_page_inner(&requests[1], &programs),
        render_document_page_inner(&requests[2], &programs),
    );
    assert!(first.is_err() && second.is_err() && third.is_err());
    let maximum = std::fs::read_to_string(format!("{}.maximum", source.display()))
        .unwrap()
        .trim()
        .parse::<usize>()
        .unwrap();
    assert_eq!(maximum, PAGE_RENDER_CONCURRENCY);
}

#[tokio::test]
async fn real_poppler_parses_rotated_crop_boxes_and_renders_only_requested_pages() {
    let source = fixture("mixed-crop-rotation.pdf");
    let outcome =
        prepare_document_with_programs(request(source, 1024 * 1024), DocumentPrograms::default())
            .await;
    let DocumentPrepareOutcome::Ready(prepared) = outcome else {
        panic!("expected real PDF preparation");
    };
    assert_eq!(prepared.pages.len(), 2);
    assert_eq!(
        (prepared.pages[0].width, prepared.pages[0].height),
        (500.0, 600.0)
    );
    assert_eq!(
        (prepared.pages[1].width, prepared.pages[1].height),
        (400.0, 600.0)
    );

    let workspace_root = prepared.workspace.root_path().to_path_buf();
    let mut preview =
        PagedDocumentPreview::new(prepared, CancellationToken::new(), 720.0, 440.0).unwrap();
    let requests = preview.drain_render_requests(2);
    assert_eq!(requests.len(), 2);
    let mut ratios = BTreeMap::new();
    for page_request in &requests {
        let rendered = render_document_page_inner(page_request, &PopplerPrograms::default())
            .await
            .unwrap()
            .expect("rendered page");
        ratios.insert(
            rendered.key.page_index,
            rendered.width as f64 / rendered.height as f64,
        );
        assert_eq!(
            (rendered.width, rendered.height),
            (page_request.plan.width, page_request.plan.height)
        );
        assert!(preview.accept_page_outcome(DocumentPageRenderOutcome::Ready(rendered)));
        assert!(matches!(
            preview.page_view(page_request.key.page_index),
            DocumentPageView::Ready(_)
        ));
    }
    assert!((ratios[&0] - 500.0 / 600.0).abs() < 0.01);
    assert!((ratios[&1] - 400.0 / 600.0).abs() < 0.01);
    assert!(std::fs::read_dir(&workspace_root).unwrap().next().is_none());

    drop(requests);
    drop(preview);
    assert!(!workspace_root.exists());
}

#[tokio::test]
async fn encrypted_real_pdf_returns_explicit_local_failure() {
    let outcome = prepare_document_with_programs(
        request(fixture("encrypted.pdf"), 1024 * 1024),
        DocumentPrograms::default(),
    )
    .await;
    let DocumentPrepareOutcome::Failed(_, error) = outcome else {
        panic!("expected encrypted PDF failure");
    };

    assert!(error.contains("Encrypted PDF preview is not supported"));
}

#[tokio::test]
async fn fake_page_failure_removes_partial_output_and_stays_page_local() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("page-fail.pdf");
    tokio::fs::write(&source, b"fixture").await.unwrap();
    let workspace = Arc::new(DocumentPreviewWorkspace::create_for_pdf(source.clone()).unwrap());
    let key = crate::document_preview::DocumentPageRequestKey {
        render: crate::document_preview::DocumentRenderKey {
            request: crate::document_preview::DocumentPreviewRequestKey {
                source_path: source,
                document_generation: 1,
            },
            render_generation: 1,
            width_bucket: 512,
        },
        page_index: 0,
    };
    let output = workspace.page_output_prefix(&key).with_extension("png");
    let render_request = DocumentPageRenderRequest {
        key,
        workspace,
        plan: crate::document_preview::DocumentPageRenderPlan {
            width: 512,
            height: 683,
            estimated_rgba_bytes: 512 * 683 * 4,
            scale_axis: DocumentScaleAxis::Width(512),
        },
        document_cancellation: CancellationToken::new(),
        render_cancellation: CancellationToken::new(),
    };

    let error = render_document_page_inner(&render_request, &fake_programs())
        .await
        .expect_err("page failure");
    assert!(error.contains("page fixture failure"));
    assert!(!output.exists());
}
