//! tar 一类压缩包：走系统 tar（tar.gz 新建再接 gzip）。各发行版都有 tar 和 gzip；
//! xz / zstd / bzip2 没装的格式跳过。

use super::archive::{
    assert_monotonic, byte_counts, create, extract, into_archive_folder, request, smart,
};
use super::*;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use file_core::{list_archive_members, materialize_archive_member_for_open};
use tokio_util::sync::CancellationToken;

fn command_available(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
    })
}

#[tokio::test]
async fn tar_gz_creation_progress_follows_the_stream_and_round_trips() {
    let dir = tempdir().unwrap();
    let small = dir.path().join("small.bin");
    let large = dir.path().join("large.bin");
    fs::write(&small, vec![1_u8; 10]).unwrap();
    fs::write(&large, vec![2_u8; 990]).unwrap();
    let archive = dir.path().join("progress.tar.gz");

    let updates = create(request(
        vec![small, large],
        &archive,
        ArchiveFormat::TarGz,
        None,
    ))
    .await;

    assert_monotonic(&updates);
    assert_eq!(
        updates.first().map(byte_counts),
        Some((0, 1_000, 0, Some(2)))
    );
    assert_eq!(
        updates.last().map(byte_counts),
        Some((1_000, 1_000, 2, Some(2)))
    );
    let members = list_archive_members(&archive).await.unwrap();
    let mut names = members
        .iter()
        .map(|member| (member.path.as_str(), member.len))
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, vec![("large.bin", 990), ("small.bin", 10)]);
    assert!(members.iter().all(|member| member.modified.is_some()));

    let extraction = extract(into_archive_folder(&archive)).await;
    assert_eq!(extraction.placed, dir.path().join("progress"));
    assert_eq!(
        fs::read(extraction.placed.join("large.bin")).unwrap(),
        vec![2_u8; 990]
    );
}

/// tar 也一样：唯一的顶层文件夹在压缩包旁边还没有，就直接放过去；已经有了就放进新文件夹，
/// 原有的同名文件不动。
#[tokio::test]
async fn smart_tar_extraction_never_writes_into_an_existing_folder() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let photos = source.join("photos");
    fs::create_dir_all(&photos).unwrap();
    fs::write(photos.join("a.jpg"), b"from the archive").unwrap();
    let archive = dir.path().join("photos.tar.gz");
    create(request(vec![photos], &archive, ArchiveFormat::TarGz, None)).await;

    let beside = extract(smart(&archive)).await;
    assert_eq!(beside.placed, dir.path().join("photos"));
    fs::write(beside.placed.join("a.jpg"), b"mine").unwrap();

    let wrapped = extract(smart(&archive)).await;
    assert_eq!(wrapped.placed, dir.path().join("photos 2"));
    assert_eq!(fs::read(dir.path().join("photos/a.jpg")).unwrap(), b"mine");
    assert_eq!(
        fs::read(wrapped.placed.join("photos/a.jpg")).unwrap(),
        b"from the archive"
    );
}

/// tar 没有目录表：按送进 tar 的压缩包字节走，条目总数未知，最后一条读到头。
#[tokio::test]
async fn tar_gz_extraction_reports_read_archive_bytes_and_entry_count() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"content").unwrap();
    let archive = dir.path().join("opaque.tar.gz");
    create(request(vec![source], &archive, ArchiveFormat::TarGz, None)).await;
    let archive_bytes = fs::metadata(&archive).unwrap().len();

    let updates = extract(into_archive_folder(&archive)).await.progress;

    assert_monotonic(&updates);
    assert_eq!(
        updates.last(),
        Some(&ArchiveProgress::Bytes {
            completed_bytes: archive_bytes,
            total_bytes: archive_bytes,
            completed_entries: 1,
            total_entries: None,
        })
    );
}

/// 各种压缩的 tar 都能列目录、整包解压（从管道读，要按文件头告诉 tar 压缩方式）、打开单个文件。
#[tokio::test]
async fn compressed_tars_list_extract_and_open_members() {
    let dir = tempdir().unwrap();
    let payload = dir.path().join("payload");
    fs::create_dir_all(payload.join("文件夹")).unwrap();
    fs::write(payload.join("文件夹/说明.txt"), b"inside").unwrap();
    fs::write(payload.join("top.txt"), b"top").unwrap();

    for (name, option, compressor) in [
        ("plain.tar", None, None),
        ("bundle.tar.gz", Some("--gzip"), Some("gzip")),
        ("bundle.tgz", Some("--gzip"), Some("gzip")),
        ("bundle.tar.xz", Some("--xz"), Some("xz")),
        ("bundle.tar.zst", Some("--zstd"), Some("zstd")),
        ("bundle.tbz2", Some("--bzip2"), Some("bzip2")),
    ] {
        if compressor.is_some_and(|compressor| !command_available(compressor)) {
            continue;
        }
        let archive = dir.path().join(name);
        let mut command = std::process::Command::new("tar");
        command.arg("-c").args(option).arg("-f").arg(&archive);
        let status = command
            .arg("-C")
            .arg(&payload)
            .arg("--")
            .arg("文件夹")
            .arg("top.txt")
            .status()
            .unwrap();
        assert!(status.success(), "{name}");

        let members = list_archive_members(&archive).await.unwrap();
        let mut paths = members
            .iter()
            .map(|member| member.path.trim_end_matches('/'))
            .collect::<Vec<_>>();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec!["top.txt", "文件夹", "文件夹/说明.txt"],
            "{name}"
        );

        let destination = extract(into_archive_folder(&archive)).await.placed;
        assert_eq!(
            fs::read(destination.join("文件夹/说明.txt")).unwrap(),
            b"inside"
        );

        let opened = materialize_archive_member_for_open(
            &archive.join("文件夹/说明.txt"),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(opened).unwrap(), b"inside", "{name}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn tar_gz_keeps_non_utf8_entry_names_byte_for_byte() {
    let dir = tempdir().unwrap();
    let source_name = OsString::from_vec(b"entry-\x80".to_vec());
    let source = dir.path().join(&source_name);
    fs::write(&source, b"content").unwrap();
    let archive = dir.path().join("bundle.tar.gz");
    create(request(vec![source], &archive, ArchiveFormat::TarGz, None)).await;

    let destination = extract(into_archive_folder(&archive)).await.placed;

    let names = fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].as_bytes(), source_name.as_bytes());
}

