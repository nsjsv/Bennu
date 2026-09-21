use file_core::{
    discover_directory_with_progress, DirectoryDiscovery, DirectoryDiscoveryBatch, FileError,
    ScanOptions,
};
use iced::futures::SinkExt;
use iced::Task;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::model::{
    DirectoryLoadFailure, DirectoryLoadRequest, ExpandedDirectoryLoadRequest, Message,
};
use crate::startup_trace;

/// 目录加载 UI 提示节流：对齐 60Hz 帧步进，提示密度与渲染节奏一致。
const DIRECTORY_HINT_MIN_INTERVAL: Duration = crate::ui_pacing::FRAME_INTERVAL_60HZ;
const DIRECTORY_LOAD_CHANNEL_SIZE: usize = 8;

#[derive(Debug, Default)]
struct DirectoryHintThrottle {
    last_emitted_at: Option<Instant>,
}

impl DirectoryHintThrottle {
    fn should_emit(&mut self, now: Instant) -> bool {
        let should_emit = self.last_emitted_at.is_none_or(|last_emitted_at| {
            now.saturating_duration_since(last_emitted_at) >= DIRECTORY_HINT_MIN_INTERVAL
        });
        if should_emit {
            self.last_emitted_at = Some(now);
        }
        should_emit
    }
}

pub(crate) fn load_directory_command(
    request: DirectoryLoadRequest,
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        DIRECTORY_LOAD_CHANNEL_SIZE,
        async move |mut output| {
            let collected = collect_directory_with_hints(
                request.path.clone(),
                options,
                cancellation,
                |batch| {
                    output
                        .try_send(Message::DirectoryDiscoveryBatch(request.clone(), batch))
                        .is_ok()
                },
            )
            .await;
            let authoritative = collected.map(|collected| {
                startup_trace::record_directory_collection_hint_counts(
                    collected.produced_hint_count,
                    collected.accepted_hint_count,
                    collected.dropped_hint_count,
                );
                crate::model::PrebuiltDirectoryDiscovery::build(collected.discovery)
            });
            let _ = output
                .send(Message::DirectoryEntriesReady(request, authoritative))
                .await;
        },
    ))
}

pub(crate) fn load_expanded_directory_command(
    request: ExpandedDirectoryLoadRequest,
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        DIRECTORY_LOAD_CHANNEL_SIZE,
        async move |mut output| {
            let collected = collect_directory_with_hints(
                request.path.clone(),
                options,
                cancellation,
                |batch| {
                    output
                        .try_send(Message::ExpandedDirectoryDiscoveryBatch(
                            request.clone(),
                            batch,
                        ))
                        .is_ok()
                },
            )
            .await;
            let authoritative = collected.map(|collected| {
                startup_trace::record_directory_collection_hint_counts(
                    collected.produced_hint_count,
                    collected.accepted_hint_count,
                    collected.dropped_hint_count,
                );
                crate::model::PrebuiltDirectoryDiscovery::build(collected.discovery)
            });
            let _ = output
                .send(Message::ExpandedDirectoryEntriesReady(
                    request,
                    authoritative,
                ))
                .await;
        },
    ))
}

struct CollectedDirectory {
    discovery: DirectoryDiscovery,
    produced_hint_count: usize,
    accepted_hint_count: usize,
    dropped_hint_count: usize,
}

async fn collect_directory_with_hints(
    path: PathBuf,
    options: ScanOptions,
    cancellation: CancellationToken,
    mut emit_hint: impl FnMut(DirectoryDiscoveryBatch) -> bool,
) -> Result<CollectedDirectory, DirectoryLoadFailure> {
    startup_trace::mark_once("initial_directory_collection_started");
    let mut produced_hint_count = 0;
    let mut accepted_hint_count = 0;
    let mut hint_throttle = DirectoryHintThrottle::default();
    let discovery = discover_directory_with_progress(path, options, cancellation, |batch| {
        produced_hint_count += 1;
        if hint_throttle.should_emit(Instant::now()) && emit_hint(batch) {
            accepted_hint_count += 1;
        }
    })
    .await
    .map_err(classify_directory_load_failure)?;
    startup_trace::mark_once("initial_directory_collection_finished");
    Ok(CollectedDirectory {
        discovery,
        produced_hint_count,
        accepted_hint_count,
        dropped_hint_count: produced_hint_count - accepted_hint_count,
    })
}

fn classify_directory_load_failure(file_error: FileError) -> DirectoryLoadFailure {
    let directory_is_unavailable = matches!(
        &file_error,
        FileError::ReadDirectory { source, .. } | FileError::ReadEntry { source, .. }
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            )
    );
    let message = file_error.to_string();

    if directory_is_unavailable {
        DirectoryLoadFailure::DirectoryUnavailable { message }
    } else {
        DirectoryLoadFailure::ReadFailed { message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropped_hints_do_not_change_authoritative_collection() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..300 {
            std::fs::write(directory.path().join(format!("file-{index:03}.dat")), []).unwrap();
        }

        let collected = collect_directory_with_hints(
            directory.path().to_path_buf(),
            ScanOptions::default(),
            CancellationToken::new(),
            |_| false,
        )
        .await
        .unwrap();

        assert_eq!(collected.discovery.entries.len(), 300);
        assert_eq!(collected.produced_hint_count, 3);
        assert_eq!(collected.accepted_hint_count, 0);
        assert_eq!(collected.dropped_hint_count, 3);
    }

    #[test]
    fn missing_directory_is_classified_as_unavailable() {
        let missing_path = PathBuf::from("/workspace/removed");
        let failure = classify_directory_load_failure(FileError::ReadDirectory {
            path: missing_path,
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });

        assert!(matches!(
            failure,
            DirectoryLoadFailure::DirectoryUnavailable { .. }
        ));
    }

    #[test]
    fn permission_denial_remains_a_visible_read_failure() {
        let locked_path = PathBuf::from("/workspace/locked");
        let failure = classify_directory_load_failure(FileError::ReadDirectory {
            path: locked_path,
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        });

        assert!(matches!(failure, DirectoryLoadFailure::ReadFailed { .. }));
    }
    #[test]
    fn hint_throttle_emits_immediately_and_after_interval() {
        let start = Instant::now();
        let mut throttle = DirectoryHintThrottle::default();

        assert!(throttle.should_emit(start));
        assert!(
            !throttle.should_emit(start + DIRECTORY_HINT_MIN_INTERVAL - Duration::from_millis(1))
        );
        assert!(throttle.should_emit(start + DIRECTORY_HINT_MIN_INTERVAL));
    }

    #[test]
    fn hint_throttle_does_not_emit_before_the_previous_hint() {
        let start = Instant::now();
        let mut throttle = DirectoryHintThrottle::default();

        assert!(throttle.should_emit(start));
        assert!(!throttle.should_emit(start - DIRECTORY_HINT_MIN_INTERVAL));
    }
}
