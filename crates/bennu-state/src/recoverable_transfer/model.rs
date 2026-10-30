use super::*;

pub(super) struct RecoverableTransferSeed<'a> {
    pub(super) transfer_index: u64,
    pub(super) operation: StoredTransferOperation,
    pub(super) transfer: &'a StoredTransfer,
    pub(super) verification: StoredFileOperationVerification,
}

pub(super) fn recoverable_transfer_seeds(
    operation: &StoredOperation,
) -> StoreResult<Vec<RecoverableTransferSeed<'_>>> {
    let (transfers, transfer_operation, verification, recovery_version) = match operation {
        StoredOperation::Copy {
            transfers,
            verification,
            recovery_version,
        } => (
            transfers,
            StoredTransferOperation::Copy,
            *verification,
            *recovery_version,
        ),
        StoredOperation::Move {
            transfers,
            verification,
            recovery_version,
        } => (
            transfers,
            StoredTransferOperation::Move,
            *verification,
            *recovery_version,
        ),
        _ => {
            return Err(StoreError::InvalidRecoverableOperation(
                "only copy and move tasks have transfer journals",
            ));
        }
    };
    if recovery_version != Some(TRANSFER_JOURNAL_VERSION) {
        return Err(StoreError::InvalidRecoverableOperation(
            "operation does not use the current journal version",
        ));
    }
    if transfers.is_empty() {
        return Err(StoreError::InvalidRecoverableOperation(
            "recoverable transfer task has no transfers",
        ));
    }

    transfers
        .iter()
        .enumerate()
        .map(|(transfer_index, transfer)| {
            Ok(RecoverableTransferSeed {
                transfer_index: u64::try_from(transfer_index)
                    .map_err(|_| StoreError::InvalidRecoverableOperation("too many transfers"))?,
                operation: transfer_operation,
                transfer,
                verification,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoredTransferConflictStrategy {
    #[default]
    Fail,
    Replace,
    Skip,
    KeepBoth,
    Merge,
}

impl StoredTransferConflictStrategy {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Replace => "replace",
            Self::Skip => "skip",
            Self::KeepBoth => "keep_both",
            Self::Merge => "merge",
        }
    }

    pub(super) fn parse(value: String) -> StoreResult<Self> {
        match value.as_str() {
            "fail" => Ok(Self::Fail),
            "replace" => Ok(Self::Replace),
            "skip" => Ok(Self::Skip),
            "keep_both" => Ok(Self::KeepBoth),
            "merge" => Ok(Self::Merge),
            _ => Err(invalid_transfer_value("conflict_strategy", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoredFileOperationVerification {
    #[default]
    BasicMetadata,
    Strong,
}

impl StoredFileOperationVerification {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::BasicMetadata => "basic_metadata",
            Self::Strong => "strong",
        }
    }

    pub(super) fn parse(value: String) -> StoreResult<Self> {
        match value.as_str() {
            "basic_metadata" => Ok(Self::BasicMetadata),
            "strong" => Ok(Self::Strong),
            _ => Err(invalid_transfer_value("verification", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredTransferOperation {
    Copy,
    Move,
}

impl StoredTransferOperation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }

    pub(super) fn parse(value: String) -> StoreResult<Self> {
        match value.as_str() {
            "copy" => Ok(Self::Copy),
            "move" => Ok(Self::Move),
            _ => Err(invalid_transfer_value("operation_kind", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredFileObjectKind {
    RegularFile,
    Directory,
    SymbolicLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredFileIdentity {
    pub device: u64,
    pub inode: u64,
    pub object_kind: StoredFileObjectKind,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
    pub symbolic_link_target: Option<StoredPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredManifestEntry {
    pub transfer_index: u64,
    pub relative_path: StoredPath,
    pub identity: StoredFileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransferWorkKey {
    pub transfer_index: u64,
    pub relative_path: StoredPath,
}

impl StoredTransferWorkKey {
    pub fn top_level(transfer_index: u64) -> Self {
        Self {
            transfer_index,
            relative_path: StoredPath::from_path(Path::new("")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredTransferCheckpointKind {
    AwaitingManifest,
    Merging,
    StageCreationIntent,
    Staging,
    DirectMoveIntent,
    DirectMoveRenamed,
    BackupCreationIntent,
    CommitIntent,
    TargetCommitted,
    SourceRetirementIntent,
    SourceRetired,
    Completed,
    CancelIntent,
    Canceled,
    FailureIntent,
    Failed,
    Skipped,
}

impl StoredTransferCheckpointKind {
    pub fn serde_state_name(self) -> &'static str {
        self.as_str()
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingManifest => "awaiting_manifest",
            Self::Merging => "merging",
            Self::StageCreationIntent => "stage_creation_intent",
            Self::Staging => "staging",
            Self::DirectMoveIntent => "direct_move_intent",
            Self::DirectMoveRenamed => "direct_move_renamed",
            Self::BackupCreationIntent => "backup_creation_intent",
            Self::CommitIntent => "commit_intent",
            Self::TargetCommitted => "target_committed",
            Self::SourceRetirementIntent => "source_retirement_intent",
            Self::SourceRetired => "source_retired",
            Self::Completed => "completed",
            Self::CancelIntent => "cancel_intent",
            Self::Canceled => "canceled",
            Self::FailureIntent => "failure_intent",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub(super) fn parse(value: String) -> StoreResult<Self> {
        match value.as_str() {
            "awaiting_manifest" => Ok(Self::AwaitingManifest),
            "merging" => Ok(Self::Merging),
            "stage_creation_intent" => Ok(Self::StageCreationIntent),
            "staging" => Ok(Self::Staging),
            "direct_move_intent" => Ok(Self::DirectMoveIntent),
            "direct_move_renamed" => Ok(Self::DirectMoveRenamed),
            "backup_creation_intent" => Ok(Self::BackupCreationIntent),
            "commit_intent" => Ok(Self::CommitIntent),
            "target_committed" => Ok(Self::TargetCommitted),
            "source_retirement_intent" => Ok(Self::SourceRetirementIntent),
            "source_retired" => Ok(Self::SourceRetired),
            "completed" => Ok(Self::Completed),
            "cancel_intent" => Ok(Self::CancelIntent),
            "canceled" => Ok(Self::Canceled),
            "failure_intent" => Ok(Self::FailureIntent),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            _ => Err(invalid_transfer_value("checkpoint_kind", value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransferCheckpoint {
    pub kind: StoredTransferCheckpointKind,
    pub state_json: String,
}

impl StoredTransferCheckpoint {
    pub fn awaiting_manifest() -> Self {
        Self {
            kind: StoredTransferCheckpointKind::AwaitingManifest,
            state_json: r#"{"state":"awaiting_manifest"}"#.to_owned(),
        }
    }

    pub fn new(kind: StoredTransferCheckpointKind, state_json: String) -> StoreResult<Self> {
        let checkpoint = Self { kind, state_json };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub(super) fn validate(&self) -> StoreResult<()> {
        let state: serde_json::Value = serde_json::from_str(&self.state_json)?;
        if state.get("state").and_then(serde_json::Value::as_str)
            != Some(self.kind.serde_state_name())
        {
            return Err(invalid_transfer_value(
                "checkpoint_kind",
                self.kind.as_str().to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransferJournalEntry {
    pub key: StoredTransferWorkKey,
    pub operation: StoredTransferOperation,
    pub source: StoredPath,
    pub requested_target: StoredPath,
    pub conflict_strategy: StoredTransferConflictStrategy,
    pub verification: StoredFileOperationVerification,
    pub checkpoint: StoredTransferCheckpoint,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMergeChildCompletion {
    pub transfer_index: u64,
    pub child_relative_path: StoredPath,
    pub completion_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransferRecoverySnapshot {
    pub journal_entries: Vec<StoredTransferJournalEntry>,
    pub manifest_entries: Vec<StoredManifestEntry>,
    pub replacement_manifest_entries: Vec<StoredManifestEntry>,
    pub merge_completions: Vec<StoredMergeChildCompletion>,
}
