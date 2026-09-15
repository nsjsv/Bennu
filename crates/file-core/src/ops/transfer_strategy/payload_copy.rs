//! 普通文件 payload 的策略化搬运：克隆 → 内核快拷 → 用户态循环的阶梯执行。

use std::io::{self, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::fs::{copy_file_range, fadvise, ioctl_ficlone, Advice};
use rustix::io::Errno;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::{PayloadCopyStrategy, TransferStrategyEngine};
use crate::ops::copy::{CopyProgress, FileOperationControls, ProgressSender};
use crate::FileError;

/// `copy_file_range` 单块搬运量：8 MiB 在进度粒度与系统调用次数之间折中。
const KERNEL_RANGE_CHUNK_SIZE: usize = 8 * 1024 * 1024;
/// 用户态循环缓冲：顺序大文件所需系统调用次数比旧 1 MiB 实现少 4 倍。
const USER_LOOP_BUFFER_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadCopyOutcome {
    pub strategy: PayloadCopyStrategy,
    pub bytes: u64,
}

/// 把单个普通文件搬运到尚不存在的 `target`（create_new 语义由本函数持有）。
///
/// `source_hasher` 只在 UserLoop 路径被喂数据：克隆与内核快拷不过用户态，
/// Strong 校验的调用方须依据返回 outcome 的 strategy 自行补读源内容计算哈希。
pub struct RegularFilePayloadCopy<'a> {
    pub source: &'a Path,
    pub target: &'a Path,
    pub source_device: u64,
    pub bytes_total: u64,
    pub engine: &'a TransferStrategyEngine,
    pub controls: &'a mut FileOperationControls,
    pub progress: Option<&'a ProgressSender>,
    pub source_hasher: Option<&'a mut blake3::Hasher>,
}

pub async fn copy_regular_file_payload(
    copy: RegularFilePayloadCopy<'_>,
) -> Result<PayloadCopyOutcome, FileError> {
    let RegularFilePayloadCopy {
        source,
        target,
        source_device,
        bytes_total,
        engine,
        controls,
        progress,
        mut source_hasher,
    } = copy;
    let mut source_file = File::open(source)
        .await
        .map_err(|error_source| copy_error(source, target, error_source))?;
    // tokio fs::File 默认单次派发上限 2 MiB,会对缓冲再切半;对齐后
    // 顺序大文件每个缓冲一次系统调用。
    source_file.set_max_buf_size(USER_LOOP_BUFFER_SIZE);
    let mut target_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
        .map_err(|error_source| copy_error(source, target, error_source))?;
    target_file.set_max_buf_size(USER_LOOP_BUFFER_SIZE);
    // 目标设备号在 create_new 之后才能拿到：payload 目录与最终目标同设备。
    let target_device = target_file
        .metadata()
        .await
        .map_err(|error_source| copy_error(source, target, error_source))?
        .dev();

    let mut attempted = engine.strategy(source_device, target_device);
    loop {
        let outcome = match attempted {
            PayloadCopyStrategy::KernelClone => {
                try_kernel_clone(source, target, &source_file, &target_file, bytes_total, progress)
                    .await
            }
            PayloadCopyStrategy::KernelRange => {
                try_kernel_range(
                    source,
                    target,
                    &source_file,
                    &target_file,
                    controls,
                    progress,
                    bytes_total,
                )
                .await
            }
            PayloadCopyStrategy::UserLoop => {
                try_user_loop(UserLoopCopy {
                    source,
                    target,
                    source_file: &mut source_file,
                    target_file: &mut target_file,
                    controls,
                    progress,
                    bytes_total,
                    source_hasher: source_hasher.as_deref_mut(),
                })
                .await
            }
        };

        match outcome {
            Ok(outcome) => {
                advise_dont_need(&source_file, &target_file, outcome.strategy);
                return Ok(outcome);
            }
            Err(StrategyFailure::UnsupportedDevice { from }) => {
                match from {
                    PayloadCopyStrategy::KernelClone => {
                        engine.record_kernel_clone_unsupported(source_device, target_device)
                    }
                    PayloadCopyStrategy::KernelRange => {
                        engine.record_kernel_range_unsupported(source_device, target_device)
                    }
                    // 用户态循环不依赖任何设备能力，没有可降级的层级。
                    PayloadCopyStrategy::UserLoop => unreachable!(),
                }
                attempted = engine.strategy(source_device, target_device);
                reset_target_for_retry(&mut target_file)
                    .await
                    .map_err(|error_source| copy_error(source, target, error_source))?;
            }
            Err(StrategyFailure::Cancelled) => return Err(FileError::Cancelled),
            Err(StrategyFailure::ApplicationStopping) => Err(FileError::ApplicationStopping)?,
            Err(StrategyFailure::Io(error)) => return Err(copy_error(source, target, error)),
        }
    }
}

