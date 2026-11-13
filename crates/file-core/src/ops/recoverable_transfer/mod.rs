use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

mod artifacts;
mod durability;
mod executor;
mod fingerprint;
mod identity;
mod manifest;
pub(crate) mod path_codec;
mod proof;
mod proof_memo;
mod protocol;
mod rename;

#[cfg(test)]
pub(crate) use artifacts::create_owned_artifact;
pub(crate) use artifacts::{
    owner_marker_written, plan_owned_artifact, recover_owned_artifact, remove_empty_owned_artifact,
    remove_owned_artifact, remove_owned_artifact_if_exists, remove_unclaimed_artifact,
    validate_owned_artifact,
};
pub use artifacts::{
    ArtifactOwner, ArtifactToken, OwnedArtifact, OwnedArtifactKind, OwnedArtifactPlan,
};
pub(crate) use durability::CommitBarrier;
pub use executor::{
    advance_recoverable_transfer, durable_direct_move_commit, persist_source_manifests,
    run_recoverable_segment, run_recoverable_transfer, settle_failed_recoverable_transfers,
    DurableDirectMoveCommit, TransferAdvance, TransferSegmentEvent,
};
pub use fingerprint::ObjectFingerprint;
pub(crate) use fingerprint::{fingerprint_object, fingerprint_object_with_controls};
pub(crate) use identity::inspect_file_identity;
pub use identity::{FileIdentity, FileObjectKind};
#[cfg(test)]
pub(crate) use manifest::build_source_manifest;
pub(crate) use manifest::{
    build_counted_source_manifest, build_source_manifest_with_controls, verify_source_manifest,
    verify_source_manifest_with_controls,
};
pub use manifest::{SourceManifest, SourceManifestEntry};
pub(crate) use proof::ProofContext;
pub use proof::TransferFingerprint;
pub(crate) use proof_memo::{ProofMemo, SharedProofMemo};
pub use protocol::{
    BackupCreationTransfer, CommitPayload, CommitTransfer, CommittedTransfer, CompletedTarget,
    MergeChildCompletion, MergeChildOutcome, MergeTransfer, OwnedTreeEntryDeletionIntent,
    PreparedTransfer, RecoverableTransferOperation, RecoverableTransferOutcome,
    RecoverableTransferRequest, RenamedDirectMove, RetiredSource, SourceDisposition,
    SourceRetirementPlan, StagedSourceLocation, StagingTransfer, TransferCheckpoint,
    TransferExecutionKind, TransferFailureIntent, TransferJournal, TransferJournalError,
    TransferJournalFuture, TransferJournalMutation, TransferJournalRecord, TransferTransition,
    TransferWorkKey,
};
pub(crate) use rename::{rename_noreplace, NoReplaceRenameError};

#[derive(Debug, Error)]
pub enum RecoverableTransferError {
    #[error("could not {action} {path:?}: {source}")]
    FileSystem {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not safely rename {from:?} to {to:?}: {source}")]
    SafeRename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("source changed while preparing transfer: {path:?}")]
    SourceChanged { path: PathBuf },
    #[error("unsupported transfer object: {path:?}")]
    UnsupportedObject { path: PathBuf },
    #[error("owned transfer artifact failed validation at {path:?}: {reason}")]
    ArtifactOwnership { path: PathBuf, reason: String },
    #[error("transfer target changed or already exists: {path:?}")]
    TargetConflict { path: PathBuf },
    #[error("transfer journal failed: {message}")]
    Journal { message: String },
    #[error("transfer requires startup recovery before it can continue: {diagnostic}")]
    RecoveryRequired { diagnostic: String },
    #[error("transfer recovery is blocked: {diagnostic}")]
    RecoveryBlocked { diagnostic: String },
    #[error("recoverable transfer failed after cleanup: {diagnostic}")]
    RecordedFailure { diagnostic: String },
    #[error("invalid recoverable transfer checkpoint: {message}")]
    InvalidCheckpoint { message: String },
    #[error("staged content does not match the source snapshot: {path:?}")]
    FingerprintMismatch { path: PathBuf },
    #[error(transparent)]
    FileOperation(#[from] crate::FileError),
    #[error("could not obtain a transfer ownership token: {0}")]
    RandomToken(String),
}

impl RecoverableTransferError {
    fn file_system(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::FileSystem {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    fn artifact_ownership(path: &Path, reason: impl Into<String>) -> Self {
        Self::ArtifactOwnership {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests;
