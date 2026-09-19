use super::*;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[tokio::test]
async fn create_zip_archive_preserves_selected_tree() {
    let dir = tempdir().unwrap();
    let source_dir = dir.path().join("source");
    fs::create_dir(&source_dir).unwrap();
    fs::create_dir(source_dir.join("nested")).unwrap();
    fs::write(source_dir.join("root.txt"), b"root").unwrap();
    fs::write(source_dir.join("nested/child.txt"), b"child").unwrap();
    let target = dir.path().join("bundle.zip");

    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source_dir],
            target: target.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let file = fs::File::open(target).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    assert!(archive.by_name("source/").is_ok());
    assert_eq!(archive.by_name("source/root.txt").unwrap().size(), 4);
    assert_eq!(
        archive.by_name("source/nested/child.txt").unwrap().size(),
        5
    );
}

#[tokio::test]
async fn rust_archive_creation_progress_uses_real_source_bytes() {
    let dir = tempdir().unwrap();
    let small = dir.path().join("small.bin");
    let large = dir.path().join("large.bin");
    fs::write(&small, vec![1_u8; 10]).unwrap();
    fs::write(&large, vec![2_u8; 990]).unwrap();

    for (format, target_name) in [
        (ArchiveFormat::Zip, "progress.zip"),
        (ArchiveFormat::TarGz, "progress.tar.gz"),
    ] {
        let target = dir.path().join(target_name);
        let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<
            file_core::ArchiveCreationProgress,
        >::new()));
        let captured_updates = updates.clone();

        create_archive_with_progress(
            ArchiveCreationRequest {
                sources: vec![small.clone(), large.clone()],
                target,
                format,
                compression_level: ArchiveCompressionLevel::Store,
                password: None,
            },
            tokio_util::sync::CancellationToken::new(),
            move |progress| captured_updates.lock().unwrap().push(progress),
        )
        .await
        .unwrap();

        let updates = updates.lock().unwrap();
        assert_eq!(updates.first().unwrap().completed_source_bytes, 0);
        assert_eq!(updates.first().unwrap().total_source_bytes, 1_000);
        assert_eq!(updates.first().unwrap().completed_entries, 0);
        assert_eq!(updates.last().unwrap().completed_source_bytes, 1_000);
        assert_eq!(updates.last().unwrap().total_source_bytes, 1_000);
        assert_eq!(updates.last().unwrap().completed_entries, 2);
        assert_eq!(updates.last().unwrap().total_entries, 2);
        assert!(updates.windows(2).all(|pair| {
            pair[0].completed_source_bytes <= pair[1].completed_source_bytes
                && pair[0].completed_entries <= pair[1].completed_entries
        }));
    }
}

#[tokio::test]
async fn zip_creation_rejects_source_size_changes_without_exceeding_workload() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.bin");

    for (target_name, grow_source) in [("grown.zip", true), ("shrunk.zip", false)] {
        fs::write(&source, b"seed").unwrap();
        let target = dir.path().join(target_name);
        let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<
            file_core::ArchiveCreationProgress,
        >::new()));
        let captured_updates = updates.clone();
        let source_to_change = source.clone();
        let source_was_changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let captured_source_was_changed = source_was_changed.clone();

        let outcome = create_archive_with_progress(
            ArchiveCreationRequest {
                sources: vec![source.clone()],
                target: target.clone(),
                format: ArchiveFormat::Zip,
                compression_level: ArchiveCompressionLevel::Store,
                password: None,
            },
            tokio_util::sync::CancellationToken::new(),
            move |progress| {
                captured_updates.lock().unwrap().push(progress);
                if progress.completed_source_bytes == 0
                    && !captured_source_was_changed.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    if grow_source {
                        let mut source_file = fs::OpenOptions::new()
                            .append(true)
                            .open(&source_to_change)
                            .unwrap();
                        std::io::Write::write_all(&mut source_file, b"grown").unwrap();
                    } else {
                        fs::OpenOptions::new()
                            .write(true)
                            .open(&source_to_change)
                            .unwrap()
                            .set_len(1)
                            .unwrap();
                    }
                }
            },
        )
        .await;

        assert!(matches!(outcome, Err(FileError::InvalidInput { path, .. }) if path == source));
        assert!(!target.exists());
        assert!(source_was_changed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(updates
            .lock()
            .unwrap()
            .iter()
            .all(|progress| progress.completed_source_bytes <= progress.total_source_bytes));
    }
}

