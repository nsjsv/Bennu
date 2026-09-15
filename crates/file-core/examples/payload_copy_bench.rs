//! 策略化 payload 搬运的吞吐原型对照,用于与 cp 基线及各策略互比。
//!
//! 用法：
//! `cargo run --release -p file-core --example payload_copy_bench -- <source> <target> [auto|clone|range|user]`
//!
//! 策略参数缺省 auto(按设备探测);钉死策略用于在支持克隆的文件系统上
//! 单独测出快拷/用户态循环的真实吞吐。

use std::path::PathBuf;
use std::time::Instant;

use file_core::ops::{
    copy_regular_file_payload, FileOperationControls, PayloadCopyStrategy,
    RegularFilePayloadCopy, TransferStrategyEngine,
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let source = PathBuf::from(arguments.next().expect("用法: <source> <target> [strategy]"));
    let target = PathBuf::from(arguments.next().expect("用法: <source> <target> [strategy]"));
    let forced = arguments.next().filter(|name| name != "auto").map(|name| match name.as_str() {
        "clone" => PayloadCopyStrategy::KernelClone,
        "range" => PayloadCopyStrategy::KernelRange,
        "user" => PayloadCopyStrategy::UserLoop,
        other => panic!("未知策略 {other},可选 auto|clone|range|user"),
    });

    let bytes_total = std::fs::metadata(&source).expect("读源文件").len();
    let source_device = {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(&source)
            .expect("读源设备号")
            .dev()
    };
    let engine = TransferStrategyEngine::probe(source_device, source_device, &target);
    if let Some(forced) = forced {
        engine.force_strategy(source_device, source_device, forced);
    }
    let mut controls = FileOperationControls::running(CancellationToken::new());

    let start = Instant::now();
    let outcome = copy_regular_file_payload(RegularFilePayloadCopy {
        source: &source,
        target: &target,
        source_device,
        bytes_total,
        engine: &engine,
        controls: &mut controls,
        progress: None,
        source_hasher: None,
    })
    .await
    .expect("payload 搬运失败");
    let elapsed = start.elapsed();
    let gib_per_second = outcome.bytes as f64 / 1024.0 / 1024.0 / 1024.0 / elapsed.as_secs_f64();
    println!(
        "{:?}: {} bytes in {:.3?} → {:.2} GiB/s",
        outcome.strategy, outcome.bytes, elapsed, gib_per_second
    );
}
