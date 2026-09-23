use std::os::unix::fs::PermissionsExt;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::super::{
    prepare_document_with_programs, render_document_page_inner, DocumentPrograms, PopplerPrograms,
};
use super::*;
use crate::document_preview::{
    document_preview_format_for_path, DocumentPageRenderOutcome, DocumentPrepareOutcome,
    DocumentPreviewFormat, DocumentPreviewRequestKey, OfficeDocumentFormat, PagedDocumentPreview,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fake_office_programs() -> OfficePrograms {
    let launcher = fixture("fake-libreoffice");
    OfficePrograms {
        libreoffice: launcher.clone(),
        soffice: launcher,
    }
}

fn missing_office_programs(directory: &Path) -> OfficePrograms {
    OfficePrograms {
        libreoffice: directory.join("missing-libreoffice"),
        soffice: directory.join("missing-soffice"),
    }
}

fn office_request(path: PathBuf, max_file_bytes: u64) -> DocumentPrepareRequest {
    DocumentPrepareRequest {
        key: DocumentPreviewRequestKey {
            source_path: path,
            document_generation: 41,
        },
        max_file_bytes,
        cancellation: CancellationToken::new(),
    }
}

async fn create_source(directory: &Path, name: &str) -> PathBuf {
    let source = directory.join(name);
    tokio::fs::write(&source, b"office fixture").await.unwrap();
    source
}

async fn wait_for_path(path: &Path) {
    tokio::time::timeout(Duration::from_secs(120), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture marker");
}

fn read_process_id(path: &Path) -> libc::pid_t {
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn process_is_running(process_id: libc::pid_t) -> bool {
    let status_path = PathBuf::from(format!("/proc/{process_id}/stat"));
    if let Ok(status) = std::fs::read_to_string(status_path) {
        let state = status
            .rfind(')')
            .and_then(|name_end| status.get(name_end + 2..))
            .and_then(|remaining| remaining.chars().next());
        if matches!(state, Some('Z' | 'X')) {
            return false;
        }
    }
    let result = unsafe { libc::kill(process_id, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn test_limits(timeout: Duration) -> OfficeConversionLimits {
    OfficeConversionLimits {
        timeout,
        stdout: OFFICE_CONVERSION_OUTPUT_LIMIT,
        stderr: OFFICE_CONVERSION_OUTPUT_LIMIT,
    }
}

async fn assert_follow_up_conversion_acquires_permit(directory: &Path, name: &str) {
    let source = create_source(directory, name).await;
    let request = office_request(source, 4096);
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(120),
        convert_office_document_in_workspace(
            &request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &workspace,
            &fake_office_programs(),
            OfficeConversionLimits::default(),
        ),
    )
    .await
    .expect("Office permit was not released")
    .unwrap();
    assert!(outcome.is_some());
}

#[tokio::test]
async fn fake_conversion_uses_strict_arguments_isolated_environment_and_one_pdf() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "valid document.docx").await;
    let request = office_request(source.clone(), 1024);
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();

    let output = convert_office_document_in_workspace(
        &request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &workspace,
        &fake_office_programs(),
        OfficeConversionLimits::default(),
    )
    .await
    .unwrap()
    .expect("converted PDF");

    assert_eq!(output, workspace.output_dir().join("valid document.pdf"));
    let entries = std::fs::read_dir(workspace.output_dir())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let invocation = std::fs::read_to_string(format!("{}.invocation", source.display())).unwrap();
    let arguments = invocation
        .lines()
        .filter_map(|line| line.strip_prefix("arg="))
        .collect::<Vec<_>>();
    let profile_argument = format!("-env:UserInstallation={}", workspace.profile_url());
    assert_eq!(
        arguments,
        vec![
            "--headless",
            "--nologo",
            "--nodefault",
            "--norestore",
            profile_argument.as_str(),
            "--convert-to",
            "pdf",
            "--outdir",
            workspace.output_dir().to_str().unwrap(),
            source.to_str().unwrap(),
        ]
    );
    assert!(!invocation.contains("--nolockcheck"));
    assert!(!invocation.contains("--invisible"));
    for (name, path) in [
        ("HOME", workspace.home_dir()),
        ("TMPDIR", workspace.temporary_dir()),
        ("XDG_CONFIG_HOME", workspace.xdg_config_dir()),
        ("XDG_CACHE_HOME", workspace.xdg_cache_dir()),
        ("XDG_DATA_HOME", workspace.xdg_data_dir()),
    ] {
        assert!(invocation.contains(&format!("{name}={}", path.display())));
    }
    assert!(invocation
        .lines()
        .any(|line| line == "SAL_DISABLE_OPENCL=1"));
}

#[tokio::test]
async fn missing_libreoffice_falls_back_only_to_soffice() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "fallback.docx").await;
    let request = office_request(source.clone(), 1024);
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = OfficePrograms {
        libreoffice: directory.path().join("missing-libreoffice"),
        soffice: fixture("fake-libreoffice"),
    };

    let output = convert_office_document_in_workspace(
        &request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &workspace,
        &programs,
        OfficeConversionLimits::default(),
    )
    .await
    .unwrap();

    assert!(output.is_some());
    let invocation = std::fs::read_to_string(format!("{}.invocation", source.display())).unwrap();
    assert!(invocation
        .lines()
        .any(|line| line == "SAL_DISABLE_OPENCL=1"));

    let missing_source = create_source(directory.path(), "missing-tools.docx").await;
    let missing_request = office_request(missing_source, 1024);
    let missing_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let error = convert_office_document_in_workspace(
        &missing_request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &missing_workspace,
        &missing_office_programs(directory.path()),
        OfficeConversionLimits::default(),
    )
    .await
    .expect_err("missing LibreOffice");
    assert_eq!(error, LIBREOFFICE_INSTALL_ERROR);
}

#[tokio::test]
async fn conversion_rejects_invalid_or_oversized_output() {
    let directory = tempdir().unwrap();
    for (name, limit, expected) in [
        ("no-output.docx", 4096, "did not produce a PDF"),
        ("multiple-output.docx", 4096, "more than one output"),
        ("non-pdf-output.docx", 4096, "not a PDF file"),
        ("symlink-output.docx", 4096, "not a regular PDF file"),
        ("empty-output.docx", 4096, "empty PDF"),
        ("oversized-output.docx", 128, "too large to preview"),
        (
            "conversion-failure.docx",
            4096,
            "fixture conversion failure",
        ),
    ] {
        let source = create_source(directory.path(), name).await;
        let request = office_request(source, limit);
        let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
        let error = convert_office_document_in_workspace(
            &request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &workspace,
            &fake_office_programs(),
            OfficeConversionLimits::default(),
        )
        .await
        .expect_err(name);
        assert!(error.contains(expected), "{name}: {error}");
    }
}

#[tokio::test]
async fn output_directory_replacement_cannot_escape_the_workspace() {
    let directory = tempdir().unwrap();
    let external = tempdir().unwrap();
    let source = create_source(directory.path(), "output-escape.docx").await;
    tokio::fs::write(
        format!("{}.external", source.display()),
        external.path().as_os_str().as_encoded_bytes(),
    )
    .await
    .unwrap();
    let request = office_request(source, 4096);

    let error = prepare_office_document_workspace(
        &request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &fake_office_programs(),
    )
    .await
    .expect_err("replaced output directory");

    assert!(error.contains("output directory identity changed"));
    let escaped_pdf = external.path().join("escaped.pdf");
    assert_eq!(std::fs::read(&escaped_pdf).unwrap(), b"external-pdf");
    assert!(escaped_pdf.exists());
}

#[tokio::test]
async fn nonzero_libreoffice_does_not_fall_back_to_soffice() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "conversion-failure.docx").await;
    let request = office_request(source, 4096);
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = OfficePrograms {
        libreoffice: fixture("fake-libreoffice"),
        soffice: directory.path().join("missing-soffice"),
    };

    let error = convert_office_document_in_workspace(
        &request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &workspace,
        &programs,
        OfficeConversionLimits::default(),
    )
    .await
    .expect_err("nonzero LibreOffice");

    assert!(error.contains("fixture conversion failure"));
    assert!(!error.contains("requires LibreOffice"));
}

