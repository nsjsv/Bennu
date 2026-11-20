//! 没装 7-Zip 的机器：zip / 7z / rar 报 `ToolMissing`（文字里带安装办法），tar 一类照常可用。
//! 要改整个进程的 PATH，所以单独一个测试程序、只有一个测试。

use std::path::{Path, PathBuf};

use file_core::{
    create_archive_with_progress, extract_archive, list_archive_members, ArchiveCompressionLevel,
    ArchiveCreationRequest, ArchiveExtractionPlacement, ArchiveExtractionRequest, ArchiveFormat,
    ExternalTool, FileError,
};
use tokio_util::sync::CancellationToken;

fn creation(sources: Vec<PathBuf>, target: &Path, format: ArchiveFormat) -> ArchiveCreationRequest {
    ArchiveCreationRequest {
        sources,
        target: target.to_path_buf(),
        format,
        compression_level: ArchiveCompressionLevel::Fast,
        password: None,
    }
}

fn assert_seven_zip_missing(outcome: Result<impl std::fmt::Debug, FileError>) {
    match outcome {
        Err(error @ FileError::ToolMissing(ExternalTool::SevenZip)) => {
            assert!(error.to_string().contains("install it with"), "{error}");
        }
        other => panic!("expected 7-Zip to be missing, got {other:?}"),
    }
}

#[tokio::test]
async fn zip_needs_seven_zip_while_tar_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    // 只放 tar 和 gzip 进 PATH。
    let tools = dir.path().join("bin");
    std::fs::create_dir(&tools).unwrap();
    let search_path = std::env::var_os("PATH").unwrap();
    for tool in ["tar", "gzip"] {
        let real = std::env::split_paths(&search_path)
            .map(|directory| directory.join(tool))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| panic!("{tool} is installed"));
        std::os::unix::fs::symlink(real, tools.join(tool)).unwrap();
    }
    // 这个测试程序只有这一个测试，没有别的线程在读环境变量。
    std::env::set_var("PATH", &tools);
    assert!(ExternalTool::SevenZip.locate().is_none());

    let source = dir.path().join("note.txt");
    std::fs::write(&source, b"hello").unwrap();
    let zip = dir.path().join("note.zip");
    assert_seven_zip_missing(
        create_archive_with_progress(
            creation(vec![source.clone()], &zip, ArchiveFormat::Zip),
            CancellationToken::new(),
            |_| {},
        )
        .await,
    );
    assert!(!zip.exists());

    std::fs::write(&zip, b"PK\x05\x06").unwrap();
    assert_seven_zip_missing(list_archive_members(&zip).await);
    assert_seven_zip_missing(
        extract_archive(
            ArchiveExtractionRequest::new(&zip, ArchiveExtractionPlacement::Smart).unwrap(),
            CancellationToken::new(),
        )
        .await,
    );
    assert!(!dir.path().join("note").exists());

    let tar_gz = dir.path().join("note.tar.gz");
    create_archive_with_progress(
        creation(vec![source], &tar_gz, ArchiveFormat::TarGz),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(list_archive_members(&tar_gz).await.unwrap().len(), 1);
    let extracted = extract_archive(
        ArchiveExtractionRequest::new(&tar_gz, ArchiveExtractionPlacement::Smart).unwrap(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(extracted.placed.join("note.txt")).unwrap(),
        b"hello"
    );
}
