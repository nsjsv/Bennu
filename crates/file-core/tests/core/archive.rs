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
        let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ArchiveProgress>::new()));
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
        let counts = updates.iter().map(entry_counts).collect::<Vec<_>>();
        assert_eq!(counts.first(), Some(&(0, 1_000, 0, 2)), "{format:?}");
        assert_eq!(counts.last(), Some(&(1_000, 1_000, 2, 2)), "{format:?}");
        assert!(counts
            .windows(2)
            .all(|pair| { pair[0].0 <= pair[1].0 && pair[0].2 <= pair[1].2 }));
        // 两种格式都要报当前文件的字节进度。
        assert!(updates.iter().any(|progress| matches!(
            progress,
            ArchiveProgress::Entries {
                current_entry: Some(entry),
                ..
            } if entry.name == "large.bin" && entry.total_bytes == 990
        )));
    }
}

/// (已处理字节, 总字节, 已完成文件数, 文件总数)：按条目计数的进度。
fn entry_counts(progress: &ArchiveProgress) -> (u64, u64, usize, usize) {
    let ArchiveProgress::Entries {
        completed_entries,
        total_entries,
        bytes: Some((completed_bytes, total_bytes)),
        ..
    } = progress
    else {
        panic!("expected entry-counted progress with bytes, got {progress:?}");
    };
    (
        *completed_bytes,
        *total_bytes,
        *completed_entries,
        *total_entries,
    )
}

#[tokio::test]
async fn zip_creation_rejects_source_size_changes_without_exceeding_workload() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.bin");

    for (target_name, grow_source) in [("grown.zip", true), ("shrunk.zip", false)] {
        fs::write(&source, b"seed").unwrap();
        let target = dir.path().join(target_name);
        let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ArchiveProgress>::new()));
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
                let completed_bytes = entry_counts(&progress).0;
                captured_updates.lock().unwrap().push(progress);
                if completed_bytes == 0
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
            .map(entry_counts)
            .all(|(completed_bytes, total_bytes, _, _)| completed_bytes <= total_bytes));
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
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ArchiveProgress>::new()));
    let captured_updates = updates.clone();

    extract_archive_with_progress(
        request.clone(),
        tokio_util::sync::CancellationToken::new(),
        move |progress| captured_updates.lock().unwrap().push(progress),
    )
    .await
    .unwrap();

    let updates = updates.lock().unwrap();
    let counts = updates.iter().map(entry_counts).collect::<Vec<_>>();
    assert_eq!(counts.first(), Some(&(0, 2_500_000, 0, 1)));
    assert!(counts
        .iter()
        .any(|(completed, total, _, _)| *completed > 0 && completed < total));
    assert_eq!(counts.last(), Some(&(2_500_000, 2_500_000, 1, 1)));
    assert!(counts.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(updates.iter().any(|progress| matches!(
        progress,
        ArchiveProgress::Entries {
            current_entry: Some(entry),
            ..
        } if entry.name == "payload.bin" && entry.total_bytes == 2_500_000
    )));
    assert_eq!(
        fs::read(request.destination.join("payload.bin")).unwrap(),
        source_bytes
    );
}