#[tokio::test]
async fn waiting_cancellation_and_post_wait_size_check_never_spawn_a_child() {
    let directory = tempdir().unwrap();
    let holding_dir = directory.path().join("first");
    tokio::fs::create_dir(&holding_dir).await.unwrap();
    let holding_source = create_source(&holding_dir, "holding.docx").await;
    let holding_marker = PathBuf::from(format!("{}.started", holding_source.display()));
    let holding_request = office_request(holding_source, 1024);
    let holding_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let first = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &holding_request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &holding_workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    wait_for_path(&holding_marker).await;

    let cancelled_source = create_source(directory.path(), "waiting-cancel.docx").await;
    let cancelled_marker = PathBuf::from(format!("{}.started", cancelled_source.display()));
    let cancelled_request = office_request(cancelled_source, 1024);
    let cancellation = cancelled_request.cancellation.clone();
    let cancelled_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let waiting = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &cancelled_request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &cancelled_workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    assert!(waiting.await.unwrap().unwrap().is_none());
    assert!(!cancelled_marker.exists());
    let _ = first.await.unwrap();

    let second_holding_dir = directory.path().join("second");
    tokio::fs::create_dir(&second_holding_dir).await.unwrap();
    let second_holding_source = create_source(&second_holding_dir, "holding.docx").await;
    let second_holding_marker =
        PathBuf::from(format!("{}.started", second_holding_source.display()));
    let holding_request = office_request(second_holding_source, 1024);
    let holding_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let holder = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &holding_request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &holding_workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    wait_for_path(&second_holding_marker).await;

    let growing_source = create_source(directory.path(), "growing.docx").await;
    let growing_marker = PathBuf::from(format!("{}.started", growing_source.display()));
    let growing_request = office_request(growing_source.clone(), 32);
    let growing_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let growing = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &growing_request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &growing_workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    tokio::fs::write(&growing_source, vec![0_u8; 64])
        .await
        .unwrap();
    let _ = holder.await.unwrap();
    let error = growing.await.unwrap().expect_err("post-wait size check");
    assert!(error.contains("File is too large to preview"));
    assert!(!growing_marker.exists());
}