#[tokio::test]
async fn archive_creation_waits_for_running_state_before_progress_and_side_effects() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let target = dir.path().join("paused.zip");
    fs::write(&source, vec![3_u8; 1_024]).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (run_state_sender, run_state) = tokio::sync::watch::channel(FileOperationRunState::Paused);
    let controls = FileOperationControls::new(cancel, run_state);
    let callback_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured_count = callback_count.clone();
    let target_for_task = target.clone();
    let archive = tokio::spawn(async move {
        create_archive_with_controls_and_progress(
            ArchiveCreationRequest {
                sources: vec![source],
                target: target_for_task,
                format: ArchiveFormat::Zip,
                compression_level: ArchiveCompressionLevel::Store,
                password: None,
            },
            controls,
            move |_| {
                captured_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            },
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!target.exists());
    assert_eq!(callback_count.load(std::sync::atomic::Ordering::Relaxed), 0);

    run_state_sender
        .send(FileOperationRunState::Running)
        .unwrap();
    archive.await.unwrap().unwrap();
    assert!(target.exists());
    assert!(callback_count.load(std::sync::atomic::Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn zip_extraction_progress_uses_uncompressed_chunk_bytes() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("payload.bin");
    let source_bytes = vec![7_u8; 2_500_000];
    fs::write(&source, &source_bytes).unwrap();
    let archive = dir.path().join("payload.zip");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: archive.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Store,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let request = ArchiveExtractionRequest {
        archive,
        destination: dir.path().join("extracted"),
        password: None,
    };
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<
        file_core::ArchiveExtractionProgress,
    >::new()));
    let captured_updates = updates.clone();

    extract_archive_with_progress(
        request.clone(),
        tokio_util::sync::CancellationToken::new(),
        move |progress| captured_updates.lock().unwrap().push(progress),
    )
    .await
    .unwrap();

    let updates = updates.lock().unwrap();
    assert_eq!(updates.first().unwrap().completed_bytes, 0);
    assert_eq!(updates.first().unwrap().total_bytes, 2_500_000);
    assert!(updates.iter().any(|progress| {
        progress.completed_bytes > 0 && progress.completed_bytes < progress.total_bytes
    }));
    assert_eq!(updates.last().unwrap().completed_bytes, 2_500_000);
    assert_eq!(updates.last().unwrap().total_bytes, 2_500_000);
    assert_eq!(updates.last().unwrap().completed_entries, 1);
    assert!(updates
        .windows(2)
        .all(|pair| pair[0].completed_bytes <= pair[1].completed_bytes));
    assert_eq!(
        fs::read(request.destination.join("payload.bin")).unwrap(),
        source_bytes
    );
}

#[tokio::test]
async fn canceled_zip_extraction_keeps_partial_progress_and_removes_destination() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("cancel.bin");
    fs::write(&source, vec![8_u8; 2_500_000]).unwrap();
    let archive = dir.path().join("cancel.zip");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: archive.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Store,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let request = ArchiveExtractionRequest {
        archive,
        destination: dir.path().join("canceled-output"),
        password: None,
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let callback_cancel = cancel.clone();
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<
        file_core::ArchiveExtractionProgress,
    >::new()));
    let captured_updates = updates.clone();

    let error = extract_archive_with_progress(request.clone(), cancel, move |progress| {
        captured_updates.lock().unwrap().push(progress);
        if progress.completed_bytes > 0 {
            callback_cancel.cancel();
        }
    })
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(!request.destination.exists());
    let updates = updates.lock().unwrap();
    let last = updates.last().unwrap();
    assert!(last.completed_bytes > 0);
    assert!(last.completed_bytes < last.total_bytes);
}

#[tokio::test]
async fn zip_extraction_into_existing_destination_succeeds() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"content").unwrap();
    let archive = dir.path().join("note.zip");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: archive.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Store,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    // 智能解压单根语义：目的地是已存在的目录（如压缩包所在文件夹）。
    let destination = dir.path().join("already-there");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), b"keep").unwrap();

    extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(fs::read(destination.join("note.txt")).unwrap(), b"content");
}

#[tokio::test]
async fn failed_zip_extraction_keeps_pre_existing_destination() {
    let dir = tempdir().unwrap();
    let payload_dir = dir.path().join("payload");
    fs::create_dir(&payload_dir).unwrap();
    fs::write(payload_dir.join("inner.txt"), b"inner").unwrap();
    let archive = dir.path().join("payload.zip");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![payload_dir],
            target: archive.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Store,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    // 目的地已存在且内有同名普通文件，解压建目录必然失败；
    // 目录不是本次创建的，失败后必须原样保留。
    let destination = dir.path().join("occupied");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("payload"), b"precious").unwrap();

    let error = extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::CreateDirectory { .. }));
    assert_eq!(fs::read(destination.join("payload")).unwrap(), b"precious");
}

