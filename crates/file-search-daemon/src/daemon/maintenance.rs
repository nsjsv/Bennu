use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use tokio_util::sync::CancellationToken;

use crate::error::{SearchError, SearchResult};
use crate::logging::bounded_search_log_detail;
use crate::writer::IndexWriter;
use crate::{SearchIndexConfig, SearchIndexer};

use super::lifecycle::DaemonLifecycleSnapshot;
use super::memory::release_allocator_idle_pages;
use super::watch_ingress::{DaemonWatchIngress, DaemonWatchIngressBootstrap};
use super::work_queue::{DaemonWorkQueue, DaemonWorkRequest, WorkOutcome};

pub(super) struct ActiveMaintenance {
    work_queue: Arc<DaemonWorkQueue>,
    crawl_cancellation: CancellationToken,
    worker_join: thread::JoinHandle<SearchResult<()>>,
    watch_ingress: Option<DaemonWatchIngress>,
}

impl ActiveMaintenance {
    pub(super) fn retry_failed_roots_now(&self) {
        self.work_queue.retry_failed_roots_now();
    }
}

/// 只由 `DaemonState::start_maintenance` 调用。返回的错误表示 worker 没起来；
/// 第二项是监听没建起来，维护照常进行，只是健康度要报告它。
pub(super) fn spawn_active_maintenance(
    writer: Arc<IndexWriter>,
    config: SearchIndexConfig,
    newly_included_frontiers: Vec<PathBuf>,
    database_path: PathBuf,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    directory_snapshot_epoch: Arc<AtomicU64>,
) -> SearchResult<(ActiveMaintenance, Option<SearchError>)> {
    // 全量检查在任何线程取工作之前就排好；否则监听线程最先投进来的修补路径会抢在它前面。
    let work_queue = Arc::new(DaemonWorkQueue::starting_with_full_check(
        config.available_roots().cloned().collect(),
        newly_included_frontiers,
    ));
    let crawl_cancellation = CancellationToken::new();
    let worker_join = spawn_daemon_worker(
        writer,
        config.clone(),
        Arc::clone(&lifecycle_snapshot),
        Arc::clone(&work_queue),
        Arc::clone(&directory_snapshot_epoch),
        crawl_cancellation.clone(),
    )?;
    let (watch_ingress, watch_failure) = match DaemonWatchIngressBootstrap::establish(
        config,
        database_path,
        directory_snapshot_epoch,
    )
    .and_then(|bootstrap| bootstrap.spawn(Arc::clone(&work_queue), lifecycle_snapshot))
    {
        Ok(watch_ingress) => (Some(watch_ingress), None),
        Err(error) => (None, Some(error)),
    };
    Ok((
        ActiveMaintenance {
            work_queue,
            crawl_cancellation,
            worker_join,
            watch_ingress,
        },
        watch_failure,
    ))
}

pub(super) fn stop_active_maintenance(maintenance: ActiveMaintenance) -> SearchResult<()> {
    if let Some(watch_ingress) = maintenance.watch_ingress {
        watch_ingress.shutdown();
    }
    maintenance.crawl_cancellation.cancel();
    maintenance.work_queue.begin_shutdown();
    maintenance
        .worker_join
        .join()
        .map_err(|_| SearchError::WorkerFailed("search daemon worker thread panicked".to_owned()))?
}

fn spawn_daemon_worker(
    writer: Arc<IndexWriter>,
    config: SearchIndexConfig,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    work_queue: Arc<DaemonWorkQueue>,
    directory_snapshot_epoch: Arc<AtomicU64>,
    crawl_cancellation: CancellationToken,
) -> SearchResult<thread::JoinHandle<SearchResult<()>>> {
    thread::Builder::new()
        .name("file-search-daemon-core".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    SearchError::WorkerFailed(format!(
                        "could not create search daemon runtime: {error}"
                    ))
                })?;
            loop {
                let work_request = work_queue.wait_for_next_work();
                if matches!(work_request, DaemonWorkRequest::Shutdown) {
                    break;
                }
                let outcome = run_index_maintenance(
                    &runtime,
                    Arc::clone(&writer),
                    &config,
                    Arc::clone(&lifecycle_snapshot),
                    &directory_snapshot_epoch,
                    work_request,
                    &crawl_cancellation,
                );
                let completeness = work_queue.finish_work(&outcome);
                let mut snapshot = lifecycle_snapshot
                    .lock()
                    .expect("search daemon lifecycle mutex poisoned");
                match outcome {
                    WorkOutcome::Cancelled => snapshot.end_cancelled_work(),
                    WorkOutcome::Succeeded | WorkOutcome::Failed { .. } => {
                        snapshot.finish_work(completeness)
                    }
                }
            }
            Ok(())
        })
        .map_err(|error| {
            SearchError::WorkerFailed(format!("could not spawn search daemon worker: {error}"))
        })
}

