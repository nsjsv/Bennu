use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use super::super::copy::{FileOperationVerification, TransferConflictStrategy};
use super::{
    ArtifactOwner, FileIdentity, ObjectFingerprint, OwnedArtifact, OwnedArtifactPlan,
    SourceManifest, SourceManifestEntry, TransferFingerprint,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverableTransferOperation {
    Copy,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecoverableTransferRequest {
    #[serde(with = "super::path_codec")]
    pub source: PathBuf,
    #[serde(with = "super::path_codec")]
    pub requested_target: PathBuf,
    pub operation: RecoverableTransferOperation,
    pub conflict_strategy: TransferConflictStrategy,
    pub verification: FileOperationVerification,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TransferWorkKey {
    pub transfer_index: u64,
    #[serde(with = "super::path_codec")]
    pub relative_path: PathBuf,
}

impl TransferWorkKey {
    pub fn top_level(transfer_index: u64) -> Self {
        Self {
            transfer_index,
            relative_path: PathBuf::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferExecutionKind {
    CopyToStage,
    MoveDirect,
    MoveToStage,
    MergeDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreparedTransfer {
    pub source_identity: FileIdentity,
    #[serde(with = "super::path_codec")]
    pub resolved_target: PathBuf,
    pub expected_target_identity: Option<FileIdentity>,
    pub expected_target_fingerprint: Option<ObjectFingerprint>,
    #[serde(with = "required_optional_fingerprint")]
    pub source_fingerprint: Option<ObjectFingerprint>,
    pub execution: TransferExecutionKind,
    pub staging_plan: Option<OwnedArtifactPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedSourceLocation {
    OriginalPath,
    ArtifactPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StagingTransfer {
    pub prepared: PreparedTransfer,
    pub artifact: OwnedArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "fields", rename_all = "snake_case")]
pub enum CommitPayload {
    DirectSource {
        identity: FileIdentity,
    },
    Artifact {
        artifact: OwnedArtifact,
        payload_identity: FileIdentity,
        source_location: StagedSourceLocation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitTransfer {
    pub prepared: PreparedTransfer,
    pub payload: CommitPayload,
    pub fingerprint: TransferFingerprint,
    #[serde(default)]
    pub backup_identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenamedDirectMove {
    pub prepared: PreparedTransfer,
    pub target_identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackupCreationTransfer {
    pub prepared: PreparedTransfer,
    pub payload: CommitPayload,
    pub fingerprint: ObjectFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDisposition {
    Preserved,
    MovedByCommit,
    RequiresRetirement,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommittedTransfer {
    #[serde(with = "super::path_codec")]
    pub final_target: PathBuf,
    pub target_identity: FileIdentity,
    pub fingerprint: TransferFingerprint,
    pub artifact: Option<OwnedArtifact>,
    pub source_disposition: SourceDisposition,
    #[serde(default)]
    pub backup_identity: Option<FileIdentity>,
    pub backup_fingerprint: Option<ObjectFingerprint>,
    #[serde(default)]
    pub backup_cleanup_index: usize,
    #[serde(default)]
    pub backup_cleanup_intent: Option<OwnedTreeEntryDeletionIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceRetirementPlan {
    pub committed: CommittedTransfer,
    pub artifact_plan: OwnedArtifactPlan,
    #[serde(default)]
    pub artifact: Option<OwnedArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetiredSource {
    pub committed: CommittedTransfer,
    pub artifact: OwnedArtifact,
    #[serde(default)]
    pub payload_identity: Option<FileIdentity>,
    #[serde(default)]
    pub cleanup_index: usize,
    #[serde(default)]
    pub cleanup_intent: Option<OwnedTreeEntryDeletionIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnedTreeEntryDeletionIntent {
    pub entry: SourceManifestEntry,
    pub fingerprint: Option<ObjectFingerprint>,
    #[serde(default)]
    pub expected_identity: Option<FileIdentity>,
    #[serde(default)]
    pub deletion_slot_identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", content = "fields", rename_all = "snake_case")]
pub enum MergeChildOutcome {
    Committed(CompletedTarget),
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeChildCompletion {
    pub parent_key: TransferWorkKey,
    pub child_key: TransferWorkKey,
    pub outcome: MergeChildOutcome,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MergeTransfer {
    pub target_root_identity: FileIdentity,
    pub next_child: usize,
    pub active_child: Option<Box<TransferJournalRecord>>,
    #[serde(skip)]
    pub child_names: Vec<PathBuf>,
    #[serde(skip)]
    pub completed_children: Vec<MergeChildCompletion>,
    #[serde(skip)]
    pub completed_prefix_verified: bool,
}

impl PartialEq for MergeTransfer {
    fn eq(&self, other: &Self) -> bool {
        self.target_root_identity == other.target_root_identity
            && self.next_child == other.next_child
            && self.active_child == other.active_child
    }
}

impl Eq for MergeTransfer {}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompletedTarget {
    #[serde(with = "super::path_codec")]
    pub path: PathBuf,
    pub identity: FileIdentity,
    pub fingerprint: TransferFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferFailureIntent {
    pub previous: Box<TransferCheckpoint>,
    pub diagnostic: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "fields", rename_all = "snake_case")]
pub enum TransferCheckpoint {
    AwaitingManifest,
    Merging(MergeTransfer),
    StageCreationIntent(PreparedTransfer),
    Staging(StagingTransfer),
    DirectMoveIntent(PreparedTransfer),
    DirectMoveRenamed(RenamedDirectMove),
    BackupCreationIntent(BackupCreationTransfer),
    CommitIntent(CommitTransfer),
    TargetCommitted(CommittedTransfer),
    SourceRetirementIntent(Box<SourceRetirementPlan>),
    SourceRetired(Box<RetiredSource>),
    Completed(CompletedTarget),
    CancelIntent(Box<TransferCheckpoint>),
    Canceled {
        #[serde(with = "super::path_codec::optional")]
        final_target: Option<PathBuf>,
    },
    FailureIntent(TransferFailureIntent),
    Failed {
        #[serde(with = "super::path_codec::optional")]
        final_target: Option<PathBuf>,
        diagnostic: String,
    },
    Skipped,
}

mod required_optional_fingerprint {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ObjectFingerprint;

    pub fn serialize<S>(value: &Option<ObjectFingerprint>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<ObjectFingerprint>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ObjectFingerprint>::deserialize(deserializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferJournalRecord {
    pub task_id: u64,
    pub key: TransferWorkKey,
    pub request: RecoverableTransferRequest,
    pub checkpoint: TransferCheckpoint,
    pub revision: u64,
    #[serde(skip)]
    pub manifest: Option<SourceManifest>,
    #[serde(skip)]
    pub replacement_manifest: Option<SourceManifest>,
}

impl TransferJournalRecord {
    pub fn owner(&self, work_index: u64) -> ArtifactOwner {
        ArtifactOwner {
            task_id: self.task_id,
            transfer_index: self.key.transfer_index,
            work_index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferJournalMutation {
    InstallManifestAndCheckpoint {
        task_id: u64,
        key: TransferWorkKey,
        expected_revision: u64,
        manifest: SourceManifest,
        replacement_manifest: Option<SourceManifest>,
        checkpoint: TransferCheckpoint,
    },
    CompareAndSwapCheckpoint {
        task_id: u64,
        key: TransferWorkKey,
        expected_revision: u64,
        checkpoint: TransferCheckpoint,
    },
    PersistMergeCompletionAndCheckpoint {
        task_id: u64,
        key: TransferWorkKey,
        expected_revision: u64,
        completion: MergeChildCompletion,
        checkpoint: TransferCheckpoint,
    },
    InstallManifestAndCheckpointBatch {
        updates: Vec<ManifestCheckpointBatchUpdate>,
    },
}

/// One checkpoint compare-and-swap in a batch commit. A batch swap is only
/// valid when every swap moves a distinct top-level record forward by exactly
/// one revision; it is the persistence unit for the post-rename facts of a
/// Basic DirectMove visibility segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferCheckpointSwap {
    pub task_id: u64,
    pub key: TransferWorkKey,
    pub expected_revision: u64,
    pub checkpoint: TransferCheckpoint,
}

/// One manifest + checkpoint install inside a batch commit. Every update moves
/// a distinct top-level record forward by exactly one revision and installs the
/// manifest entries for that record in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestCheckpointBatchUpdate {
    pub task_id: u64,
    pub key: TransferWorkKey,
    pub expected_revision: u64,
    pub manifest: SourceManifest,
    pub replacement_manifest: Option<SourceManifest>,
    pub checkpoint: TransferCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferJournalError {
    StaleRevision,
    UserCancelled,
    ApplicationStopping,
    Storage(String),
}

pub type TransferJournalFuture<'a> =
    Pin<Box<dyn Future<Output = Result<u64, TransferJournalError>> + Send + 'a>>;

pub trait TransferJournal: Send + Sync {
    fn commit(&self, mutation: TransferJournalMutation) -> TransferJournalFuture<'_>;

    /// Journals must opt into an atomic batch implementation. A journal that
    /// cannot provide this boundary fails closed instead of silently turning a
    /// segment back into per-record persistence.
    fn commit_checkpoint_batch<'a>(
        &'a self,
        _swaps: Vec<TransferCheckpointSwap>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>> {
        Box::pin(async {
            Err(TransferJournalError::Storage(
                "atomic checkpoint batches are not supported by this journal".to_owned(),
            ))
        })
    }

    fn commit_manifest_batch<'a>(
        &'a self,
        _updates: Vec<ManifestCheckpointBatchUpdate>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>> {
        Box::pin(async {
            Err(TransferJournalError::Storage(
                "atomic manifest batches are not supported by this journal".to_owned(),
            ))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableTransferOutcome {
    pub source: PathBuf,
    pub final_target: Option<PathBuf>,
}
