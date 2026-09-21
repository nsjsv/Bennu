use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::payload_copy::{copy_regular_file_payload, RegularFilePayloadCopy};
use super::{PayloadCopyStrategy, TransferStrategyEngine};
use crate::ops::copy::{CopyProgress, FileOperationControls, FileOperationRunState};
use crate::FileError;

fn running_controls() -> FileOperationControls {
    FileOperationControls::running(CancellationToken::new())
}

#[test]
fn initial_strategy_follows_device_pair() {
    let directory = tempfile::tempdir().unwrap();
    let engine = TransferStrategyEngine::probe(7, 7, directory.path());
    assert_eq!(
        engine.strategy(7, 7),
        PayloadCopyStrategy::KernelClone,
        "同设备对必须从克隆起步"
    );
    assert_eq!(
        engine.strategy(7, 8),
        PayloadCopyStrategy::KernelRange,
        "跨设备对从内核快拷起步,克隆不可跨文件系统"
    );
}

#[test]
fn ladder_downgrades_monotonically_and_never_upgrades() {
    let directory = tempfile::tempdir().unwrap();
    let engine = TransferStrategyEngine::probe(7, 7, directory.path());

    engine.record_kernel_clone_unsupported(7, 7);
    assert_eq!(engine.strategy(7, 7), PayloadCopyStrategy::KernelRange);

    engine.record_kernel_range_unsupported(7, 7);
    assert_eq!(engine.strategy(7, 7), PayloadCopyStrategy::UserLoop);

    engine.record_kernel_clone_unsupported(7, 7);
    assert_eq!(
        engine.strategy(7, 7),
        PayloadCopyStrategy::UserLoop,
        "迟到的克隆降级不得把阶梯拉回快路径"
    );
}

#[test]
fn parallelism_follows_device_capabilities_table() {
    // 各设备/挂载形态的并行度阶梯;核数上限同时约束所有形态。
    assert_eq!(parallelism_case(true, false, false, 16), 4);
    assert_eq!(parallelism_case(false, true, false, 16), 2);
    assert_eq!(parallelism_case(false, false, true, 16), 12);
    assert_eq!(parallelism_case(false, false, false, 16), 8);
    assert_eq!(parallelism_case(false, false, true, 4), 4, "核数不足时封顶");
    assert_eq!(
        parallelism_case(false, false, true, 1),
        1,
        "至少保留 1 并发"
    );
}

fn parallelism_case(is_fuse: bool, is_rotational: bool, is_nvme: bool, cpus: usize) -> usize {
    super::parallelism_from_capabilities(is_fuse, is_rotational, is_nvme, cpus)
}

async fn write_random_file(path: &std::path::Path, size: usize) -> Vec<u8> {
    let mut content = vec![0u8; size];
    getrandom::fill(&mut content).unwrap();
    std::fs::write(path, &content).unwrap();
    content
}

#[tokio::test]
async fn payload_copy_preserves_content_on_default_ladder() {
    // 默认阶梯命中的策略依赖宿主文件系统能力(btrfs 走克隆,tmpfs 走快拷),
    // 这里只锁定内容一致性与字节数;具体策略由钉死阶梯的专项用例覆盖。
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let target_path = directory.path().join("target.bin");
    let content = write_random_file(&source_path, 3 * 1024 * 1024).await;
    let source_device = std::fs::symlink_metadata(&source_path).unwrap().dev();
    let engine = TransferStrategyEngine::probe(source_device, source_device, directory.path());
    let mut controls = running_controls();

    let outcome = copy_regular_file_payload(RegularFilePayloadCopy {
        source: &source_path,
        target: &target_path,
        source_device,
        bytes_total: content.len() as u64,
        engine: &engine,
        controls: &mut controls,
        progress: None,
        source_hasher: None,
    })
    .await
    .unwrap();

    assert_eq!(std::fs::read(&target_path).unwrap(), content);
    assert_eq!(outcome.bytes, content.len() as u64);
}

#[tokio::test]
async fn forced_user_loop_copies_and_second_file_hits_cached_strategy() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let content = write_random_file(&source_path, 1024 * 1024).await;
    let source_device = std::fs::symlink_metadata(&source_path).unwrap().dev();
    let engine = Arc::new(TransferStrategyEngine::probe(
        source_device,
        source_device,
        directory.path(),
    ));
    engine.force_strategy(source_device, source_device, PayloadCopyStrategy::UserLoop);

    for index in 0..2 {
        let mut controls = running_controls();
        let target_path = directory.path().join(format!("target{index}.bin"));
        let outcome = copy_regular_file_payload(RegularFilePayloadCopy {
            source: &source_path,
            target: &target_path,
            source_device,
            bytes_total: content.len() as u64,
            engine: &engine,
            controls: &mut controls,
            progress: None,
            source_hasher: None,
        })
        .await
        .unwrap();
        assert_eq!(outcome.strategy, PayloadCopyStrategy::UserLoop);
        assert_eq!(std::fs::read(&target_path).unwrap(), content);
    }
}

