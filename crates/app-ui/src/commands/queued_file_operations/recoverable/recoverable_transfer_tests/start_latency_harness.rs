use super::*;

use crate::model::Message;
use crate::operation_queue::{
    execute_file_operation_persistence, FileOperationEnqueueOutcome, FileOperationQueue,
    QueuedFileOperation, QueuedTransfer,
};
use file_operation_store::TaskQueueStore;
use iced::futures::channel::mpsc;
use iced::futures::StreamExt;
use rusqlite::Connection;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CORPUS_SIZE: usize = 256 * 1024 * 1024;
const WRITER_LOCK: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, PartialEq, Eq)]
enum HarnessOperation {
    Copy,
    Move,
}

impl HarnessOperation {
    fn queued_mode(self) -> QueuedTransferMode {
        match self {
            Self::Copy => QueuedTransferMode::Copy,
            Self::Move => QueuedTransferMode::Move,
        }
    }

    fn queued_operation(
        self,
        transfer: QueuedTransfer,
        verification: FileOperationVerification,
    ) -> QueuedFileOperation {
        match self {
            Self::Copy => QueuedFileOperation::Copy {
                transfers: vec![transfer],
                verification,
            },
            Self::Move => QueuedFileOperation::Move {
                transfers: vec![transfer],
                verification,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct HarnessVerification {
    value: FileOperationVerification,
    name: &'static str,
}

const VERIFICATIONS: [HarnessVerification; 2] = [
    HarnessVerification {
        value: FileOperationVerification::BasicMetadata,
        name: "basic",
    },
    HarnessVerification {
        value: FileOperationVerification::Strong,
        name: "strong",
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-only startup latency harness; run explicitly with --ignored --nocapture"]
async fn file_operation_start_latency_release_harness() {
    let corpus_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-operation-start-latency-corpus")
        .join(std::process::id().to_string());
    fs::create_dir_all(&corpus_root).unwrap();
    assert_disk_backed(&corpus_root);

    for operation in [HarnessOperation::Copy, HarnessOperation::Move] {
        for verification in VERIFICATIONS {
            run_scenario(&corpus_root, operation, verification).await;
        }
    }

    fs::remove_dir_all(corpus_root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-only target visibility harness; run explicitly with --ignored --nocapture"]
async fn basic_same_filesystem_move_target_visibility_release_harness() {
    let corpus_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-operation-start-latency-corpus")
        .join(format!("target-visibility-{}", std::process::id()));
    fs::create_dir_all(&corpus_root).unwrap();
    assert_disk_backed(&corpus_root);

    run_scenario(&corpus_root, HarnessOperation::Move, VERIFICATIONS[0]).await;

    fs::remove_dir_all(corpus_root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-only batch target visibility harness; run explicitly with --ignored --nocapture"]
async fn basic_same_filesystem_move_batch_visibility_release_harness() {
    let corpus_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-operation-start-latency-corpus")
        .join(format!("batch-target-visibility-{}", std::process::id()));
    fs::create_dir_all(&corpus_root).unwrap();
    assert_disk_backed(&corpus_root);

    run_move_batch_visibility_scenario(&corpus_root).await;

    fs::remove_dir_all(corpus_root).unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "moves the three JPEG files from FILE_MANAGER_MOVE_TRACE_ROOT into its test1 directory"]
async fn real_directory_move_batch_comparison_harness() {
    let root =
        PathBuf::from(std::env::var_os("FILE_MANAGER_MOVE_TRACE_ROOT").expect(
            "set FILE_MANAGER_MOVE_TRACE_ROOT to the explicitly approved fixture directory",
        ));
    let target_directory = std::env::var_os("FILE_MANAGER_MOVE_TRACE_TARGET")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("test1"));
    assert!(
        root.is_absolute(),
        "comparison fixture root must be absolute"
    );
    assert!(
        target_directory.is_dir(),
        "fixture target must be a directory"
    );
    assert_disk_backed(&root);

    let mut sources = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jpeg"))
        })
        .collect::<Vec<_>>();
    sources.sort();
    assert_eq!(
        sources.len(),
        3,
        "comparison fixture must contain exactly three root-level JPEG files"
    );
    for source in &sources {
        assert!(
            !target_directory.join(source.file_name().unwrap()).exists(),
            "comparison target must not already exist"
        );
    }

    let store_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/file-operation-start-latency-corpus")
        .join(format!("real-fixture-{}", std::process::id()));
    fs::create_dir_all(&store_root).unwrap();
    run_real_directory_move_batch_trace(&store_root, sources, target_directory).await;
    fs::remove_dir_all(store_root).unwrap();
}

#[cfg(target_os = "linux")]
async fn run_real_directory_move_batch_trace(
    store_root: &Path,
    sources: Vec<PathBuf>,
    target_directory: PathBuf,
) {
    use std::os::unix::fs::MetadataExt;

    let transfers = sources
        .iter()
        .map(|source| {
            QueuedTransfer::new(
                source.clone(),
                target_directory.join(source.file_name().unwrap()),
            )
        })
        .collect::<Vec<_>>();
    let source_inodes = sources
        .iter()
        .map(|source| fs::metadata(source).unwrap().ino())
        .collect::<Vec<_>>();

    let store = TaskQueueStore::new(store_root.join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);

    let trace_started = Instant::now();
    let acceptance_started = Instant::now();
    let FileOperationEnqueueOutcome::Queued { .. } = queue.enqueue(QueuedFileOperation::Move {
        transfers: transfers.clone(),
        verification: FileOperationVerification::BasicMetadata,
    }) else {
        panic!("comparison fixture enqueue failed");
    };
    let acceptance_finished = Instant::now();

    let persistence_started = Instant::now();
    while let Some(request) = queue.take_next_persistence_request() {
        let outcome =
            tokio::task::spawn_blocking(move || execute_file_operation_persistence(request))
                .await
                .unwrap();
        let acceptance = queue.accept_persistence_outcome(outcome);
        assert!(acceptance.error.is_none());
    }
    let persistence_finished = Instant::now();

    let running = queue
        .active_subscription()
        .expect("comparison fixture runner must be observable");
    let stored_task_id = running
        .stored_id
        .expect("comparison fixture task must be durable");
    let targets = transfers
        .iter()
        .map(|transfer| transfer.target.clone())
        .collect::<Vec<_>>();
    let target_visibilities = Arc::new(Mutex::new(vec![None; targets.len()]));
    let observed_visibilities = Arc::clone(&target_visibilities);
    let observed_targets = targets.clone();
    let mut observer = tokio::spawn(async move {
        loop {
            let now = Instant::now();
            let all_visible = {
                let mut visibilities = observed_visibilities.lock().unwrap();
                for (index, target) in observed_targets.iter().enumerate() {
                    if visibilities[index].is_none() && target.exists() {
                        visibilities[index] = Some(now);
                    }
                }
                visibilities.iter().all(Option::is_some)
            };
            if all_visible {
                return;
            }
            tokio::task::yield_now().await;
        }
    });

    let (mut output, _messages) = mpsc::channel(32);
    let runner_started = Instant::now();
    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        stored_task_id,
        running.id,
        &mut output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    let runner_finished = Instant::now();
    match tokio::time::timeout(Duration::from_secs(1), &mut observer).await {
        Ok(joined) => joined.unwrap(),
        Err(_) => {
            observer.abort();
            let _ = observer.await;
        }
    }
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));

    let visibilities = target_visibilities.lock().unwrap();
    let visibilities = visibilities
        .iter()
        .map(|visibility| visibility.expect("comparison fixture target was not observed"))
        .collect::<Vec<_>>();
    let first_target = *visibilities.iter().min().unwrap();
    let last_target = *visibilities.iter().max().unwrap();
    println!(
        "real_move_comparison accepted_ms={:.3} persistence_started_ms={:.3} persistence_finished_ms={:.3} runner_started_ms={:.3} target_ms=[{}] runner_target_ms=[{}] spread_ms={:.3} runner_finished_ms={:.3}",
        elapsed_ms(trace_started, acceptance_finished),
        elapsed_ms(trace_started, persistence_started),
        elapsed_ms(trace_started, persistence_finished),
        elapsed_ms(trace_started, runner_started),
        visibilities
            .iter()
            .map(|visible| format!("{:.3}", elapsed_ms(trace_started, *visible)))
            .collect::<Vec<_>>()
            .join(", "),
        visibilities
            .iter()
            .map(|visible| format!("{:.3}", elapsed_ms(runner_started, *visible)))
            .collect::<Vec<_>>()
            .join(", "),
        elapsed_ms(first_target, last_target),
        elapsed_ms(trace_started, runner_finished),
    );
    assert!(
        acceptance_finished.duration_since(acceptance_started) <= Duration::from_millis(50),
        "comparison fixture UI acceptance exceeded 50 ms"
    );
    for ((source, target), source_inode) in sources.iter().zip(targets.iter()).zip(source_inodes) {
        assert!(!source.exists());
        assert_eq!(fs::metadata(target).unwrap().ino(), source_inode);
    }
}

async fn run_scenario(
    corpus_root: &Path,
    operation: HarnessOperation,
    verification: HarnessVerification,
) {
    let scenario_name = match operation {
        HarnessOperation::Copy => "copy",
        HarnessOperation::Move => "move",
    };
    let scenario_root = corpus_root.join(format!("{scenario_name}-{}", verification.name));
    fs::create_dir_all(&scenario_root).unwrap();
    let source = scenario_root.join("source.bin");
    let target = scenario_root.join("target.bin");
    write_corpus(&source);

    let store = TaskQueueStore::new(scenario_root.join("state.sqlite")).unwrap();
    let transfer = QueuedTransfer::new(source.clone(), target.clone());
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);

    let database_path = scenario_root.join("state.sqlite");
    let (writer_ready_tx, writer_ready_rx) = std::sync::mpsc::channel();
    let (writer_finished_tx, writer_finished_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let mut connection = Connection::open(database_path).unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        writer_ready_tx.send(()).unwrap();
        std::thread::sleep(WRITER_LOCK);
        drop(transaction);
        writer_finished_tx.send(Instant::now()).unwrap();
    });
    writer_ready_rx.recv().unwrap();

    let measurement_start = Instant::now();
    let acceptance_start = Instant::now();
    let outcome = queue.enqueue(operation.queued_operation(transfer.clone(), verification.value));
    let acceptance_finished = Instant::now();
    let _local_task_id = match outcome {
        FileOperationEnqueueOutcome::Queued { task_id }
        | FileOperationEnqueueOutcome::QueuedWithStorageWarning { task_id, .. } => task_id,
        FileOperationEnqueueOutcome::Rejected { error } => {
            panic!("harness enqueue failed: {error}")
        }
    };

    let request = queue
        .take_next_persistence_request()
        .expect("enqueue must produce a persistence request");
    let persistence_started = Instant::now();
    let persistence_outcome =
        tokio::task::spawn_blocking(move || execute_file_operation_persistence(request))
            .await
            .unwrap();
    let persistence_finished = Instant::now();
    let writer_released = writer_finished_rx.recv().unwrap();
    writer.join().unwrap();

    assert_eq!(persistence_outcome.request_id, 1);
    let persistence_acceptance = queue.accept_persistence_outcome(persistence_outcome);
    assert!(persistence_acceptance.error.is_none());
    assert_eq!(queue.tasks()[0].status_label(), "Preparing");

    // Drain the Running status write before recording runner start.
    while let Some(request) = queue.take_next_persistence_request() {
        let outcome =
            tokio::task::spawn_blocking(move || execute_file_operation_persistence(request))
                .await
                .unwrap();
        let acceptance = queue.accept_persistence_outcome(outcome);
        assert!(acceptance.error.is_none());
    }

    let running = queue
        .active_subscription()
        .expect("runner must be observable");
    let stored_task_id = running.stored_id.expect("recoverable task must be durable");
    let task_id = running.id;
    let controls = running.controls;
    let store = running.store;
    let (mut output, mut messages) = mpsc::channel(32);
    let first_side_effect = Arc::new(Mutex::new(None));
    let observed_side_effect = Arc::clone(&first_side_effect);
    let observed_source = source.clone();
    let observed_parent = scenario_root.to_path_buf();
    let mut observer = tokio::spawn(async move {
        loop {
            let side_effect_seen = match operation {
                HarnessOperation::Move => !observed_source.exists(),
                HarnessOperation::Copy => {
                    let mut entries = tokio::fs::read_dir(&observed_parent).await.unwrap();
                    let mut payload_written = false;
                    while let Some(entry) = entries.next_entry().await.unwrap() {
                        if !entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".file-manager-transfer-")
                        {
                            continue;
                        }
                        let payload = entry.path().join("payload");
                        if tokio::fs::metadata(payload)
                            .await
                            .is_ok_and(|metadata| metadata.len() > 0)
                        {
                            payload_written = true;
                            break;
                        }
                    }
                    payload_written
                }
            };
            if side_effect_seen {
                let mut timestamp = observed_side_effect.lock().unwrap();
                if timestamp.is_none() {
                    *timestamp = Some(Instant::now());
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    let target_visible = Arc::new(Mutex::new(None));
    let observed_target_visible = Arc::clone(&target_visible);
    let observed_target = target.clone();
    let mut target_observer = tokio::spawn(async move {
        loop {
            if observed_target.exists() {
                let mut timestamp = observed_target_visible.lock().unwrap();
                if timestamp.is_none() {
                    *timestamp = Some(Instant::now());
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });

    let runner_started = Instant::now();
    let run_task = tokio::spawn(async move {
        run_queued_transfers(
            vec![transfer],
            controls,
            stored_task_id,
            task_id,
            &mut output,
            store,
            operation.queued_mode(),
            verification.value,
        )
        .await
    });
    let mut first_progress = None;
    while let Some(message) = messages.next().await {
        if matches!(message, Message::FileOperationProgressed(_, _, _)) && first_progress.is_none() {
            first_progress = Some(Instant::now());
        }
    }
    let completion = run_task.await.unwrap();
    let runner_finished = Instant::now();
    match tokio::time::timeout(Duration::from_secs(1), &mut observer).await {
        Ok(joined) => {
            joined.unwrap();
        }
        Err(_) => {
            observer.abort();
            let _ = observer.await;
        }
    }
    match tokio::time::timeout(Duration::from_secs(1), &mut target_observer).await {
        Ok(joined) => {
            joined.unwrap();
        }
        Err(_) => {
            target_observer.abort();
            let _ = target_observer.await;
        }
    }
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));

    let first_side_effect = first_side_effect
        .lock()
        .unwrap()
        .as_ref()
        .copied()
        .expect("harness did not observe first filesystem side effect");
    let target_visible = target_visible
        .lock()
        .unwrap()
        .as_ref()
        .copied()
        .expect("harness did not observe the final target");
    if operation == HarnessOperation::Copy {
        assert!(
            first_progress.is_some(),
            "harness did not observe first copy progress"
        );
    }
    println!(
        "start_latency scenario={scenario_name} verification={} accepted_ms={:.3} persistence_started_ms={:.3} persistence_finished_ms={:.3} runner_started_ms={:.3} first_progress_ms={} first_side_effect_ms={:.3} target_visible_ms={:.3} runner_finished_ms={:.3} ui_handler_ms={:.3} sqlite_worker_ms={:.3} sqlite_lock_wait_ms={:.3} core_preparation_ms={:.3} target_visibility_ms={:.3}",
        verification.name,
        elapsed_ms(measurement_start, acceptance_finished),
        elapsed_ms(measurement_start, persistence_started),
        elapsed_ms(measurement_start, persistence_finished),
        elapsed_ms(measurement_start, runner_started),
        optional_elapsed_ms(runner_started, first_progress),
        elapsed_ms(measurement_start, first_side_effect),
        elapsed_ms(measurement_start, target_visible),
        elapsed_ms(measurement_start, runner_finished),
        elapsed_ms(acceptance_start, acceptance_finished),
        elapsed_ms(persistence_started, persistence_finished),
        elapsed_ms(persistence_started, writer_released),
        elapsed_ms(runner_started, first_side_effect),
        elapsed_ms(runner_started, target_visible),
    );

    assert!(
        acceptance_finished.duration_since(acceptance_start) <= Duration::from_millis(50),
        "UI acceptance exceeded 50 ms"
    );
    if verification.value == FileOperationVerification::BasicMetadata {
        assert!(
            first_side_effect.duration_since(runner_started) <= Duration::from_millis(100),
            "Basic {scenario_name} first side effect exceeded 100 ms"
        );
        if operation == HarnessOperation::Move {
            assert!(
                target_visible.duration_since(runner_started) <= Duration::from_millis(100),
                "Basic same-filesystem Move target visibility exceeded 100 ms"
            );
        }
    }
}

async fn run_move_batch_visibility_scenario(scenario_root: &Path) {
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = scenario_root.join(format!("source-{index}.bin"));
        let target = scenario_root.join(format!("target-{index}.bin"));
        if index == 0 {
            write_corpus(&source);
        } else {
            fs::write(&source, format!("small-{index}")).unwrap();
        }
        transfers.push(QueuedTransfer::new(source, target));
    }

    let store = TaskQueueStore::new(scenario_root.join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("batch visibility harness enqueue failed");
    };
    let running = queue
        .active_subscription()
        .expect("batch runner must be observable");
    let stored_task_id = running
        .stored_id
        .expect("batch visibility task must be durable");
    let targets = transfers
        .iter()
        .map(|transfer| transfer.target.clone())
        .collect::<Vec<_>>();
    let target_visibilities = Arc::new(Mutex::new(vec![None; targets.len()]));
    let observed_visibilities = Arc::clone(&target_visibilities);
    let mut observer = tokio::spawn(async move {
        loop {
            let now = Instant::now();
            let all_visible = {
                let mut visibilities = observed_visibilities.lock().unwrap();
                for (index, target) in targets.iter().enumerate() {
                    if visibilities[index].is_none() && target.exists() {
                        visibilities[index] = Some(now);
                    }
                }
                visibilities.iter().all(Option::is_some)
            };
            if all_visible {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });

    let (mut output, _messages) = mpsc::channel(32);
    let runner_started = Instant::now();
    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        stored_task_id,
        task_id,
        &mut output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    let runner_finished = Instant::now();
    match tokio::time::timeout(Duration::from_secs(1), &mut observer).await {
        Ok(joined) => joined.unwrap(),
        Err(_) => {
            observer.abort();
            let _ = observer.await;
        }
    }
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));

    let visibilities = target_visibilities.lock().unwrap();
    let visibilities = visibilities
        .iter()
        .map(|visibility| visibility.expect("harness did not observe every batch target"))
        .collect::<Vec<_>>();
    let first_target = *visibilities.iter().min().unwrap();
    let last_target = *visibilities.iter().max().unwrap();
    println!(
        "batch_target_visibility target_ms=[{}] spread_ms={:.3} runner_finished_ms={:.3}",
        visibilities
            .iter()
            .map(|visible| format!("{:.3}", elapsed_ms(runner_started, *visible)))
            .collect::<Vec<_>>()
            .join(", "),
        elapsed_ms(first_target, last_target),
        elapsed_ms(runner_started, runner_finished),
    );
    assert!(
        last_target.duration_since(runner_started) <= Duration::from_millis(125),
        "Basic same-filesystem Move batch visibility exceeded 125 ms"
    );
    assert!(
        last_target.duration_since(first_target) <= Duration::from_millis(75),
        "Basic same-filesystem Move batch targets did not appear as one burst"
    );
    for transfer in transfers {
        assert!(!transfer.source.exists());
        assert!(transfer.target.exists());
    }
}

fn write_corpus(path: &Path) {
    let mut file = File::create(path).unwrap();
    let chunk = vec![0x5a_u8; 1024 * 1024];
    for _ in 0..(CORPUS_SIZE / chunk.len()) {
        file.write_all(&chunk).unwrap();
    }
    file.sync_all().unwrap();
}

fn optional_elapsed_ms(start: Instant, end: Option<Instant>) -> String {
    end.map(|end| format!("{:.3}", elapsed_ms(start, end)))
        .unwrap_or_else(|| "na".to_owned())
}

fn elapsed_ms(start: Instant, end: Instant) -> f64 {
    end.duration_since(start).as_secs_f64() * 1_000.0
}

#[cfg(target_os = "linux")]
fn assert_disk_backed(path: &Path) {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    const TMPFS_MAGIC: u64 = 0x0102_1994;
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    let status = unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) };
    assert_eq!(status, 0, "cannot inspect harness filesystem");
    let stat = unsafe { stat.assume_init() };
    assert_ne!(
        stat.f_type as u64, TMPFS_MAGIC,
        "harness corpus must not use tmpfs"
    );
}

#[cfg(not(target_os = "linux"))]
fn assert_disk_backed(_path: &Path) {}
