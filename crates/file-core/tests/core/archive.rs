//! 压缩包：zip、7z 走系统 7z（没装的机器上跳过），以及与格式无关的请求校验。tar 一类见
//! `archive_tar.rs`。

use super::*;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Arc, Mutex};

use file_core::{
    inspect_archive_extraction, list_archive_members, materialize_archive_member_for_open,
    ExternalTool,
};
use tokio_util::sync::CancellationToken;

pub(super) fn seven_zip_available() -> bool {
    ExternalTool::SevenZip.locate().is_some()
}

pub(super) fn request(
    sources: Vec<PathBuf>,
    target: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
) -> ArchiveCreationRequest {
    ArchiveCreationRequest {
        sources,
        target: target.to_path_buf(),
        format,
        compression_level: ArchiveCompressionLevel::Fast,
        password: password.and_then(ArchivePassword::new),
    }
}

/// 建包并收下全部进度。
pub(super) async fn create(request: ArchiveCreationRequest) -> Vec<ArchiveProgress> {
    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    create_archive_with_progress(request, CancellationToken::new(), move |progress| {
        captured.lock().unwrap().push(progress)
    })
    .await
    .unwrap();
    let updates = updates.lock().unwrap().clone();
    updates
}

pub(super) async fn extract(request: ArchiveExtractionRequest) -> Vec<ArchiveProgress> {
    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    extract_archive_with_progress(request, CancellationToken::new(), move |progress| {
        captured.lock().unwrap().push(progress)
    })
    .await
    .unwrap();
    let updates = updates.lock().unwrap().clone();
    updates
}

/// (已处理字节, 总字节, 已完成条目, 条目总数)。外部命令都按字节报进度。
pub(super) fn byte_counts(progress: &ArchiveProgress) -> (u64, u64, usize, Option<usize>) {
    let ArchiveProgress::Bytes {
        completed_bytes,
        total_bytes,
        completed_entries,
        total_entries,
    } = progress
    else {
        panic!("expected byte progress, got {progress:?}");
    };
    (
        *completed_bytes,
        *total_bytes,
        *completed_entries,
        *total_entries,
    )
}

pub(super) fn assert_monotonic(updates: &[ArchiveProgress]) {
    let counts = updates.iter().map(byte_counts).collect::<Vec<_>>();
    assert!(!counts.is_empty());
    assert!(
        counts
            .windows(2)
            .all(|pair| pair[0].0 <= pair[1].0 && pair[0].2 <= pair[1].2),
        "{counts:?}"
    );
    assert!(counts.iter().all(|count| count.0 <= count.1), "{counts:?}");
}

fn listed_paths(members: &[file_core::ArchiveListingEntry]) -> Vec<&str> {
    let mut paths = members
        .iter()
        .map(|member| member.path.trim_end_matches('/'))
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths
}

fn write_payload(directory: &Path) -> PathBuf {
    let payload = directory.join("source");
    fs::create_dir_all(payload.join("nested")).unwrap();
    fs::write(payload.join("root.txt"), vec![b'r'; 300_000]).unwrap();
    fs::write(payload.join("nested/中文.txt"), vec![b'c'; 200_000]).unwrap();
    payload
}

#[tokio::test]
async fn zip_round_trip_lists_extracts_and_opens_members_with_seven_zip() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload = write_payload(dir.path());
    let archive = dir.path().join("bundle.zip");

    let creation = create(request(vec![payload], &archive, ArchiveFormat::Zip, None)).await;
    assert_monotonic(&creation);
    assert!(creation
        .iter()
        .all(|progress| byte_counts(progress).1 == 500_000));

    let members = list_archive_members(&archive).await.unwrap();
    assert_eq!(
        listed_paths(&members),
        vec![
            "source",
            "source/nested",
            "source/nested/中文.txt",
            "source/root.txt"
        ]
    );
    let root = members
        .iter()
        .find(|member| member.path == "source/root.txt")
        .unwrap();
    assert_eq!(root.len, 300_000);
    assert!(root.modified.is_some());

    let extraction_request = ArchiveExtractionRequest::from_archive_path(&archive, None).unwrap();
    let extraction = extract(extraction_request.clone()).await;
    assert_monotonic(&extraction);
    assert_eq!(
        fs::read(
            extraction_request
                .destination
                .join("source/nested/中文.txt")
        )
        .unwrap()
        .len(),
        200_000
    );

    let opened = materialize_archive_member_for_open(
        &archive.join("source/nested/中文.txt"),
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(opened).unwrap().len(), 200_000);
}