#[tokio::test]
async fn running_cancel_timeout_and_output_limit_reap_the_child() {
    let directory = tempdir().unwrap();
    let timeout_source = create_source(directory.path(), "timeout.docx").await;
    let timeout_request = office_request(timeout_source.clone(), 1024);
    let timeout_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let timeout_error = convert_office_document_in_workspace(
        &timeout_request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &timeout_workspace,
        &fake_office_programs(),
        test_limits(Duration::from_millis(40)),
    )
    .await
    .expect_err("timeout");
    assert!(timeout_error.contains("timed out"));
    let pid = std::fs::read_to_string(format!("{}.pid", timeout_source.display()))
        .unwrap()
        .trim()
        .to_owned();
    assert!(!Path::new("/proc").join(pid).exists());

    let cancel_source = create_source(directory.path(), "running-cancel.docx").await;
    let cancel_marker = PathBuf::from(format!("{}.started", cancel_source.display()));
    let cancel_request = office_request(cancel_source.clone(), 1024);
    let cancellation = cancel_request.cancellation.clone();
    let cancel_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let running = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &cancel_request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &cancel_workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    wait_for_path(&cancel_marker).await;
    cancellation.cancel();
    assert!(running.await.unwrap().unwrap().is_none());
    let pid = std::fs::read_to_string(format!("{}.pid", cancel_source.display()))
        .unwrap()
        .trim()
        .to_owned();
    assert!(!Path::new("/proc").join(pid).exists());

    let output_source = create_source(directory.path(), "output-limit.docx").await;
    let output_request = office_request(output_source, 1024);
    let output_workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let error = convert_office_document_in_workspace(
        &output_request,
        DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
        &output_workspace,
        &fake_office_programs(),
        OfficeConversionLimits {
            timeout: Duration::from_secs(2),
            stdout: 32,
            stderr: 32,
        },
    )
    .await
    .expect_err("output limit");
    assert!(error.contains("stdout exceeded the safety limit"));
}