#[tokio::test]
async fn canceled_zip_extraction_keeps_what_was_already_extracted() {
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
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ArchiveProgress>::new()));
    let captured_updates = updates.clone();

    let error = extract_archive_with_progress(request.clone(), cancel, move |progress| {
        if entry_counts(&progress).0 > 0 {
            callback_cancel.cancel();
        }
        captured_updates.lock().unwrap().push(progress);
    })
    .await
    .unwrap_err();

    // 取消不删任何已解出的东西（用户决定：清理有 bug 会多删用户文件）。
    assert!(matches!(error, FileError::Cancelled));
    assert!(request.destination.join("cancel.bin").exists());
    let updates = updates.lock().unwrap();
    let (completed, total, _, _) = entry_counts(updates.last().unwrap());
    assert!(completed > 0);
    assert!(completed < total);
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
async fn tar_gz_extraction_reports_read_archive_bytes_and_entry_count() {
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
    let archive_bytes = fs::metadata(&archive).unwrap().len();
    let request = ArchiveExtractionRequest::from_archive_path(archive, None).unwrap();
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ArchiveProgress>::new()));
    let captured_updates = updates.clone();

    extract_archive_with_progress(
        request,
        tokio_util::sync::CancellationToken::new(),
        move |progress| captured_updates.lock().unwrap().push(progress),
    )
    .await
    .unwrap();

    // tar 没有目录表：按读过的压缩包字节走，条目总数未知，最后一条读到头。
    assert_eq!(
        updates.lock().unwrap().last(),
        Some(&ArchiveProgress::Bytes {
            completed_bytes: archive_bytes,
            total_bytes: archive_bytes,
            completed_entries: 1,
            total_entries: None,
        })
    );
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

fn seven_zip_available() -> bool {
    std::process::Command::new("7z")
        .arg("i")
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok()
}

async fn create_seven_zip(
    dir: &Path,
    name: &str,
    password: Option<&str>,
) -> (PathBuf, Vec<ArchiveProgress>) {
    let payload = dir.join(format!("{name}-payload"));
    fs::create_dir_all(payload.join("inner")).unwrap();
    fs::write(payload.join("a.txt"), vec![b'a'; 300_000]).unwrap();
    fs::write(payload.join("inner/b.txt"), vec![b'b'; 200_000]).unwrap();
    let archive = dir.join(format!("{name}.7z"));
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_updates = updates.clone();
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![payload],
            target: archive.clone(),
            format: ArchiveFormat::SevenZip,
            compression_level: ArchiveCompressionLevel::Fast,
            password: password.and_then(ArchivePassword::new),
        },
        tokio_util::sync::CancellationToken::new(),
        move |progress| captured_updates.lock().unwrap().push(progress),
    )
    .await
    .unwrap();
    let updates = updates.lock().unwrap().clone();
    (archive, updates)
}

/// 走 7z 的压缩和解压按 7z 报的百分比给字节进度；解压只跑一遍 `7z x`。
#[tokio::test]
async fn seven_zip_creation_and_extraction_report_byte_progress() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (archive, creation_updates) = create_seven_zip(dir.path(), "plain", None).await;
    assert!(creation_updates.iter().all(|progress| matches!(
        progress,
        ArchiveProgress::Bytes {
            total_bytes: 500_000,
            total_entries: Some(4),
            ..
        }
    )));

    let destination = dir.path().join("plain-out");
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_updates = updates.clone();
    extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        move |progress| captured_updates.lock().unwrap().push(progress),
    )
    .await
    .unwrap();

    assert_eq!(
        fs::read(destination.join("plain-payload/inner/b.txt"))
            .unwrap()
            .len(),
        200_000
    );
    let updates = updates.lock().unwrap();
    let completed = updates
        .iter()
        .map(|progress| match progress {
            ArchiveProgress::Bytes {
                completed_bytes,
                total_bytes: 500_000,
                total_entries: Some(4),
                ..
            } => *completed_bytes,
            other => panic!("unexpected 7z extraction progress {other:?}"),
        })
        .collect::<Vec<_>>();
    assert!(!completed.is_empty());
    assert!(completed.windows(2).all(|pair| pair[0] <= pair[1]));
}