#[tokio::test]
async fn tar_gz_password_returns_invalid_input() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    let target = dir.path().join("bundle.tar.gz");
    fs::write(&source, b"hello").unwrap();

    let error = create_archive_with_progress(
        request(vec![source], &target, ArchiveFormat::TarGz, Some("secret")),
        CancellationToken::new(),
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
    assert!(!target.exists());
}

#[tokio::test]
async fn archive_creation_waits_for_running_state_before_progress_and_side_effects() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let target = dir.path().join("paused.tar.gz");
    fs::write(&source, vec![3_u8; 1_024]).unwrap();
    let (run_state_sender, run_state) = tokio::sync::watch::channel(FileOperationRunState::Paused);
    let controls = FileOperationControls::new(CancellationToken::new(), run_state);
    let callback_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured_count = callback_count.clone();
    let creation = tokio::spawn(create_archive_with_controls_and_progress(
        request(vec![source], &target, ArchiveFormat::TarGz, None),
        controls,
        move |_| {
            captured_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    ));

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!target.exists());
    assert_eq!(callback_count.load(std::sync::atomic::Ordering::Relaxed), 0);

    run_state_sender
        .send(FileOperationRunState::Running)
        .unwrap();
    creation.await.unwrap().unwrap();
    assert!(target.exists());
    assert!(callback_count.load(std::sync::atomic::Ordering::Relaxed) > 0);
}

/// 压到一半暂停：不再往 gzip 送数据，目标文件不再长；停着时取消，删掉半成品。
#[tokio::test]
async fn tar_gz_creation_pauses_and_cancel_removes_the_half_made_archive() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("large.bin");
    let mut payload = vec![0_u8; 32 * 1024 * 1024];
    getrandom::fill(&mut payload).unwrap();
    fs::write(&source, &payload).unwrap();
    let target = dir.path().join("large.tar.gz");
    let (run_state_sender, run_state) = tokio::sync::watch::channel(FileOperationRunState::Running);
    let cancel = CancellationToken::new();
    let controls = FileOperationControls::new(cancel.clone(), run_state);
    let pause_sender = std::sync::Arc::new(run_state_sender);
    let pause_on_progress = pause_sender.clone();
    let creation = tokio::spawn(create_archive_with_controls_and_progress(
        request(vec![source], &target, ArchiveFormat::TarGz, None),
        controls,
        move |progress| {
            if byte_counts(&progress).0 > 0 {
                let _ = pause_on_progress.send(FileOperationRunState::Paused);
            }
        },
    ));

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!creation.is_finished(), "paused creation must wait");
    let size_while_paused = fs::metadata(&target).unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(fs::metadata(&target).unwrap().len(), size_while_paused);

    cancel.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), creation)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, Err(FileError::Cancelled)), "{outcome:?}");
    assert!(!target.exists());
    drop(pause_sender);
}

/// 解到大条目中间也要停得住：暂停就停送，tar 读不到数据停在原地；停着时取消，结果是
/// Cancelled，已解出的部分留着。
#[tokio::test]
async fn tar_gz_extraction_pauses_and_cancels_inside_a_large_entry() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("large.bin");
    let mut payload = vec![0_u8; 16 * 1024 * 1024];
    getrandom::fill(&mut payload).unwrap();
    fs::write(&source, &payload).unwrap();
    let archive = dir.path().join("large.tar.gz");
    create(request(vec![source], &archive, ArchiveFormat::TarGz, None)).await;

    let (run_state_sender, run_state) = tokio::sync::watch::channel(FileOperationRunState::Running);
    let cancel = CancellationToken::new();
    let controls = FileOperationControls::new(cancel.clone(), run_state);
    let pause_sender = std::sync::Arc::new(run_state_sender);
    let pause_on_progress = pause_sender.clone();
    let destination = dir.path().join("large");
    let extraction = tokio::spawn(file_core::extract_archive_with_controls_and_progress(
        into_archive_folder(&archive),
        controls,
        move |progress| {
            if byte_counts(&progress).0 > 0 {
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
    assert!(destination.exists());
    drop(pause_sender);
}