#[tokio::test]
async fn timeout_kills_descendants_that_hold_pipes_after_parent_exit() {
    let directory = tempdir().unwrap();
    for name in ["descendant-timeout.docx", "leader-early-exit.docx"] {
        let source = create_source(directory.path(), name).await;
        let parent_pid_path = PathBuf::from(format!("{}.pid", source.display()));
        let child_pid_path = PathBuf::from(format!("{}.pid.child", source.display()));
        let request = office_request(source, 4096);
        let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
        let programs = fake_office_programs();
        let job = tokio::spawn(async move {
            convert_office_document_in_workspace(
                &request,
                DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
                &workspace,
                &programs,
                test_limits(Duration::from_millis(80)),
            )
            .await
        });
        wait_for_path(&child_pid_path).await;
        let parent_pid = read_process_id(&parent_pid_path);
        let child_pid = read_process_id(&child_pid_path);
        let error = tokio::time::timeout(Duration::from_secs(3), job)
            .await
            .expect("descendant pipe kept conversion alive")
            .unwrap()
            .expect_err(name);
        assert!(error.contains("timed out"), "{name}: {error}");
        assert!(
            !process_is_running(parent_pid),
            "parent {parent_pid} survived"
        );
        assert!(!process_is_running(child_pid), "child {child_pid} survived");
    }
    assert_follow_up_conversion_acquires_permit(directory.path(), "after-timeout.docx").await;
}

#[tokio::test]
async fn cancellation_kills_parent_and_descendant_pipe_holder() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "descendant-cancel.docx").await;
    let parent_pid_path = PathBuf::from(format!("{}.pid", source.display()));
    let child_pid_path = PathBuf::from(format!("{}.pid.child", source.display()));
    let request = office_request(source, 4096);
    let cancellation = request.cancellation.clone();
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let programs = fake_office_programs();
    let job = tokio::spawn(async move {
        convert_office_document_in_workspace(
            &request,
            DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
            &workspace,
            &programs,
            OfficeConversionLimits::default(),
        )
        .await
    });
    wait_for_path(&child_pid_path).await;
    let parent_pid = read_process_id(&parent_pid_path);
    let child_pid = read_process_id(&child_pid_path);
    cancellation.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(3), job)
        .await
        .expect("descendant pipe kept cancelled conversion alive")
        .unwrap()
        .unwrap();
    assert!(outcome.is_none());
    assert!(
        !process_is_running(parent_pid),
        "parent {parent_pid} survived"
    );
    assert!(!process_is_running(child_pid), "child {child_pid} survived");
    assert_follow_up_conversion_acquires_permit(directory.path(), "after-cancel.docx").await;
}

#[tokio::test]
async fn office_conversions_share_one_global_permit() {
    let directory = tempdir().unwrap();
    let mut jobs = Vec::new();
    for index in 0..3 {
        let source = create_source(directory.path(), &format!("concurrency-{index}.docx")).await;
        let request = office_request(source, 1024);
        let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
        let programs = fake_office_programs();
        jobs.push(tokio::spawn(async move {
            convert_office_document_in_workspace(
                &request,
                DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx),
                &workspace,
                &programs,
                OfficeConversionLimits::default(),
            )
            .await
        }));
    }
    for job in jobs {
        assert!(job.await.unwrap().is_err());
    }

    let maximum = std::fs::read_to_string(directory.path().join("office-concurrency.maximum"))
        .unwrap()
        .trim()
        .parse::<usize>()
        .unwrap();
    assert_eq!(maximum, OFFICE_CONVERSION_CONCURRENCY);
}

#[tokio::test]
async fn office_prepare_reuses_the_pdf_metadata_pipeline() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "valid.docx").await;
    let programs = DocumentPrograms {
        poppler: {
            let launcher = fixture("fake-poppler");
            PopplerPrograms {
                pdfinfo: launcher.clone(),
                pdftoppm: launcher,
            }
        },
        office: fake_office_programs(),
    };

    let outcome = prepare_document_with_programs(office_request(source, 1024), programs).await;
    let DocumentPrepareOutcome::Ready(prepared) = outcome else {
        panic!("expected prepared Office document");
    };
    assert_eq!(prepared.pages.len(), 1);
    assert_eq!(
        (prepared.pages[0].width, prepared.pages[0].height),
        (600.0, 800.0)
    );
}