/// 中文 Windows 打的 zip：名字是 GBK、没标 UTF-8。列目录、整包解压、打开单个文件都要是中文名。
#[tokio::test]
async fn gbk_named_zip_shows_extracts_and_opens_chinese_names() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let archive = dir.path().join("windows.zip");
    let names = [
        "项目资料/",
        "项目资料/年度工作总结报告.txt",
        "项目资料/会议记录与安排.txt",
        "照片说明文档.txt",
    ];
    let files = names
        .iter()
        .map(|name| {
            let contents = if name.ends_with('/') {
                Vec::new()
            } else {
                format!("内容：{name}").into_bytes()
            };
            (encoding_rs::GBK.encode(name).0.into_owned(), contents)
        })
        .collect::<Vec<_>>();
    fs::write(&archive, gbk_zip_bytes(&files)).unwrap();

    let members = list_archive_members(&archive).await.unwrap();
    assert_eq!(
        listed_paths(&members),
        vec![
            "照片说明文档.txt",
            "项目资料",
            "项目资料/会议记录与安排.txt",
            "项目资料/年度工作总结报告.txt",
        ]
    );

    let destination = dir.path().join("out");
    extract(ArchiveExtractionRequest {
        archive: archive.clone(),
        destination: destination.clone(),
        password: None,
    })
    .await;
    assert_eq!(
        fs::read_to_string(destination.join("项目资料/年度工作总结报告.txt")).unwrap(),
        "内容：项目资料/年度工作总结报告.txt"
    );
    let mut top_level = fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    top_level.sort();
    assert_eq!(top_level, vec!["照片说明文档.txt", "项目资料"]);

    let opened = materialize_archive_member_for_open(
        &archive.join("项目资料/会议记录与安排.txt"),
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        fs::read_to_string(opened).unwrap(),
        "内容：项目资料/会议记录与安排.txt"
    );
}

