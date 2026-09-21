use iced::futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::*;

async fn collected_progress_updates(
    receiver: &mut iced::futures::channel::mpsc::Receiver<Message>,
    task_id: u64,
) -> Vec<FileOperationProgressUpdate> {
    let mut updates = Vec::new();
    while let Some(message) = receiver.next().await {
        if let Message::FileOperationProgressed(id, progress, _) = message {
            assert_eq!(id, task_id);
            updates.push(progress);
        }
    }
    updates
}

#[tokio::test]
async fn queued_zip_creation_emits_real_source_byte_progress() {
    let directory = tempfile::tempdir().unwrap();
    let small = directory.path().join("small.bin");
    let large = directory.path().join("large.bin");
    tokio::fs::write(&small, vec![1_u8; 10]).await.unwrap();
    tokio::fs::write(&large, vec![2_u8; 990]).await.unwrap();
    let task_id = 81;
    let (mut output, mut messages) = iced::futures::channel::mpsc::channel(32);

    let outcome = run_queued_create_archive(
        vec![small, large],
        directory.path().join("bundle.zip"),
        file_core::ArchiveFormat::Zip,
        file_core::ArchiveCompressionLevel::Store,
        None,
        FileOperationControls::running(CancellationToken::new()),
        task_id,
        &mut output,
    )
    .await;
    drop(output);

    assert!(matches!(outcome, Ok(FileOperationOutcome::NoHistory)));
    let updates = collected_progress_updates(&mut messages, task_id).await;
    assert!(matches!(
        updates.first(),
        Some(FileOperationProgressUpdate::Indeterminate)
    ));
    let byte_updates = updates
        .iter()
        .filter_map(|update| match update {
            FileOperationProgressUpdate::Bytes {
                completed_bytes,
                total_bytes,
                completed_items,
                total_items,
            } => Some((
                *completed_bytes,
                *total_bytes,
                *completed_items,
                *total_items,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!byte_updates.is_empty());
    assert_eq!(byte_updates.last(), Some(&(1_000, 1_000, 2, 2)));
    assert!(byte_updates
        .iter()
        .all(|(_, total_bytes, _, total_items)| *total_bytes == 1_000 && *total_items == 2));
}

#[tokio::test]
async fn queued_zip_extraction_emits_bytes_but_tar_gz_remains_indeterminate() {
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
        let (mut output, mut messages) = iced::futures::channel::mpsc::channel(32);

        let outcome = run_queued_extract_archive(
            request,
            FileOperationControls::running(CancellationToken::new()),
            task_id,
            &mut output,
        )
        .await;
        drop(output);

        assert!(matches!(outcome, Ok(FileOperationOutcome::NoHistory)));
        let updates = collected_progress_updates(&mut messages, task_id).await;
        assert!(matches!(
            updates.first(),
            Some(FileOperationProgressUpdate::Indeterminate)
        ));
        let byte_updates = updates
            .iter()
            .filter_map(|update| match update {
                FileOperationProgressUpdate::Bytes {
                    completed_bytes,
                    total_bytes,
                    completed_items,
                    total_items,
                } => Some((
                    *completed_bytes,
                    *total_bytes,
                    *completed_items,
                    *total_items,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        match format {
            file_core::ArchiveFormat::Zip => {
                assert_eq!(byte_updates.last(), Some(&(2_500_000, 2_500_000, 1, 1)));
            }
            file_core::ArchiveFormat::TarGz => assert!(byte_updates.is_empty()),
            file_core::ArchiveFormat::SevenZip => unreachable!(),
        }
    }
}
