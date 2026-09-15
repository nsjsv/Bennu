//! 传输策略引擎：按 (源设备, 目标设备) 探测并缓存普通文件 payload 的搬运方式，
//! 并按目标设备能力推导并行度。
//!
//! 策略阶梯为 FICLONE 克隆 → copy_file_range → 用户态循环；阶梯只在真实搬运
//! 遇到"文件系统不支持"类错误时单调降级，降级结果在任务生命周期内缓存，
//! 后续文件直接命中，不再重复尝试已确认不支持的快路径。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod payload_copy;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub use payload_copy::{
    copy_regular_file_payload, PayloadCopyOutcome, RegularFilePayloadCopy,
};

/// 单个普通文件 payload 的搬运方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadCopyStrategy {
    /// `ioctl FICLONE`：内核原子克隆，仅同文件系统可用；失败时目标保持空文件。
    KernelClone,
    /// `copy_file_range` 分块循环；同文件系统上多数实现内部仍会走克隆。
    KernelRange,
    /// 大缓冲用户态读-写循环，不依赖任何内核快路径的兜底。
    UserLoop,
}

impl PayloadCopyStrategy {
    /// 阶梯位置：数值越大越保守，降级只允许沿阶梯向数值大的方向移动。
    fn ladder_position(self) -> u8 {
        match self {
            Self::KernelClone => 0,
            Self::KernelRange => 1,
            Self::UserLoop => 2,
        }
    }

    fn from_ladder_position(position: u8) -> Self {
        match position {
            0 => Self::KernelClone,
            1 => Self::KernelRange,
            _ => Self::UserLoop,
        }
    }
}

/// 单任务生命周期的策略缓存。方法全部取 `&self` 以便并行 staging 的多个
/// worker 共享同一份探测结论；内部状态只做罕见的单调降级写入。
pub struct TransferStrategyEngine {
    strategies: Mutex<HashMap<(u64, u64), u8>>,
    parallelism: usize,
}

impl TransferStrategyEngine {
    /// `source_device`/`target_device` 为两侧 `st_dev`；`target_probe_path`
    /// 用于识别 FUSE 挂载（网盘、rclone 等），其并行度受单次往返延迟支配
    /// 而非带宽，需要单独压制。
    pub fn probe(source_device: u64, target_device: u64, target_probe_path: &Path) -> Self {
        let _ = source_device;
        Self {
            strategies: Mutex::new(HashMap::new()),
            parallelism: target_parallelism(target_device, target_probe_path),
        }
    }

    pub fn parallelism(&self) -> usize {
        self.parallelism
    }

    /// 当前该设备对应尝试的策略。同设备从克隆起步，跨设备从内核快拷起步；
    /// 已降级过的设备对返回缓存结论。
    pub fn strategy(&self, source_device: u64, target_device: u64) -> PayloadCopyStrategy {
        let initial = if source_device == target_device {
            PayloadCopyStrategy::KernelClone
        } else {
            PayloadCopyStrategy::KernelRange
        };
        let cached = self
            .strategies
            .lock()
            .unwrap()
            .get(&(source_device, target_device))
            .copied();
        cached
            .map(PayloadCopyStrategy::from_ladder_position)
            .unwrap_or(initial)
    }

    pub fn record_kernel_clone_unsupported(&self, source_device: u64, target_device: u64) {
        self.downgrade_to(
            source_device,
            target_device,
            PayloadCopyStrategy::KernelRange,
        );
    }

    pub fn record_kernel_range_unsupported(&self, source_device: u64, target_device: u64) {
        self.downgrade_to(source_device, target_device, PayloadCopyStrategy::UserLoop);
    }

    /// 把策略钉死在指定阶梯位，不再按设备对探测。供基准对照与测试使用；
    /// 真实搬运遇到不支持错误时仍会正常降级。
    pub fn force_strategy(
        &self,
        source_device: u64,
        target_device: u64,
        forced: PayloadCopyStrategy,
    ) {
        self.strategies
            .lock()
            .unwrap()
            .insert((source_device, target_device), forced.ladder_position());
    }

    fn downgrade_to(&self, source_device: u64, target_device: u64, floor: PayloadCopyStrategy) {
        let mut strategies = self.strategies.lock().unwrap();
        let entry = strategies
            .entry((source_device, target_device))
            .or_insert_with(|| floor.ladder_position());
        if floor.ladder_position() > *entry {
            *entry = floor.ladder_position();
        }
    }
}

/// 探测目录所在文件系统是否支持 FICLONE 克隆:真实执行一次小文件克隆。
/// 供测试与诊断使用;引擎本体不依赖它(真实搬运失败自然降级)。
pub fn ficlone_supported(directory: &Path) -> bool {
    let probe = match tempfile_probe(directory) {
        Some(paths) => paths,
        None => return false,
    };
    let outcome = (|| {
        let source = std::fs::File::open(&probe.0).ok()?;
        let target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe.1)
            .ok()?;
        rustix::fs::ioctl_ficlone(&target, &source).ok()
    })();
    let _ = std::fs::remove_file(&probe.1);
    let _ = std::fs::remove_file(&probe.0);
    outcome.is_some()
}

fn tempfile_probe(directory: &Path) -> Option<(PathBuf, PathBuf)> {
    let source = directory.join(".ficlone-probe-source");
    std::fs::write(&source, b"probe").ok()?;
    Some((source, directory.join(".ficlone-probe-target")))
}

const FUSE_SUPER_MAGIC: i64 = 0x65735546;
const MAX_TRANSFER_PARALLELISM: usize = 16;

fn target_parallelism(target_device: u64, target_probe_path: &Path) -> usize {
    let cpus = std::thread::available_parallelism()
        .map_or(4, |count| count.get())
        .min(MAX_TRANSFER_PARALLELISM);
    parallelism_from_capabilities(
        statfs_is_fuse(target_probe_path),
        block_device_is_rotational(target_device),
        block_device_is_nvme(target_device),
        cpus,
    )
}

/// 并行度推导的纯函数部分：探测结果作输入，保证可单测。
/// NVMe 队列深、SATA SSD 次之、机械盘受寻道约束只能低并发、FUSE 受单次
/// 往返延迟约束取小并发。
fn parallelism_from_capabilities(
    is_fuse: bool,
    is_rotational: bool,
    is_nvme: bool,
    cpus: usize,
) -> usize {
    let device_parallelism = if is_fuse {
        4
    } else if is_rotational {
        2
    } else if is_nvme {
        12
    } else {
        8
    };
    device_parallelism.clamp(1, cpus.max(1))
}

fn statfs_is_fuse(path: &Path) -> bool {
    rustix::fs::statfs(path)
        .map(|stat| stat.f_type == FUSE_SUPER_MAGIC)
        .unwrap_or(false)
}

fn sysfs_block_directory(device: u64) -> PathBuf {
    // dev_t 的 major/minor 必须经 libc 宏拆分：大设备号的 minor 分居高低段，
    // 直接移位会得到错误的 sysfs 路径。
    let major = libc::major(device);
    let minor = libc::minor(device);
    PathBuf::from(format!("/sys/dev/block/{major}:{minor}"))
}

fn block_device_is_rotational(device: u64) -> bool {
    std::fs::read_to_string(sysfs_block_directory(device).join("queue/rotational"))
        .map(|content| content.trim() == "1")
        .unwrap_or(false)
}

fn block_device_is_nvme(device: u64) -> bool {
    std::fs::read_link(sysfs_block_directory(device).join("device"))
        .map(|target| target.to_string_lossy().contains("nvme"))
        .unwrap_or(false)
}