#[tokio::test]
async fn tar_gz_extraction_does_not_emit_determinate_progress() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"content").unwrap();
    let archive = dir.path().join("opaque.tar.gz");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: archive.clone(),
            format: ArchiveFormat::TarGz,
            compression_level: ArchiveCompressionLevel::Fast,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let request = ArchiveExtractionRequest::from_archive_path(archive, None).unwrap();
    let callback_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured_count = callback_count.clone();

    extract_archive_with_progress(
        request,
        tokio_util::sync::CancellationToken::new(),
        move |_| {
            captured_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    )
    .await
    .unwrap();

    assert_eq!(callback_count.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn single_file_zip_round_trip_extracts_file_into_default_directory() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"hello").unwrap();
    let target = dir.path().join("note.zip");

    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: target.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let request = ArchiveExtractionRequest::from_archive_path(target, None).unwrap();
    let destination = extract_archive(request.clone(), tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(destination, request.destination);
    assert_eq!(
        fs::read(request.destination.join("note.txt")).unwrap(),
        b"hello"
    );
}

#[tokio::test]
async fn create_tar_gz_archive_preserves_selected_file() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"hello").unwrap();
    let target = dir.path().join("bundle.tar.gz");

    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: target.clone(),
            format: ArchiveFormat::TarGz,
            compression_level: ArchiveCompressionLevel::Fast,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let file = fs::File::open(target).unwrap();
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let names = archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["note.txt".to_owned()]);
}

#[cfg(unix)]
#[tokio::test]
async fn zip_rejects_non_utf8_entries_before_creating_target() {
    let dir = tempdir().unwrap();
    let first_name = OsString::from_vec(b"entry-\x80".to_vec());
    let second_name = OsString::from_vec(b"entry-\x81".to_vec());
    let first_source = dir.path().join(&first_name);
    let second_source = dir.path().join(&second_name);
    fs::write(&first_source, b"first").unwrap();
    fs::write(&second_source, b"second").unwrap();
    assert_ne!(first_name.as_bytes(), second_name.as_bytes());
    assert_eq!(first_name.to_string_lossy(), second_name.to_string_lossy());

    for (target_name, password) in [
        ("plain.zip", None),
        (
            "password-protected.zip",
            file_core::ArchivePassword::new("secret"),
        ),
    ] {
        let target = dir.path().join(target_name);
        let error = create_archive_with_progress(
            ArchiveCreationRequest {
                sources: vec![first_source.clone(), second_source.clone()],
                target: target.clone(),
                format: ArchiveFormat::Zip,
                compression_level: ArchiveCompressionLevel::Balanced,
                password,
            },
            tokio_util::sync::CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

        match error {
            FileError::InvalidInput { path, message } => {
                assert_eq!(path, first_source);
                assert!(message.contains("UTF-8"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(!target.exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn tar_gz_preserves_non_utf8_entry_name() {
    let dir = tempdir().unwrap();
    let source_name = OsString::from_vec(b"entry-\x80".to_vec());
    let source = dir.path().join(&source_name);
    fs::write(&source, b"content").unwrap();
    let target = dir.path().join("bundle.tar.gz");

    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: target.clone(),
            format: ArchiveFormat::TarGz,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let file = fs::File::open(target).unwrap();
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entry = archive.entries().unwrap().next().unwrap().unwrap();

    assert_eq!(entry.path_bytes().as_ref(), source_name.as_bytes());
}

#[tokio::test]
async fn create_archive_rejects_existing_target() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    let target = dir.path().join("bundle.zip");
    fs::write(&source, b"hello").unwrap();
    fs::write(&target, b"taken").unwrap();

    let error = create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: target.clone(),
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();

    match error {
        FileError::CreateFile { path, source } => {
            assert_eq!(path, target);
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn tar_gz_password_returns_invalid_input() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    let target = dir.path().join("bundle.tar.gz");
    fs::write(&source, b"hello").unwrap();

    let error = create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: target.clone(),
            format: ArchiveFormat::TarGz,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: file_core::ArchivePassword::new("secret"),
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, target);
            assert!(message.contains("tar.gz"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn archive_password_debug_is_redacted() {
    let password = file_core::ArchivePassword::new("secret").unwrap();

    assert_eq!(format!("{password:?}"), "ArchivePassword(<redacted>)");
}
