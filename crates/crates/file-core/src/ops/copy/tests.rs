use std::io;

use tempfile::tempdir;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::ops::copy::FileOperationControls;

#[tokio::test]
async fn strong_verification_rejects_matching_size_hash_mismatch() {
    let directory = tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let target = directory.path().join("target.bin");
    let source_contents = b"same length data";
    let target_contents = b"same length DATA";

    fs::write(&source_path, source_contents).await.unwrap();
    fs::write(&target, target_contents).await.unwrap();

    let source_metadata = fs::metadata(&source_path).await.unwrap();
    let mut controls = FileOperationControls::running(CancellationToken::new());
    let mut buffer = vec![0; 1024];
    let error = verify_copied_file(
        &source_path,
        &target,
        &source_metadata,
        &mut controls,
        &mut buffer,
        Some(blake3::hash(source_contents)),
    )
    .await
    .unwrap_err();

    match error {
        FileError::Copy { from, to, source } => {
            assert_eq!(from, source_path);
            assert_eq!(to, target);
            assert_eq!(source.kind(), io::ErrorKind::InvalidData);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn basic_verification_does_not_treat_permission_metadata_as_content() {
    let directory = tempdir().unwrap();
    let source_path = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");

    fs::write(&source_path, b"same").await.unwrap();
    fs::write(&target, b"same").await.unwrap();
    let mut source_permissions = fs::metadata(&source_path).await.unwrap().permissions();
    source_permissions.set_readonly(true);
    fs::set_permissions(&source_path, source_permissions)
        .await
        .unwrap();

    let source_metadata = fs::metadata(&source_path).await.unwrap();
    let mut controls = FileOperationControls::running(CancellationToken::new());
    let mut buffer = vec![0; 1024];

    verify_copied_file(
        &source_path,
        &target,
        &source_metadata,
        &mut controls,
        &mut buffer,
        None,
    )
    .await
    .unwrap();
}