/// 不压缩的 zip：本地文件头 + 数据，再中央目录和结尾记录。通用标志位不带 0x800，
/// 名字按原样字节写——中文 Windows 自带的压缩就是这样存的。
fn gbk_zip_bytes(files: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central = Vec::new();
    for (name, contents) in files {
        let offset = archive.len() as u32;
        let crc = crc32(contents);
        let size = contents.len() as u32;
        let mut local = 0x0403_4b50_u32.to_le_bytes().to_vec();
        local.extend_from_slice(&20_u16.to_le_bytes()); // 解压所需版本
        let mut record = 0x0201_4b50_u32.to_le_bytes().to_vec();
        record.extend_from_slice(&20_u16.to_le_bytes()); // 创建者版本（FAT）
        record.extend_from_slice(&20_u16.to_le_bytes());
        for header in [&mut local, &mut record] {
            header.extend_from_slice(&0_u16.to_le_bytes()); // 标志位
            header.extend_from_slice(&0_u16.to_le_bytes()); // 不压缩
            header.extend_from_slice(&0_u16.to_le_bytes()); // 时间
            header.extend_from_slice(&0x5a21_u16.to_le_bytes()); // 日期 2025-01-01
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0_u16.to_le_bytes()); // 扩展字段长度
        }
        record.extend_from_slice(&[0; 6]); // 注释长度、磁盘号、内部属性
        let external: u32 = if name.ends_with(b"/") { 0x10 } else { 0 };
        record.extend_from_slice(&external.to_le_bytes());
        record.extend_from_slice(&offset.to_le_bytes());
        record.extend_from_slice(name);
        local.extend_from_slice(name);
        archive.extend_from_slice(&local);
        archive.extend_from_slice(contents);
        central.extend_from_slice(&record);
    }
    let central_offset = archive.len() as u32;
    archive.extend_from_slice(&central);
    archive.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    archive.extend_from_slice(&[0; 4]);
    archive.extend_from_slice(&(files.len() as u16).to_le_bytes());
    archive.extend_from_slice(&(files.len() as u16).to_le_bytes());
    archive.extend_from_slice(&(central.len() as u32).to_le_bytes());
    archive.extend_from_slice(&central_offset.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// 带密码的 zip（AES-256）：入队前分清没给密码和密码错；对的密码能整包解、能打开单个文件。
#[tokio::test]
async fn aes_zip_password_check_extraction_and_member_open() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload = write_payload(dir.path());
    let archive = dir.path().join("locked.zip");
    create(request(
        vec![payload],
        &archive,
        ArchiveFormat::Zip,
        Some("secret"),
    ))
    .await;
    let with_password = |password: Option<&str>| ArchiveExtractionRequest {
        archive: archive.clone(),
        destination: dir.path().join("locked-out"),
        password: password.and_then(ArchivePassword::new),
    };

    assert!(matches!(
        inspect_archive_extraction(with_password(None), CancellationToken::new()).await,
        Err(FileError::ArchivePasswordRequired { .. })
    ));
    assert!(matches!(
        inspect_archive_extraction(with_password(Some("wrong")), CancellationToken::new()).await,
        Err(FileError::ArchiveInvalidPassword { .. })
    ));
    inspect_archive_extraction(with_password(Some("secret")), CancellationToken::new())
        .await
        .unwrap();
    extract(with_password(Some("secret"))).await;
    assert_eq!(
        fs::read(dir.path().join("locked-out/source/root.txt"))
            .unwrap()
            .len(),
        300_000
    );

    let member = archive.join("source/root.txt");
    assert!(matches!(
        materialize_archive_member_for_open(&member, None, CancellationToken::new()).await,
        Err(FileError::ArchivePasswordRequired { .. })
    ));
    let opened = materialize_archive_member_for_open(
        &member,
        ArchivePassword::new("secret"),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(opened).unwrap().len(), 300_000);
}

#[tokio::test]
async fn zip_extraction_into_existing_destination_succeeds() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    fs::write(&source, b"content").unwrap();
    let archive = dir.path().join("note.zip");
    create(request(vec![source], &archive, ArchiveFormat::Zip, None)).await;

    // 智能解压单根语义：目的地是已存在的目录（如压缩包所在文件夹）。
    let destination = dir.path().join("already-there");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), b"keep").unwrap();
    extract(ArchiveExtractionRequest {
        archive,
        destination: destination.clone(),
        password: None,
    })
    .await;

    assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(fs::read(destination.join("note.txt")).unwrap(), b"content");
}

