//! Basic 校验模式的运行内证明记忆。
//!
//! 协议原本在每个 checkpoint 都全文重读同一对象(单个文件复制全程被读
//! 5 遍)。按已确认的红线修订二,Basic 模式下同一对象每次前向运行至多
//! 全文哈希一次:memo 以完整 FileIdentity(含 ctime)为键登记已证哈希,
//! 后续检查点身份精确命中即复用。ctime 无法被普通用户改写,因此"改内容
//! 但保留大小/时间戳"的篡改仍会使键失配、触发重新全文校验。memo 不落
//! 盘:崩溃重启后的恢复路径每次都全文重验。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{ObjectFingerprint, RecoverableTransferError};

/// memo 键 = 完整身份事实(device/inode/size/mtime/ctime)。
/// rename 只改路径不改 inode,键天然跨 rename 命中;硬链接等改变 nlink 的
/// 操作会更新 ctime,键失配,安全方向是多哈希一次。
pub type ProofMemoKey = (u64, u64, u64, i64, i64, i64, i64);

#[derive(Default)]
pub struct ProofMemo {
    entries: HashMap<ProofMemoKey, ObjectFingerprint>,
}

impl ProofMemo {
    pub fn shared() -> SharedProofMemo {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn lookup(&self, key: &ProofMemoKey) -> Option<ObjectFingerprint> {
        self.entries.get(key).copied()
    }

    pub fn insert(&mut self, key: ProofMemoKey, fingerprint: ObjectFingerprint) {
        self.entries.insert(key, fingerprint);
    }
}

pub type SharedProofMemo = Arc<Mutex<ProofMemo>>;

pub fn memo_lookup(
    memo: &SharedProofMemo,
    key: &ProofMemoKey,
) -> Result<Option<ObjectFingerprint>, RecoverableTransferError> {
    memo.lock()
        .map(|memo| memo.lookup(key))
        .map_err(|poison_error| {
            RecoverableTransferError::Journal {
                message: format!("proof memo poisoned: {poison_error}"),
            }
        })
}

pub fn memo_insert(
    memo: &SharedProofMemo,
    key: ProofMemoKey,
    fingerprint: ObjectFingerprint,
) -> Result<(), RecoverableTransferError> {
    memo.lock()
        .map(|mut memo| memo.insert(key, fingerprint))
        .map_err(|poison_error| {
            RecoverableTransferError::Journal {
                message: format!("proof memo poisoned: {poison_error}"),
            }
        })
}