/// 入队前的密码检查：只加密内容的包能列目录，再只试解一个文件；头部加密的
/// 包列目录时就会失败。两种都要在开始解之前分清“要密码”和“密码错”。
#[tokio::test]
async fn seven_zip_password_check_tells_missing_wrong_and_right_passwords_apart() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    // 本软件带密码压 7z 时总是加密头部（-mhe=on）。
    let (archive, _) = create_seven_zip(dir.path(), "locked", Some("secret")).await;
    let request = |password: Option<&str>| ArchiveExtractionRequest {
        archive: archive.clone(),
        destination: dir.path().join("locked-out"),
        password: password.and_then(ArchivePassword::new),
    };
    let inspect = |request| {
        file_core::inspect_archive_extraction(request, tokio_util::sync::CancellationToken::new())
    };

    assert!(matches!(
        inspect(request(None)).await,
        Err(FileError::ArchivePasswordRequired { .. })
    ));
    assert!(matches!(
        inspect(request(Some("wrong"))).await,
        Err(FileError::ArchiveInvalidPassword { .. })
    ));
    inspect(request(Some("secret"))).await.unwrap();

    // 只加密内容（头部明文）的包：目录列得出来，靠试解一个文件判断。
    let payload = dir.path().join("locked-payload");
    let content_only = dir.path().join("content-only.7z");
    let status = std::process::Command::new("7z")
        .arg("a")
        .arg("-bso0")
        .arg("-bsp0")
        .arg("-psecret")
        .arg(&content_only)
        .arg(&payload)
        .status()
        .unwrap();
    assert!(status.success());
    let content_request = |password: Option<&str>| ArchiveExtractionRequest {
        archive: content_only.clone(),
        destination: dir.path().join("content-out"),
        password: password.and_then(ArchivePassword::new),
    };
    assert!(matches!(
        inspect(content_request(None)).await,
        Err(FileError::ArchivePasswordRequired { .. })
    ));
    assert!(matches!(
        inspect(content_request(Some("wrong"))).await,
        Err(FileError::ArchiveInvalidPassword { .. })
    ));
    inspect(content_request(Some("secret"))).await.unwrap();
}

/// 解压失败不删任何东西：截断的包解到一半出错，已解出的目录原样留着。
#[tokio::test]
async fn failed_seven_zip_extraction_keeps_its_output() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let (archive, _) = create_seven_zip(dir.path(), "broken", None).await;
    let bytes = fs::read(&archive).unwrap();
    fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
    let destination = dir.path().join("broken-out");

    let outcome = extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await;

    // 截断后可能连目录都列不出来（直接失败，不建目的地），也可能解到一半；
    // 只要建了目的地，失败后它必须还在。
    assert!(outcome.is_err());
    if let Ok(metadata) = fs::metadata(&destination) {
        assert!(metadata.is_dir());
    }
}

/// tar 条目解到一半也要停得住：暂停在读数据的地方等，而不是等整个条目解完；
/// 停着时取消，结果是 Cancelled 而不是一条被 tar 包过的读错误。
#[tokio::test]
async fn tar_gz_extraction_pauses_and_cancels_inside_a_large_entry() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("large.bin");
    let mut payload = vec![0_u8; 16 * 1024 * 1024];
    getrandom::fill(&mut payload).unwrap();
    fs::write(&source, &payload).unwrap();
    let archive = dir.path().join("large.tar.gz");
    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source],
            target: archive.clone(),
            format: ArchiveFormat::TarGz,
            compression_level: ArchiveCompressionLevel::Store,
            password: None,
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let (run_state_sender, run_state) = tokio::sync::watch::channel(FileOperationRunState::Running);
    let cancel = tokio_util::sync::CancellationToken::new();
    let controls = FileOperationControls::new(cancel.clone(), run_state);
    let pause_sender = std::sync::Arc::new(run_state_sender);
    let pause_on_progress = pause_sender.clone();
    let destination = dir.path().join("large-out");
    let extraction = tokio::spawn(file_core::extract_archive_with_controls_and_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        controls,
        move |progress| {
            if matches!(progress, ArchiveProgress::Bytes { completed_bytes, .. } if completed_bytes > 0)
            {
                let _ = pause_on_progress.send(FileOperationRunState::Paused);
            }
        },
    ));

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!extraction.is_finished(), "paused extraction must wait");
    let written = fs::metadata(destination.join("large.bin"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    assert!(written < payload.len() as u64, "paused inside the entry");

    cancel.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), extraction)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, Err(FileError::Cancelled)), "{outcome:?}");
    drop(pause_sender);
}