#[tokio::test]
async fn failed_zip_extraction_keeps_pre_existing_destination() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload_dir = dir.path().join("payload");
    fs::create_dir(&payload_dir).unwrap();
    fs::write(payload_dir.join("inner.txt"), b"inner").unwrap();
    let archive = dir.path().join("payload.zip");
    create(request(
        vec![payload_dir],
        &archive,
        ArchiveFormat::Zip,
        None,
    ))
    .await;

    // 目的地已存在且内有同名普通文件，解压建目录必然失败；
    // 目录不是本次创建的，失败后必须原样保留。
    let destination = dir.path().join("occupied");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("payload"), b"precious").unwrap();

    let outcome = extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        CancellationToken::new(),
        |_| {},
    )
    .await;

    assert!(outcome.is_err());
    assert_eq!(fs::read(destination.join("payload")).unwrap(), b"precious");
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
        ("password-protected.zip", Some("secret")),
    ] {
        let target = dir.path().join(target_name);
        let error = create_archive_with_progress(
            request(
                vec![first_source.clone(), second_source.clone()],
                &target,
                ArchiveFormat::Zip,
                password,
            ),
            CancellationToken::new(),
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

#[tokio::test]
async fn create_archive_rejects_existing_target() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("note.txt");
    let target = dir.path().join("bundle.zip");
    fs::write(&source, b"hello").unwrap();
    fs::write(&target, b"taken").unwrap();

    let error = create_archive_with_progress(
        request(vec![source], &target, ArchiveFormat::Zip, None),
        CancellationToken::new(),
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
    assert_eq!(fs::read(&target).unwrap(), b"taken");
}

#[test]
fn archive_password_debug_is_redacted() {
    let password = ArchivePassword::new("secret").unwrap();

    assert_eq!(format!("{password:?}"), "ArchivePassword(<redacted>)");
}

/// 走 7z 的压缩和解压按 7z 报的百分比给字节进度；解压只跑一遍 `7z x`。
#[tokio::test]
async fn seven_zip_creation_and_extraction_report_byte_progress() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload = write_payload(dir.path());
    let archive = dir.path().join("plain.7z");
    let creation = create(request(
        vec![payload],
        &archive,
        ArchiveFormat::SevenZip,
        None,
    ))
    .await;
    assert!(creation
        .iter()
        .all(|progress| matches!(byte_counts(progress), (_, 500_000, _, Some(4)))));

    let destination = dir.path().join("plain-out");
    let extraction = extract(ArchiveExtractionRequest {
        archive,
        destination: destination.clone(),
        password: None,
    })
    .await;

    assert_eq!(
        fs::read(destination.join("source/nested/中文.txt"))
            .unwrap()
            .len(),
        200_000
    );
    assert_monotonic(&extraction);
    assert!(extraction
        .iter()
        .all(|progress| matches!(byte_counts(progress), (_, 500_000, _, Some(4)))));
}

/// 入队前的密码检查：只加密内容的包能列目录，再只试解一个文件；头部加密的
/// 包列目录时就会失败。两种都要在开始解之前分清“要密码”和“密码错”。
#[tokio::test]
async fn seven_zip_password_check_tells_missing_wrong_and_right_passwords_apart() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload = write_payload(dir.path());
    // 本软件带密码压 7z 时总是加密头部（-mhe=on）。
    let header_encrypted = dir.path().join("locked.7z");
    create(request(
        vec![payload.clone()],
        &header_encrypted,
        ArchiveFormat::SevenZip,
        Some("secret"),
    ))
    .await;
    // 只加密内容（头部明文）的包：目录列得出来，靠试解一个文件判断。
    let content_only = dir.path().join("content-only.7z");
    let status = std::process::Command::new(ExternalTool::SevenZip.locate().unwrap())
        .arg("a")
        .arg("-bso0")
        .arg("-bsp0")
        .arg("-psecret")
        .arg(&content_only)
        .arg(&payload)
        .status()
        .unwrap();
    assert!(status.success());

    for archive in [header_encrypted, content_only] {
        let with_password = |password: Option<&str>| ArchiveExtractionRequest {
            archive: archive.clone(),
            destination: dir.path().join("locked-out"),
            password: password.and_then(ArchivePassword::new),
        };
        assert!(matches!(
            inspect_archive_extraction(with_password(None), CancellationToken::new()).await,
            Err(FileError::ArchivePasswordRequired { .. })
        ));
        assert!(matches!(
            inspect_archive_extraction(with_password(Some("wrong")), CancellationToken::new())
                .await,
            Err(FileError::ArchiveInvalidPassword { .. })
        ));
        inspect_archive_extraction(with_password(Some("secret")), CancellationToken::new())
            .await
            .unwrap();
    }
}

/// 解压失败不删任何东西：截断的包解到一半出错，已解出的目录原样留着。
#[tokio::test]
async fn failed_seven_zip_extraction_keeps_its_output() {
    if !seven_zip_available() {
        return;
    }
    let dir = tempdir().unwrap();
    let payload = write_payload(dir.path());
    let archive = dir.path().join("broken.7z");
    create(request(
        vec![payload],
        &archive,
        ArchiveFormat::SevenZip,
        None,
    ))
    .await;
    let bytes = fs::read(&archive).unwrap();
    fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
    let destination = dir.path().join("broken-out");

    let outcome = extract_archive_with_progress(
        ArchiveExtractionRequest {
            archive,
            destination: destination.clone(),
            password: None,
        },
        CancellationToken::new(),
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