fn run_index_maintenance(
    runtime: &tokio::runtime::Runtime,
    writer: Arc<IndexWriter>,
    config: &SearchIndexConfig,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    directory_snapshot_epoch: &AtomicU64,
    work_request: DaemonWorkRequest,
    crawl_cancellation: &CancellationToken,
) -> WorkOutcome {
    let work_label = work_request_label(&work_request);
    let high_frequency_maintenance = matches!(
        &work_request,
        DaemonWorkRequest::ChangedPaths { .. } | DaemonWorkRequest::WatchBudgetPatrol { .. }
    );
    lifecycle_snapshot
        .lock()
        .expect("search daemon lifecycle mutex poisoned")
        .begin_work();

    let maintenance_outcome = runtime.block_on(async {
        let indexer = SearchIndexer::new(Arc::clone(&writer), config.clone());
        let progress_snapshot = Arc::clone(&lifecycle_snapshot);
        let on_progress = move |progress| {
            progress_snapshot
                .lock()
                .expect("search daemon lifecycle mutex poisoned")
                .apply_progress(progress);
        };

        let stats = match work_request {
            DaemonWorkRequest::StartupCheck => {
                indexer
                    .rebuild_with_progress_cancelled(crawl_cancellation, on_progress)
                    .await?
            }
            DaemonWorkRequest::ChangedPaths { changed_paths } => {
                indexer
                    .rebuild_paths_with_progress_cancelled(
                        changed_paths,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::CoverageRepair { scopes } => {
                indexer
                    .repair_scopes_with_progress_cancelled(scopes, crawl_cancellation, on_progress)
                    .await?
            }
            DaemonWorkRequest::WatchBudgetPatrol { directories } => {
                indexer
                    .patrol_unwatched_directories_with_progress_cancelled(
                        directories,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::DirtyRootRecovery { roots } => {
                indexer
                    .recover_dirty_roots_with_progress_cancelled(
                        roots,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::Shutdown => return Ok::<_, SearchError>((Default::default(), 0)),
        };
        let total = writer.count()?;
        Ok::<_, SearchError>((stats, total))
    });
    release_allocator_idle_pages();
    match maintenance_outcome {
        Ok((stats, total)) => {
            if stats.directory_snapshots_changed > 0 {
                directory_snapshot_epoch.fetch_add(1, Ordering::AcqRel);
            }
            if high_frequency_maintenance {
                tracing::debug!(
                    target: "file_search::daemon",
                    event = "index_maintenance_completed",
                    maintenance = %work_label,
                    checked = stats.checked,
                    changed = stats.changed,
                    reindexed = stats.reindexed,
                    directories_enumerated = stats.directories_enumerated,
                    database_mutations = stats.database_mutations,
                    content_reads = stats.content_reads,
                    indexed_files = total,
                    "index maintenance completed"
                );
            } else {
                tracing::info!(
                    target: "file_search::daemon",
                    event = "index_maintenance_completed",
                    maintenance = %work_label,
                    checked = stats.checked,
                    changed = stats.changed,
                    reindexed = stats.reindexed,
                    directories_enumerated = stats.directories_enumerated,
                    database_mutations = stats.database_mutations,
                    content_reads = stats.content_reads,
                    indexed_files = total,
                    "index maintenance completed"
                );
            }
            lifecycle_snapshot
                .lock()
                .expect("search daemon lifecycle mutex poisoned")
                .record_visible_count(total);
            WorkOutcome::Succeeded
        }
        Err(_) if crawl_cancellation.is_cancelled() => WorkOutcome::Cancelled,
        Err(SearchError::Cancelled) => WorkOutcome::Cancelled,
        Err(error) => {
            let message = error.to_string();
            let log_error = bounded_search_log_detail(&message);
            tracing::error!(
                target: "file_search::daemon",
                event = "index_maintenance_failed",
                maintenance = %work_label,
                error = %log_error,
                "index maintenance failed"
            );
            WorkOutcome::Failed { message }
        }
    }
}

fn work_request_label(work_request: &DaemonWorkRequest) -> String {
    match work_request {
        DaemonWorkRequest::StartupCheck => "startup check".to_owned(),
        DaemonWorkRequest::ChangedPaths { changed_paths } => {
            format!("changed path batch ({})", changed_paths.len())
        }
        DaemonWorkRequest::CoverageRepair { scopes } => {
            format!("coverage repair ({})", scopes.len())
        }
        DaemonWorkRequest::WatchBudgetPatrol { directories } => {
            format!("watch budget patrol ({})", directories.len())
        }
        DaemonWorkRequest::DirtyRootRecovery { roots } => {
            format!("dirty root recovery ({})", roots.len())
        }
        DaemonWorkRequest::Shutdown => "shutdown".to_owned(),
    }
}