enum StrategyFailure {
    /// 设备/文件系统不支持该策略；目标内容已被重置为空，可降级重试。
    UnsupportedDevice { from: PayloadCopyStrategy },
    Cancelled,
    ApplicationStopping,
    Io(io::Error),
}

/// 内核快路径"设备/文件系统不支持"的 errno 集合。权限类错误（EPERM/EACCES）
/// 不属于该集合——那是真实失败，降级会掩盖错误。
fn is_unsupported_device_error(error: Errno) -> bool {
    matches!(
        error.raw_os_error(),
        libc::EOPNOTSUPP | libc::EINVAL | libc::EXDEV | libc::ENOSYS | libc::ENOTTY
    )
}

fn control_failure(error: FileError) -> StrategyFailure {
    match error {
        FileError::Cancelled => StrategyFailure::Cancelled,
        FileError::ApplicationStopping => StrategyFailure::ApplicationStopping,
        other => StrategyFailure::Io(io::Error::other(other)),
    }
}

async fn try_kernel_clone(
    source: &Path,
    target: &Path,
    source_file: &File,
    target_file: &File,
    bytes_total: u64,
    progress: Option<&ProgressSender>,
) -> Result<PayloadCopyOutcome, StrategyFailure> {
    // FICLONE 是原子 ioctl：成功即全部数据块已共享，失败即目标保持空文件，
    // 不存在半克隆状态，因此失败后无需清理即可降级重试。
    if let Err(error) = ioctl_ficlone(target_file, source_file) {
        if is_unsupported_device_error(error) {
            return Err(StrategyFailure::UnsupportedDevice {
                from: PayloadCopyStrategy::KernelClone,
            });
        }
        return Err(StrategyFailure::Io(error.into()));
    }
    if let Some(progress) = progress {
        let _ = progress.send(CopyProgress {
            from: source.to_path_buf(),
            to: target.to_path_buf(),
            bytes_done: bytes_total,
            bytes_total,
        });
    }
    Ok(PayloadCopyOutcome {
        strategy: PayloadCopyStrategy::KernelClone,
        bytes: bytes_total,
    })
}

async fn try_kernel_range(
    source: &Path,
    target: &Path,
    source_file: &File,
    target_file: &File,
    controls: &mut FileOperationControls,
    progress: Option<&ProgressSender>,
    bytes_total: u64,
) -> Result<PayloadCopyOutcome, StrategyFailure> {
    // 内核快拷是同步系统调用：本地文件系统上单块为内核内 memcpy（毫秒级），
    // 显式偏移量让 fd 自身偏移保持零点，降级重试无需重新打开。
    let mut source_offset = 0u64;
    let mut target_offset = 0u64;
    let mut bytes_done = 0u64;
    let mut first_chunk = true;
    loop {
        controls
            .wait_until_running()
            .await
            .map_err(control_failure)?;
        match copy_file_range(
            source_file,
            Some(&mut source_offset),
            target_file,
            Some(&mut target_offset),
            KERNEL_RANGE_CHUNK_SIZE,
        ) {
            Ok(0) => break,
            Ok(moved) => {
                bytes_done += moved as u64;
                first_chunk = false;
                if let Some(progress) = progress {
                    let _ = progress.send(CopyProgress {
                        from: source.to_path_buf(),
                        to: target.to_path_buf(),
                        bytes_done,
                        bytes_total,
                    });
                }
            }
            Err(error) if first_chunk && is_unsupported_device_error(error) => {
                return Err(StrategyFailure::UnsupportedDevice {
                    from: PayloadCopyStrategy::KernelRange,
                });
            }
            Err(error) => return Err(StrategyFailure::Io(error.into())),
        }
    }
    Ok(PayloadCopyOutcome {
        strategy: PayloadCopyStrategy::KernelRange,
        bytes: bytes_done,
    })
}