#[tokio::test]
async fn missing_libreoffice_is_reported_before_poppler_is_considered() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "missing-office.docx").await;
    let missing_poppler = directory.path().join("missing-poppler");
    let outcome = prepare_document_with_programs(
        office_request(source, 1024),
        DocumentPrograms {
            poppler: PopplerPrograms {
                pdfinfo: missing_poppler.clone(),
                pdftoppm: missing_poppler,
            },
            office: missing_office_programs(directory.path()),
        },
    )
    .await;

    let DocumentPrepareOutcome::Failed(_, error) = outcome else {
        panic!("expected missing LibreOffice failure");
    };
    assert_eq!(error, LIBREOFFICE_INSTALL_ERROR);
    assert!(!error.contains("Poppler"));
}

#[tokio::test]
async fn oversized_office_source_is_rejected_before_libreoffice_starts() {
    let directory = tempdir().unwrap();
    let source = create_source(directory.path(), "oversized-source.docx").await;
    tokio::fs::write(&source, vec![0_u8; 64]).await.unwrap();
    let marker = PathBuf::from(format!("{}.started", source.display()));
    let programs = DocumentPrograms {
        poppler: PopplerPrograms::default(),
        office: fake_office_programs(),
    };

    let outcome = prepare_document_with_programs(office_request(source, 32), programs).await;
    let DocumentPrepareOutcome::Failed(_, error) = outcome else {
        panic!("expected source budget failure");
    };
    assert!(error.contains("File is too large to preview"));
    assert!(!marker.exists());
}

#[test]
fn real_office_samples_keep_native_container_signatures() {
    for name in [
        "office/preview.doc",
        "office/preview.xls",
        "office/preview.ppt",
    ] {
        assert!(std::fs::read(fixture(name))
            .unwrap()
            .starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]));
    }
    for name in [
        "office/preview.docx",
        "office/preview.xlsx",
        "office/preview.pptx",
        "office/preview.odt",
        "office/preview.ods",
        "office/preview.odp",
    ] {
        assert!(std::fs::read(fixture(name)).unwrap().starts_with(b"PK"));
    }
}

fn command_is_available(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            std::fs::metadata(directory.join(program)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
    })
}

#[tokio::test]
#[ignore = "requires LibreOffice and Poppler"]
async fn real_office_samples_convert_and_render_the_first_page() {
    for program in ["libreoffice", "pdfinfo", "pdftoppm", "pdftotext"] {
        assert!(
            command_is_available(program),
            "real Office preview test requires `{program}` in PATH"
        );
    }

    // 这些单页样本由 LibreOffice 26.2.5.2 从最小 Flat ODF 源生成，保留真实二进制容器和代表文字、表格、幻灯片内容。
    for name in [
        "office/preview.doc",
        "office/preview.docx",
        "office/preview.xls",
        "office/preview.xlsx",
        "office/preview.ppt",
        "office/preview.pptx",
        "office/preview.odt",
        "office/preview.ods",
        "office/preview.odp",
    ] {
        let source = fixture(name);
        let mut request = office_request(source.clone(), 4 * 1024 * 1024);
        let started_at = std::time::Instant::now();
        let outcome = prepare_document_with_programs(request, DocumentPrograms::default()).await;
        let DocumentPrepareOutcome::Ready(prepared) = outcome else {
            panic!("could not prepare {}: {outcome:?}", source.display());
        };
        assert_eq!(prepared.pages.len(), 1, "{}", source.display());
        let text_output = tokio::process::Command::new("pdftotext")
            .arg(prepared.workspace.pdf_path())
            .arg("-")
            .output()
            .await
            .unwrap();
        assert!(
            text_output.status.success(),
            "{}: {}",
            source.display(),
            String::from_utf8_lossy(&text_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&text_output.stdout).contains("File Manager"),
            "{} lost its content marker",
            source.display()
        );

        let mut preview =
            PagedDocumentPreview::new(prepared, CancellationToken::new(), 720.0, 440.0).unwrap();
        let page_request = preview
            .drain_render_requests(1)
            .pop()
            .expect("first page request");
        let rendered = render_document_page_inner(&page_request, &PopplerPrograms::default())
            .await
            .unwrap()
            .expect("rendered first page");
        assert!(rendered.width > 0 && rendered.height > 0);
        assert!(preview.accept_page_outcome(DocumentPageRenderOutcome::Ready(rendered)));
        let elapsed = started_at.elapsed();
        println!("{}: {elapsed:?}", source.display());
        assert!(
            elapsed < Duration::from_secs(2),
            "{} took {elapsed:?} to prepare and render the first page",
            source.display()
        );
    }
}
