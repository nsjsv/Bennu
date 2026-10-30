//! 文件操作执行后的结果：成功/失败/取消各自带着"已经生效的那一部分"，
//! 界面据此记撤销历史、迁移浏览路径、给完成提示。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use bennu_localization::{tr_in, UiLanguage};
use file_core::{
    CompletedBatchRename, FileError, InvalidEntryName, MetadataKind, MetadataLossSummary,
    TrashRestoreEntry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedTransfer {
    pub source: PathBuf,
    pub target: PathBuf,
}

/// 撤销收纳后保留下来的新建目录与删不掉的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptNewFolder {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperationHistoryEligibility {
    Replayable,
    NotReplayable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPathMigration {
    source: PathBuf,
    destination: PathBuf,
}

impl CompletedPathMigration {
    pub fn new(source: PathBuf, destination: PathBuf) -> Self {
        Self {
            source,
            destination,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOperationOutcome {
    NoHistory,
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    BatchRename {
        renames: Vec<CompletedBatchRename>,
    },
    CreateDirectory {
        path: PathBuf,
    },
    /// 高级新建文件夹成果：请求的叶子路径（重做按它重建）、各叶子实际建成的目录
    /// （重名时续编号）、本次新建的全部目录（按创建顺序）、整批移入第一个叶子的条目。
    CreateDirectories {
        parent: PathBuf,
        leaves: Vec<String>,
        leaf_paths: Vec<PathBuf>,
        created: Vec<PathBuf>,
        moved: Vec<CompletedTransfer>,
    },
    CreateEmptyFile {
        path: PathBuf,
    },
    /// `untrashable` 是所在位置用不了回收站、原样没动的请求路径，结束后问用户要不要
    /// 永久删除。
    Trash {
        entries: Vec<TrashRestoreEntry>,
        untrashable: Vec<PathBuf>,
    },
    Restore {
        entries: Vec<TrashRestoreEntry>,
        restored_paths: Vec<PathBuf>,
    },
    /// 不可重放的复制(例如目标冲突按 Replace/Merge 处理)也走这里而不是
    /// `NoHistory`,否则完成提示所需的 `metadata_loss` 会随之丢失。
    Copy {
        transfers: Vec<CompletedTransfer>,
        history_eligibility: FileOperationHistoryEligibility,
        metadata_loss: MetadataLossSummary,
    },
    Move {
        transfers: Vec<CompletedTransfer>,
        history_eligibility: FileOperationHistoryEligibility,
        metadata_loss: MetadataLossSummary,
    },
    /// 用选中项新建文件夹的成果:新目录连同整批移入的条目一起撤销/重做。
    GatheredIntoNewFolder {
        directory: PathBuf,
        moved: Vec<CompletedTransfer>,
    },
    /// 撤销收纳：条目都已移回；新建目录里有了别的内容时保留并提示，不算失败。
    NewFolderUngathered {
        kept_directories: Vec<KeptNewFolder>,
    },
    Convert {
        /// 逐文件失败的「源名: 原因」清单;空表示全部成功。
        failures: Vec<String>,
    },
}

/// 停下的任务带着 `partial_outcome`：停下前已经生效的那部分（什么都没做时为
/// `NoHistory`），浏览路径按它迁移，普通任务按它记撤销历史（同 Nautilus）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOperationCompletion {
    Succeeded(FileOperationOutcome),
    Failed {
        error: String,
        partial_outcome: FileOperationOutcome,
    },
    Canceled(FileOperationOutcome),
    RecoveryInterrupted(String, FileOperationOutcome),
    RecoveryBlocked {
        error: String,
        partial_outcome: FileOperationOutcome,
    },
    /// 重命名在改动任何东西之前被拒。与 `Failed` 分开：用户发起的重命名据此
    /// 重新打开输入框让用户换名字，而不只是弹一条错误。
    RenameRejected {
        path: PathBuf,
        attempted_name: OsString,
        reason: RenameRejection,
    },
}

/// 重命名被拒的原因。输入时按已加载条目的预检与执行端的原子判定给出同一种原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRejection {
    NameTaken,
    InvalidName(InvalidEntryName),
    SafeRenameUnsupported,
}

impl RenameRejection {
    pub fn from_error(error: &FileError) -> Option<Self> {
        match error {
            FileError::EntryNameTaken { .. } => Some(Self::NameTaken),
            FileError::InvalidEntryName { reason, .. } => Some(Self::InvalidName(*reason)),
            FileError::SafeRenameUnsupported { .. } => Some(Self::SafeRenameUnsupported),
            _ => None,
        }
    }

    pub fn message(self, language: UiLanguage) -> String {
        match self {
            Self::NameTaken => tr_in!(language, "A file or folder with this name already exists."),
            Self::InvalidName(InvalidEntryName::Empty) => tr_in!(language, "Enter a name."),
            Self::InvalidName(
                InvalidEntryName::CurrentDirectory | InvalidEntryName::ParentDirectory,
            ) => tr_in!(language, "\".\" and \"..\" cannot be used as names."),
            Self::InvalidName(InvalidEntryName::ContainsSlash) => {
                tr_in!(language, "Names cannot contain \"/\".")
            }
            Self::InvalidName(InvalidEntryName::ContainsNul) => {
                tr_in!(language, "Names cannot contain NUL characters.")
            }
            Self::SafeRenameUnsupported => tr_in!(
                language,
                "This file system cannot rename without risking an overwrite, so nothing was changed."
            ),
        }
    }
}

impl FileOperationCompletion {
    pub fn from_result(result: Result<FileOperationOutcome, String>) -> Self {
        match result {
            Ok(outcome) => Self::Succeeded(outcome),
            Err(error) => Self::Failed {
                error,
                partial_outcome: FileOperationOutcome::NoHistory,
            },
        }
    }

    pub fn completed_path_migrations(&self) -> Vec<CompletedPathMigration> {
        match self {
            Self::Succeeded(outcome)
            | Self::Failed {
                partial_outcome: outcome,
                ..
            }
            | Self::Canceled(outcome)
            | Self::RecoveryInterrupted(_, outcome)
            | Self::RecoveryBlocked {
                partial_outcome: outcome,
                ..
            } => outcome.completed_path_migrations(),
            Self::RenameRejected { .. } => Vec::new(),
        }
    }
}

impl FileOperationOutcome {
    pub fn completion_warning(&self, language: UiLanguage) -> Option<String> {
        const MAX_CONVERT_FAILURE_DETAILS: usize = 5;

        if let Self::Copy { metadata_loss, .. } | Self::Move { metadata_loss, .. } = self {
            return metadata_loss_warning(metadata_loss, language);
        }

        if let Self::NewFolderUngathered { kept_directories } = self {
            if kept_directories.is_empty() {
                return None;
            }
            let kept = kept_directories
                .iter()
                .map(|folder| tr_in!(language, "%1 (%2)", folder.path.display(), folder.reason))
                .collect::<Vec<_>>();
            return Some(tr_in!(
                language,
                "Items were moved back, but these new folders could not be removed: %1",
                kept.join(&tr_in!(language, "; "))
            ));
        }

        let Self::Convert { failures } = self else {
            return None;
        };
        if failures.is_empty() {
            return None;
        }
        let mut details = failures
            .iter()
            .take(MAX_CONVERT_FAILURE_DETAILS)
            .cloned()
            .collect::<Vec<_>>();
        let remaining = failures.len().saturating_sub(MAX_CONVERT_FAILURE_DETAILS);
        if remaining > 0 {
            details.push(tr_in!(language, "... (+%1)", remaining));
        }
        Some(tr_in!(
            language,
            "Some files could not be converted: %1",
            details.join(&tr_in!(language, "; "))
        ))
    }
}

/// 内容已复制成功、但部分元数据没能保留时的完成提示;摘要里的样本数已由
/// file-core 按界面展示上限截断,其余条目只计数。
fn metadata_loss_warning(
    metadata_loss: &MetadataLossSummary,
    language: UiLanguage,
) -> Option<String> {
    if metadata_loss.is_empty() {
        return None;
    }
    let mut items = metadata_loss
        .samples
        .iter()
        .map(|sample| {
            let name = sample.path.file_name().map_or_else(
                || sample.path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let kinds = sample
                .kinds
                .iter()
                .map(|kind| match kind {
                    MetadataKind::Permissions => tr_in!(language, "permissions"),
                    MetadataKind::ExtendedAttributes => tr_in!(language, "extended attributes"),
                    MetadataKind::AccessControlList => tr_in!(language, "access control list"),
                })
                .collect::<Vec<_>>()
                .join(&tr_in!(language, ", "));
            tr_in!(language, "%1 (%2)", name, kinds)
        })
        .collect::<Vec<_>>();
    let remaining = metadata_loss
        .affected_entries
        .saturating_sub(items.len() as u64);
    if remaining > 0 {
        items.push(tr_in!(language, "... (+%1)", remaining));
    }
    Some(tr_in!(
        language,
        "Some file attributes could not be preserved: %1",
        items.join(&tr_in!(language, "; "))
    ))
}

impl FileOperationOutcome {
    pub fn completed_path_migrations(&self) -> Vec<CompletedPathMigration> {
        match self {
            Self::Rename { from, to } => {
                vec![CompletedPathMigration::new(from.clone(), to.clone())]
            }
            Self::BatchRename { renames } => renames
                .iter()
                .map(|rename| CompletedPathMigration::new(rename.from.clone(), rename.to.clone()))
                .collect(),
            Self::Move { transfers, .. } => transfers
                .iter()
                .map(|transfer| {
                    CompletedPathMigration::new(transfer.source.clone(), transfer.target.clone())
                })
                .collect(),
            Self::NoHistory
            | Self::CreateDirectory { .. }
            | Self::CreateDirectories { .. }
            | Self::CreateEmptyFile { .. }
            | Self::Trash { .. }
            | Self::Restore { .. }
            | Self::Copy { .. }
            | Self::GatheredIntoNewFolder { .. }
            | Self::NewFolderUngathered { .. }
            | Self::Convert { .. } => Vec::new(),
        }
    }
}

pub fn path_after_completed_migrations(
    original_path: &Path,
    migrations: &[CompletedPathMigration],
) -> PathBuf {
    // ponytail: 交互式批次和浏览状态规模有限；若支持超大批次，再升级为组件前缀索引。
    migrations
        .iter()
        .filter_map(|migration| {
            let relative_suffix = original_path.strip_prefix(&migration.source).ok()?;
            Some((
                migration.source.components().count(),
                migration.destination.join(relative_suffix),
            ))
        })
        .max_by_key(|(source_component_count, _)| *source_component_count)
        .map(|(_, migrated_path)| migrated_path)
        .unwrap_or_else(|| original_path.to_path_buf())
}

pub fn completed_migrations_touch_directory_tree(
    directory: &Path,
    migrations: &[CompletedPathMigration],
) -> bool {
    migrations.iter().any(|migration| {
        migration.source.strip_prefix(directory).is_ok()
            || migration.destination.strip_prefix(directory).is_ok()
    })
}

pub fn completed_migrations_cross_directory_tree_boundary(
    directory: &Path,
    migrations: &[CompletedPathMigration],
) -> bool {
    migrations.iter().any(|migration| {
        let source_is_inside = migration.source.strip_prefix(directory).is_ok();
        let destination_is_inside = migration.destination.strip_prefix(directory).is_ok();
        source_is_inside != destination_is_inside
    })
}

pub fn completed_migrations_leave_directory_tree(
    directory: &Path,
    migrations: &[CompletedPathMigration],
) -> bool {
    migrations.iter().any(|migration| {
        migration.source.strip_prefix(directory).is_ok()
            && migration.destination.strip_prefix(directory).is_err()
    })
}

pub fn completed_migrations_have_source_in_directory_tree(
    directory: &Path,
    migrations: &[CompletedPathMigration],
) -> bool {
    migrations
        .iter()
        .any(|migration| migration.source.strip_prefix(directory).is_ok())
}