/// UserLoop 阶梯位的入参集合;字段含义与外层请求一致。
struct UserLoopCopy<'a> {
    source: &'a Path,
    target: &'a Path,
    source_file: &'a mut File,
    target_file: &'a mut File,
    controls: &'a mut FileOperationControls,
    progress: Option<&'a ProgressSender>,
    bytes_total: u64,
    source_hasher: Option<&'a mut blake3::Hasher>,
}

async fn try_user_loop(
    UserLoopCopy {
        source,
        target,
        source_file,
        target_file,
        controls,
        progress,
        bytes_total,
        mut source_hasher,
    }: UserLoopCopy<'_>,
) -> Result<PayloadCopyOutcome, StrategyFailure> {
    let mut buffer = vec![0u8; USER_LOOP_BUFFER_SIZE];
    let mut bytes_done = 0u64;
    loop {
        controls
            .wait_until_running()
            .await
            .map_err(control_failure)?;
        let read = source_file
            .read(&mut buffer)
            .await
            .map_err(StrategyFailure::Io)?;
        if read == 0 {
            break;
        }
        if let Some(hasher) = source_hasher.as_deref_mut() {
            hasher.update(&buffer[..read]);
        }
        target_file
            .write_all(&buffer[..read])
            .await
            .map_err(StrategyFailure::Io)?;
        bytes_done += read as u64;
        if let Some(progress) = progress {
            let _ = progress.send(CopyProgress {
                from: source.to_path_buf(),
                to: target.to_path_buf(),
                bytes_done,
                bytes_total,
            });
        }
    }
    // tokio fs::File 的写先进内部缓冲、由后台 worker 异步刷入内核;不在此处
    // flush 的话,调用方立刻读取或提交会拿到尚未刷完的部分文件。旧 copy.rs
    // 循环后的 flush 语义由此而来,必须保持。
    target_file.flush().await.map_err(StrategyFailure::Io)?;
    Ok(PayloadCopyOutcome {
        strategy: PayloadCopyStrategy::UserLoop,
        bytes: bytes_done,
    })
}

/// 降级重试前把目标截断回空文件：克隆失败本就未写字节，内核快拷可能
/// 已部分搬运后才遇到不可继续的错误。
async fn reset_target_for_retry(target_file: &mut File) -> Result<(), io::Error> {
    target_file.set_len(0).await?;
    target_file.seek(SeekFrom::Start(0)).await?;
    Ok(())
}

/// 搬运完成后丢弃源端页缓存，避免大文件复制挤掉应用工作集。目标端只有
/// UserLoop 走过完整脏页路径，且 DONTNEED 不会释放脏页（内核等写回），
/// 因此同样安全；错误一律忽略，这是纯优化提示。
fn advise_dont_need(source_file: &File, target_file: &File, strategy: PayloadCopyStrategy) {
    let _ = fadvise(source_file, 0, None, Advice::DontNeed);
    // 目标端仅 UserLoop 走过完整脏页路径;DONTNEED 不会释放脏页,安全。
    if strategy == PayloadCopyStrategy::UserLoop {
        let _ = fadvise(target_file, 0, None, Advice::DontNeed);
    }
}

fn copy_error(source: &Path, target: &Path, error_source: impl Into<io::Error>) -> FileError {
    FileError::Copy {
        from: source.to_path_buf(),
        to: target.to_path_buf(),
        source: error_source.into(),
    }
}
