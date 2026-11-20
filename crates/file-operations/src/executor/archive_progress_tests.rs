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

/// (已完成字节, 总字节, 已完成项, 总项)：外部命令都按字节报进度。
type ByteCounts = (u64, u64, usize, Option<usize>);

fn byte_counts(updates: &[FileOperationProgressUpdate]) -> Vec<ByteCounts> {
    updates
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
        .collect()
}

#[tokio::test]
async fn queued_archive_creation_reports_source_bytes_and_entries() {
    let directory = tempfile::tempdir().unwrap();
    let small = directory.path().join("small.bin");
    let large = directory.path().join("large.bin");
    tokio::fs::write(&small, vec![1_u8; 10]).await.unwrap();
    tokio::fs::write(&large, vec![2_u8; 990]).await.unwrap();
    let task_id = 81;
    let (output, mut messages) = tokio::sync::mpsc::channel(32);

    let outcome = run_queued_create_archive(
        vec![small, large],
        directory.path().join("bundle.tar.gz"),
        file_core::ArchiveFormat::TarGz,
        file_core::ArchiveCompressionLevel::Store,
        None,
        FileOperationControls::running(CancellationToken::new()),
        task_id,
        &output,
    )
    .await;
    drop(output);

    assert_eq!(
        outcome,
        Ok(FileOperationOutcome::CreatedArchive {
            archive: directory.path().join("bundle.tar.gz"),
        })
    );
    let updates = collected_progress_updates(&mut messages, task_id).await;
    assert!(matches!(
        updates.first(),
        Some(FileOperationProgressUpdate::Indeterminate)
    ));
    assert_eq!(
        byte_counts(&updates).last(),
        Some(&(1_000, 1_000, 2, Some(2)))
    );
}

#[tokio::test]
async fn queued_archive_extraction_reports_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("payload.bin");
    tokio::fs::write(&source, vec![5_u8; 2_500_000])
        .await
        .unwrap();
    // zip 要 7-Zip，没装的机器上只测 tar.gz。
    let formats = if file_core::ExternalTool::SevenZip.locate().is_some() {
        vec![
            file_core::ArchiveFormat::TarGz,
            file_core::ArchiveFormat::Zip,
        ]
    } else {
        vec![file_core::ArchiveFormat::TarGz]
    };

    for (index, format) in formats.into_iter().enumerate() {
        let archive = directory
            .path()
            .join(format!("payload.{}", format.extension()));
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
        let archive_bytes = std::fs::metadata(&archive).unwrap().len();
        let request = ArchiveExtractionRequest::new(
            &archive,
            file_core::ArchiveExtractionPlacement::ArchiveFolder,
        )
        .unwrap();
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

        assert!(
            matches!(&outcome, Ok(FileOperationOutcome::Extracted { placed }) if placed.is_dir()),
            "{outcome:?}"
        );
        let updates = collected_progress_updates(&mut messages, task_id).await;
        assert!(matches!(
            updates.first(),
            Some(FileOperationProgressUpdate::Indeterminate)
        ));
        let last = byte_counts(&updates).last().copied();
        match format {
            // tar 按送进去的压缩包字节，条目总数未知。
            file_core::ArchiveFormat::TarGz => {
                assert_eq!(last, Some((archive_bytes, archive_bytes, 1, None)));
            }
            // 7z 按它报的百分比折算解出的字节。
            file_core::ArchiveFormat::Zip => {
                assert!(matches!(last, Some((_, 2_500_000, _, Some(1)))), "{last:?}");
            }
            file_core::ArchiveFormat::SevenZip => unreachable!(),
        }
    }
}
