//! 可恢复传输执行器的目录树端到端延迟原型:Basic 目录 Copy 走并行 staging。
//!
//! 用法:`cargo run --release -p file-core --example recoverable_tree_bench -- <source_tree> <target>`

use std::path::PathBuf;
use std::time::Instant;

use file_core::ops::{
    run_recoverable_transfer, FileTransferOptions, RecoverableTransferOperation,
    RecoverableTransferOutcome, RecoverableTransferRequest, TransferConflictStrategy,
    TransferJournal, TransferJournalFuture, TransferJournalMutation, TransferJournalRecord,
    TransferWorkKey,
};
use tokio_util::sync::CancellationToken;

struct NoopJournal {
    revision: std::sync::Mutex<u64>,
}

impl TransferJournal for NoopJournal {
    fn commit(&self, _mutation: TransferJournalMutation) -> TransferJournalFuture<'_> {
        Box::pin(async {
            let mut revision = self.revision.lock().unwrap();
            *revision += 1;
            Ok(*revision)
        })
    }
}

fn count_files(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() {
                        count_files(&path)
                    } else {
                        1
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let source = PathBuf::from(arguments.next().expect("用法: <source_tree> <target>"));
    let target = PathBuf::from(arguments.next().expect("用法: <source_tree> <target>"));

    let file_count = count_files(&source);
    let runs = 3;
    let mut timings = Vec::new();
    for run in 0..runs {
        let run_target = target.with_extension(format!("run{run}"));
        std::fs::remove_dir_all(&run_target).ok();
        // 清理上次异常退出可能残留的隐藏 staging
        for entry in std::fs::read_dir(target.parent().unwrap()).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".file-manager-transfer-")
            {
                std::fs::remove_dir_all(entry.path()).ok();
            }
        }
        let request = RecoverableTransferRequest {
            source: source.clone(),
            requested_target: run_target.clone(),
            operation: RecoverableTransferOperation::Copy,
            conflict_strategy: TransferConflictStrategy::Fail,
            verification: file_core::ops::FileOperationVerification::BasicMetadata,
        };
        let journal = NoopJournal {
            revision: std::sync::Mutex::new(0),
        };
        let record = TransferJournalRecord {
            task_id: 1,
            key: TransferWorkKey::top_level(run + 1),
            request,
            checkpoint: file_core::ops::TransferCheckpoint::AwaitingManifest,
            revision: 0,
            manifest: None,
            replacement_manifest: None,
        };
        let options = FileTransferOptions::running(CancellationToken::new());

        let start = Instant::now();
        let outcome: RecoverableTransferOutcome =
            run_recoverable_transfer(record, &journal, options)
                .await
                .expect("传输失败");
        let elapsed = start.elapsed();
        assert_eq!(outcome.final_target.as_ref(), Some(&run_target));
        timings.push(elapsed);
        std::fs::remove_dir_all(&run_target).expect("清理目标");
    }

    for (run, elapsed) in timings.iter().enumerate() {
        println!(
            "run{run}: {elapsed:?} → {:.0} files/s ({file_count} files)",
            file_count as f64 / elapsed.as_secs_f64()
        );
    }
    let fastest = timings.iter().min().unwrap();
    println!(
        "fastest: {fastest:?} → {:.0} files/s",
        file_count as f64 / fastest.as_secs_f64()
    );
}
