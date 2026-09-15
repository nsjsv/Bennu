//! 提交证明形态与 Basic 运行内证明判定。
//!
//! `TransferFingerprint` 是 Commit/Committed/Completed 检查点里 payload 的
//! 所有权证明:Blake3 形态是全文哈希(原语义);KernelClone 形态记录
//! FICLONE 原子克隆事实——克隆事后无法从内容重新证明,其后续一切校验
//! 都退化为与登记身份的精确匹配。删除/恢复/备份证明永远要求 Blake3,
//! 该红线不变。

use std::path::Path;

use super::super::FileOperationVerification;
use super::super::copy::FileOperationControls;
use super::{
    fingerprint_object, fingerprint_object_with_controls, inspect_file_identity, FileIdentity,
    ObjectFingerprint, RecoverableTransferError,
};
use super::proof_memo::{memo_insert, memo_lookup, SharedProofMemo};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum TransferFingerprint {
    /// 全文 BLAKE3;journal 旧格式(裸 32 字节数组)反序列化为此形态,零迁移。
    Blake3(ObjectFingerprint),
    /// FICLONE 原子克隆 + 前后身份夹心的证明;后续校验 = 精确身份匹配。
    KernelClone { payload_identity: FileIdentity },
}

impl TransferFingerprint {
    /// 删除/恢复/备份证明仍要求全文哈希;KernelClone 证明的对象必须先用
    /// 该方法补一次全文哈希升级,才能进入这些路径。
    pub fn as_blake3(&self) -> Option<ObjectFingerprint> {
        match self {
            Self::Blake3(fingerprint) => Some(*fingerprint),
            Self::KernelClone { .. } => None,
        }
    }
}

/// 一次前向运行的证明判定上下文:校验模式 + 可选的 Basic 运行内 memo。
/// 在 `advance_recoverable_transfer` 入口按 record 的校验模式构造,逐层
/// 传给全部指纹比较点;Strong 模式 memo 恒为 None,行为与历史版本一致。
#[derive(Clone)]
pub struct ProofContext {
    memo: Option<SharedProofMemo>,
}

impl ProofContext {
    /// 无 memo 的退化上下文:settle 结算等无传输选项的路径用,行为等同
    /// 历史版本(每次全文重算,结算路径本就只做一次性校验)。
    pub fn blank_proof() -> Self {
        Self { memo: None }
    }

    /// Strong 是显式的"宁可慢也要严"模式:memo 置空,每个检查点全文重读。
    pub fn new(
        verification: FileOperationVerification,
        memo: Option<SharedProofMemo>,
    ) -> Self {
        let memo = match verification {
            FileOperationVerification::BasicMetadata => memo,
            FileOperationVerification::Strong => None,
        };
        Self { memo }
    }

    /// 当前对象的内容哈希:Basic 命中 memo 则复用,未命中(或 Strong)全文
    /// 哈希一次并登记。返回值可直接作为 Blake3 证明写入删除/备份事实。
    pub async fn fingerprint_object(
        &self,
        path: &Path,
    ) -> Result<ObjectFingerprint, RecoverableTransferError> {
        let Some(memo) = self.memo.as_ref() else {
            return fingerprint_object(path).await;
        };
        let identity = inspect_file_identity(path).await?;
        let key = identity.proof_memo_key();
        if let Some(fingerprint) = memo_lookup(memo, &key)? {
            return Ok(fingerprint);
        }
        let fingerprint = fingerprint_object(path).await?;
        memo_insert(memo, key, fingerprint)?;
        Ok(fingerprint)
    }

    /// staging 路径的哈希:夹心(身份→哈希→身份)由调用方持有,memo 登记
    /// 以哈希后复核通过的身份为准。
    pub async fn fingerprint_object_with_controls(
        &self,
        path: &Path,
        controls: &FileOperationControls,
    ) -> Result<ObjectFingerprint, RecoverableTransferError> {
        let Some(memo) = self.memo.as_ref() else {
            return fingerprint_object_with_controls(path, controls).await;
        };
        let identity = inspect_file_identity(path).await?;
        let key = identity.proof_memo_key();
        if let Some(fingerprint) = memo_lookup(memo, &key)? {
            return Ok(fingerprint);
        }
        let fingerprint = fingerprint_object_with_controls(path, controls).await?;
        memo_insert(memo, key, fingerprint)?;
        Ok(fingerprint)
    }

    /// 校验 path 当前状态是否与期望证明一致。
    ///
    /// Blake3 期望:Basic 走 memo(身份命中即复用已证哈希,未命中重新全文
    /// 哈希);Strong 每次全文重读。KernelClone 期望:仅精确身份匹配——
    /// 内核克隆的原子性在克隆时刻已定,事后无法从内容重新推导。
    pub async fn matches(
        &self,
        path: &Path,
        expected: &TransferFingerprint,
    ) -> Result<bool, RecoverableTransferError> {
        match expected {
            TransferFingerprint::Blake3(expected_fingerprint) => {
                Ok(self.fingerprint_object(path).await? == *expected_fingerprint)
            }
            TransferFingerprint::KernelClone { payload_identity } => {
                // 提交 rename 会更新 inode 的 ctime(btrfs 实测),因此克隆
                // 证明的身份匹配采用快照语义(kind/size/mtime 桶),不比 ctime。
                let current = inspect_file_identity(path).await?;
                Ok(current.matches_staging_snapshot(payload_identity))
            }
        }
    }
}