#[tokio::test]
async fn unsupported_ladder_falls_back_to_user_loop_and_still_copies() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let content = write_random_file(&source_path, 2 * 1024 * 1024).await;
    let source_device = std::fs::symlink_metadata(&source_path).unwrap().dev();
    let engine = TransferStrategyEngine::probe(source_device, source_device, directory.path());
    // 同时钉死克隆与快拷都"不支持",模拟老文件系统:阶梯必须一路降级到
    // 用户态循环并完成搬运,而不是报错。
    engine.record_kernel_clone_unsupported(source_device, source_device);
    engine.record_kernel_range_unsupported(source_device, source_device);
    let mut controls = running_controls();

    let target_path = directory.path().join("target.bin");
    let outcome = copy_regular_file_payload(RegularFilePayloadCopy {
        source: &source_path,
        target: &target_path,
        source_device,
        bytes_total: content.len() as u64,
        engine: &engine,
        controls: &mut controls,
        progress: None,
        source_hasher: None,
    })
    .await
    .unwrap();

    assert_eq!(outcome.strategy, PayloadCopyStrategy::UserLoop);
    assert_eq!(std::fs::read(&target_path).unwrap(), content);
}

#[tokio::test]
async fn progress_events_are_monotonic_and_end_at_total() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let content = write_random_file(&source_path, 9 * 1024 * 1024).await;
    let source_device = std::fs::symlink_metadata(&source_path).unwrap().dev();
    let engine = TransferStrategyEngine::probe(source_device, source_device, directory.path());
    // 9 MiB 源 + 4 MiB 缓冲固定产生 3 个进度事件,断言与宿主文件系统无关。
    engine.force_strategy(source_device, source_device, PayloadCopyStrategy::UserLoop);
    let (sender, receiver) = mpsc::unbounded_channel::<CopyProgress>();
    let mut controls = running_controls();

    let target_path = directory.path().join("target.bin");
    copy_regular_file_payload(RegularFilePayloadCopy {
        source: &source_path,
        target: &target_path,
        source_device,
        bytes_total: content.len() as u64,
        engine: &engine,
        controls: &mut controls,
        progress: Some(&sender),
        source_hasher: None,
    })
    .await
    .unwrap();
    drop(sender);

    let mut receiver = receiver;
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event);
    }
    // 事件数取决于 tokio fs::File 的内部单次读上限(MAX_BUF,当前 2 MiB),
    // 属于实现细节;这里锁定真正的不变量:非空、单调递增、收尾等于总量。
    assert!(!events.is_empty(), "9 MiB 源必须产生至少一个进度事件");
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].bytes_done < pair[1].bytes_done),
        "进度必须单调递增"
    );
    let last = events.last().unwrap();
    assert_eq!(last.bytes_done, content.len() as u64);
    assert_eq!(last.bytes_total, content.len() as u64);
}

#[tokio::test]
async fn cancel_while_paused_in_user_loop_returns_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.bin");
    let content = write_random_file(&source_path, 1024 * 1024).await;
    let source_device = std::fs::symlink_metadata(&source_path).unwrap().dev();
    let engine = Arc::new(TransferStrategyEngine::probe(
        source_device,
        source_device,
        directory.path(),
    ));
    engine.force_strategy(source_device, source_device, PayloadCopyStrategy::UserLoop);

    // 先置为 Paused 再启动复制:循环在首个 wait_until_running 处挂起,随后
    // 取消令牌触发,确定性地命中取消路径,不依赖计时窗口。
    let cancellation = CancellationToken::new();
    let (run_state_sender, run_state) = watch::channel(FileOperationRunState::Running);
    run_state_sender.send_replace(FileOperationRunState::Paused);
    let controls = FileOperationControls::new(cancellation.clone(), run_state);

    let target_path = directory.path().join("target.bin");
    let copy_task = tokio::spawn(async move {
        let mut controls = controls;
        copy_regular_file_payload(RegularFilePayloadCopy {
            source: &source_path,
            target: &target_path,
            source_device,
            bytes_total: content.len() as u64,
            engine: &engine,
            controls: &mut controls,
            progress: None,
            source_hasher: None,
        })
        .await
    });
    cancellation.cancel();
    let error = copy_task.await.unwrap().unwrap_err();
    assert!(matches!(error, FileError::Cancelled));
}
