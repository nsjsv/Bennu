use tokio_util::sync::CancellationToken;

use super::*;

async fn collected_progress_updates(
    receiver: &mut tokio::sync::mpsc::Receiver<FileOperationEvent>,
    task_id: u64,
) -> Vec<FileOperationProgressUpdate> {
    let mut updates = Vec::new();
    while let Some(event) = receiver.recv().await {
        if let FileOperationEvent::Progressed {
            task_id: id,
            update,
            ..
        } = event
        {
            assert_eq!(id, task_id);
            updates.push(update);
        }
    }
    updates
}

#[tokio::test]
async fn queued_zip_creation_counts_files_with_real_source_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let small = directory.path().join("small.bin");
    let large = directory.path().join("large.bin");
    tokio::fs::write(&small, vec![1_u8; 10]).await.unwrap();
    tokio::fs::write(&large, vec![2_u8; 990]).await.unwrap();
    let task_id = 81;
    let (output, mut messages) = tokio::sync::mpsc::channel(32);

    let outcome = run_queued_create_archive(
        vec![small, large],
        directory.path().join("bundle.zip"),
        file_core::ArchiveFormat::Zip,
        file_core::ArchiveCompressionLevel::Store,
        None,
        FileOperationControls::running(CancellationToken::new()),
        task_id,
        &output,
    )
    .await;
    drop(output);

    assert!(matches!(outcome, Ok(FileOperationOutcome::NoHistory)));
    let updates = collected_progress_updates(&mut messages, task_id).await;
    assert!(matches!(
        updates.first(),
        Some(FileOperationProgressUpdate::Indeterminate)
    ));
    let transfer_updates = transfer_counts(&updates);
    assert!(!transfer_updates.is_empty());
    assert_eq!(transfer_updates.last(), Some(&(2, 2, Some((1_000, 1_000)))));
}

/// (已完成项, 总项, 字节)：按条目计数的进度。
type TransferCounts = (usize, usize, Option<(u64, u64)>);

fn transfer_counts(updates: &[FileOperationProgressUpdate]) -> Vec<TransferCounts> {
    updates
        .iter()
        .filter_map(|update| match update {
            FileOperationProgressUpdate::Transfer {
                completed_items,
                total_items,
                bytes,
                ..
            } => Some((*completed_items, *total_items, *bytes)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn queued_zip_extraction_counts_files_and_tar_gz_reports_read_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("payload.bin");
    tokio::fs::write(&source, vec![5_u8; 2_500_000])
        .await
        .unwrap();

    for (index, format) in [
        file_core::ArchiveFormat::Zip,
        file_core::ArchiveFormat::TarGz,
    ]
    .into_iter()
    .enumerate()
    {
        let archive = directory.path().join(match format {
            file_core::ArchiveFormat::Zip => "payload.zip",
            file_core::ArchiveFormat::TarGz => "payload.tar.gz",
            file_core::ArchiveFormat::SevenZip => unreachable!(),
        });
        file_core::create_archive_with_progress(
            ArchiveCreationRequest {
                sources: vec![source.clone()],
                target: archive.clone(),
                format,
                compression_level: file_core::ArchiveCompressionLevel::Store,
                password: None,
            },
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let request = ArchiveExtractionRequest {
            archive,
            destination: directory.path().join(format!("extracted-{index}")),
            password: None,
        };
        let task_id = 90 + index as u64;
        let (output, mut messages) = tokio::sync::mpsc::channel(32);

        let outcome = run_queued_extract_archive(
            request,
            FileOperationControls::running(CancellationToken::new()),
            task_id,
            &output,
        )
        .await;
        drop(output);

        assert!(matches!(outcome, Ok(FileOperationOutcome::NoHistory)));
        let updates = collected_progress_updates(&mut messages, task_id).await;
        assert!(matches!(
            updates.first(),
            Some(FileOperationProgressUpdate::Indeterminate)
        ));
        match format {
            file_core::ArchiveFormat::Zip => {
                assert_eq!(
                    transfer_counts(&updates).last(),
                    Some(&(1, 1, Some((2_500_000, 2_500_000))))
                );
            }
            file_core::ArchiveFormat::TarGz => {
                let archive_bytes = std::fs::metadata(directory.path().join("payload.tar.gz"))
                    .unwrap()
                    .len();
                assert!(matches!(
                    updates.last(),
                    Some(FileOperationProgressUpdate::Bytes {
                        completed_bytes,
                        total_bytes,
                        completed_items: 1,
                        total_items: None,
                    }) if *completed_bytes == archive_bytes && *total_bytes == archive_bytes
                ));
            }
            file_core::ArchiveFormat::SevenZip => unreachable!(),
        }
    }
}
