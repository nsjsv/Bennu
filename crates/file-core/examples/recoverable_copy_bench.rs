//! 可恢复传输执行器的端到端延迟原型:Basic 单文件 Copy 从 AwaitingManifest
//! 到 Completed 的全部 journal 边界 + staging + 提交。
//!
//! 用法:`cargo run --release -p file-core --example recoverable_copy_bench -- <source> <target>`
//!
//! journal 为内存 no-op(返回单调 revision),不含 SQLite 持久化成本;
//! 真实任务队列链路的数字由应用层验收 harness 测量。

use std::path::PathBuf;
use std::time::Instant;

use file_core::ops::{
    persist_recoverable_source_manifest, run_recoverable_transfer, FileTransferOptions,
    RecoverableTransferOperation, RecoverableTransferOutcome, RecoverableTransferRequest,
    TransferConflictStrategy, TransferJournal, TransferJournalFuture, TransferJournalMutation,
    TransferJournalRecord, TransferWorkKey,
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

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let source = PathBuf::from(arguments.next().expect("用法: <source> <target>"));
    let target = PathBuf::from(arguments.next().expect("用法: <source> <target>"));

    let runs = 5;
    let mut timings = Vec::new();
    for run in 0..runs {
        let source_path = source.with_extension(format!("run{run}"));
        std::fs::remove_file(&source_path).ok();
        std::fs::hard_link(&source, &source_path).expect("hard link 源");
        let run_target = target.with_extension(format!("run{run}"));
        // 上一次异常退出可能残留目标或隐藏 staging
        std::fs::remove_file(&run_target).ok();
        for entry in std::fs::read_dir(target.parent().unwrap()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name
                .to_string_lossy()
                .starts_with(".file-manager-transfer-")
            {
                std::fs::remove_dir_all(entry.path()).ok();
            }
        }
        let request = RecoverableTransferRequest {
            source: source_path.clone(),
            requested_target: run_target.clone(),
            operation: RecoverableTransferOperation::Copy,
            conflict_strategy: TransferConflictStrategy::Fail,
            verification: file_core::ops::FileOperationVerification::BasicMetadata,
        };
        let mut record = TransferJournalRecord {
            task_id: 1,
            key: TransferWorkKey::top_level(run + 1),
            request,
            checkpoint: file_core::ops::TransferCheckpoint::AwaitingManifest,
            revision: 0,
            manifest: None,
            replacement_manifest: None,
        };
        let journal = NoopJournal {
            revision: std::sync::Mutex::new(0),
        };
        persist_recoverable_source_manifest(&mut record, &journal)
            .await
            .expect("manifest 装订失败");

        let start = Instant::now();
        let outcome: RecoverableTransferOutcome = run_recoverable_transfer(
            record,
            &journal,
            FileTransferOptions::running(fresh_cancel_token()),
        )
        .await
        .expect("传输失败");
        let elapsed = start.elapsed();
        assert_eq!(outcome.final_target.as_ref(), Some(&run_target));
        timings.push(elapsed);
        std::fs::remove_file(&run_target).expect("清理目标");
        std::fs::remove_file(&source_path).expect("清理源");
    }

    for (run, elapsed) in timings.iter().enumerate() {
        println!(
            "run{run}: {:?} ({:.1} MiB)",
            elapsed,
            std::fs::metadata(&source).unwrap().len() as f64 / 1024.0 / 1024.0
        );
    }
    let fastest = timings.iter().min().unwrap();
    println!("fastest: {fastest:?}");
}

fn fresh_cancel_token() -> CancellationToken {
    CancellationToken::new()
}
