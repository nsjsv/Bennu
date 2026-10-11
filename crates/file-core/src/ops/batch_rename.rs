use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::FileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRenameItem {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedBatchRename {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// 回滚没能把条目改回原名:用户的原名、条目此刻实际所在位置、恢复失败的原因。
/// 用户要靠这三项自己找回文件,所以回滚的每一步失败都必须落到这里。
#[derive(Debug)]
pub struct UnrestoredBatchRename {
    pub original: PathBuf,
    pub current: PathBuf,
    pub error: FileError,
}

#[derive(Debug, Clone)]
struct BatchRenameStep {
    source: PathBuf,
    temporary: PathBuf,
    target: PathBuf,
}

pub async fn batch_rename_paths(
    items: Vec<BatchRenameItem>,
) -> Result<Vec<CompletedBatchRename>, FileError> {
    // 校验、预留临时名、两阶段改名与回滚都是同步文件系统调用,整批放进一个
    // 阻塞任务;逐个 tokio::fs 调用只会多出线程切换。
    let execution = tokio::task::spawn_blocking(move || {
        run_batch_rename(items, &mut |from: &Path, to: &Path| {
            std::fs::rename(from, to)
        })
    })
    .await;
    match execution {
        Ok(result) => result,
        Err(join_error) if join_error.is_panic() => {
            std::panic::resume_unwind(join_error.into_panic())
        }
        // 阻塞任务只会在运行时关闭时被取消。
        Err(_) => Err(FileError::ApplicationStopping),
    }
}

/// `rename` 由调用方注入:正式路径传 `std::fs::rename`。回滚失败只会在
/// "第 k 次改名失败"时出现,真实文件系统无法在两次改名之间确定性地制造,
/// 测试经同一入口注入失败,正式与测试共用全部执行和回滚逻辑。
fn run_batch_rename(
    items: Vec<BatchRenameItem>,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<Vec<CompletedBatchRename>, FileError> {
    let items = validated_batch_rename_items(items)?;
    let steps = prepare_batch_rename_steps(&items)?;
    execute_batch_rename_steps(&steps, rename)?;

    Ok(items
        .into_iter()
        .filter(|item| item.from != item.to)
        .map(|item| CompletedBatchRename {
            from: item.from,
            to: item.to,
        })
        .collect())
}

fn execute_batch_rename_steps(
    steps: &[BatchRenameStep],
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), FileError> {
    let mut renamed_to_temporary = Vec::new();
    for step in steps {
        if let Err(error) = rename_for_batch(rename, &step.source, &step.temporary) {
            let unrestored = rollback_temporary_batch_renames(&renamed_to_temporary, rename);
            return Err(batch_rename_failure(error, unrestored));
        }
        renamed_to_temporary.push((step.temporary.clone(), step.source.clone()));
    }

    let mut completed_steps = Vec::new();
    for step in steps {
        if let Err(error) = rename_for_batch(rename, &step.temporary, &step.target) {
            let unrestored =
                rollback_final_batch_renames(&completed_steps, &renamed_to_temporary, rename);
            return Err(batch_rename_failure(error, unrestored));
        }
        completed_steps.push((step.target.clone(), step.source.clone()));
    }

    Ok(())
}

/// 回滚全部成功时用户的文件都已回到原名,只需报告最初的失败。
fn batch_rename_failure(cause: FileError, unrestored: Vec<UnrestoredBatchRename>) -> FileError {
    if unrestored.is_empty() {
        cause
    } else {
        FileError::BatchRenameIncomplete {
            cause: Box::new(cause),
            unrestored,
        }
    }
}

fn validated_batch_rename_items(
    items: Vec<BatchRenameItem>,
) -> Result<Vec<BatchRenameItem>, FileError> {
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    let source_set = items
        .iter()
        .map(|item| item.from.clone())
        .collect::<HashSet<_>>();

    for item in &items {
        let Some(parent) = item.from.parent() else {
            return Err(invalid_batch_rename_input(
                &item.from,
                "source path has no parent",
            ));
        };
        if item.to.parent() != Some(parent) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target must stay in the source directory",
            ));
        }
        if item.to.file_name().is_none_or(|name| name.is_empty()) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target name cannot be empty",
            ));
        }
        if !sources.insert(item.from.clone()) {
            return Err(invalid_batch_rename_input(
                &item.from,
                "source appears more than once",
            ));
        }
        if !targets.insert(item.to.clone()) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target appears more than once",
            ));
        }
        if item.from != item.to && !path_exists(&item.from)? {
            return Err(FileError::Metadata {
                path: item.from.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, "source does not exist"),
            });
        }
        if item.from != item.to && !source_set.contains(&item.to) && path_exists(&item.to)? {
            return Err(FileError::Rename {
                from: item.from.clone(),
                to: item.to.clone(),
                source: io::Error::new(io::ErrorKind::AlreadyExists, "target already exists"),
            });
        }
    }

    Ok(items)
}

fn prepare_batch_rename_steps(
    items: &[BatchRenameItem],
) -> Result<Vec<BatchRenameStep>, FileError> {
    let mut reserved = items
        .iter()
        .flat_map(|item| [item.from.clone(), item.to.clone()])
        .collect::<HashSet<_>>();
    let mut steps = Vec::new();

    for item in items.iter().filter(|item| item.from != item.to) {
        let temporary = available_batch_rename_temporary_path(&item.from, &mut reserved)?;
        steps.push(BatchRenameStep {
            source: item.from.clone(),
            temporary,
            target: item.to.clone(),
        });
    }

    Ok(steps)
}

fn available_batch_rename_temporary_path(
    source: &Path,
    reserved: &mut HashSet<PathBuf>,
) -> Result<PathBuf, FileError> {
    let parent = source
        .parent()
        .ok_or_else(|| invalid_batch_rename_input(source, "source path has no parent"))?;
    source
        .file_name()
        .ok_or_else(|| invalid_batch_rename_input(source, "source name cannot be empty"))?;

    for index in 0..1000 {
        let candidate = parent.join(format!(".bennu-batch-rename-{index}.tmp"));
        if reserved.contains(&candidate) {
            continue;
        }
        if path_exists(&candidate)? {
            continue;
        }
        reserved.insert(candidate.clone());
        return Ok(candidate);
    }

    Err(invalid_batch_rename_input(
        source,
        "could not reserve a temporary rename path",
    ))
}

fn rename_for_batch(
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
    from: &Path,
    to: &Path,
) -> Result<(), FileError> {
    rename(from, to).map_err(|source| FileError::Rename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })
}

fn path_exists(path: &Path) -> Result<bool, FileError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FileError::Metadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn rollback_temporary_batch_renames(
    renames: &[(PathBuf, PathBuf)],
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Vec<UnrestoredBatchRename> {
    let mut unrestored = Vec::new();
    for (temporary_path, original_path) in renames.iter().rev() {
        if let Err(error) = rename_for_batch(rename, temporary_path, original_path) {
            unrestored.push(UnrestoredBatchRename {
                original: original_path.clone(),
                current: temporary_path.clone(),
                error,
            });
        }
    }
    unrestored
}

fn rollback_final_batch_renames(
    completed_steps: &[(PathBuf, PathBuf)],
    temporary_steps: &[(PathBuf, PathBuf)],
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Vec<UnrestoredBatchRename> {
    let mut reserved = temporary_steps
        .iter()
        .flat_map(|(temporary_path, original_path)| [temporary_path.clone(), original_path.clone()])
        .chain(
            completed_steps
                .iter()
                .flat_map(|(final_path, original_path)| {
                    [final_path.clone(), original_path.clone()]
                }),
        )
        .collect::<HashSet<_>>();
    let mut unrestored = Vec::new();
    let mut completed_rollback_steps = Vec::new();

    // 已改成目标名的条目先挪到新的临时名,腾出目标名,避免互换/环形改名
    // 在恢复原名时互相覆盖。
    for (final_path, original_path) in completed_steps.iter().rev() {
        let parked = available_batch_rename_temporary_path(final_path, &mut reserved).and_then(
            |rollback_temporary| {
                rename_for_batch(rename, final_path, &rollback_temporary)?;
                Ok(rollback_temporary)
            },
        );
        match parked {
            Ok(rollback_temporary) => {
                completed_rollback_steps.push((rollback_temporary, original_path.clone()))
            }
            Err(error) => unrestored.push(UnrestoredBatchRename {
                original: original_path.clone(),
                current: final_path.clone(),
                error,
            }),
        }
    }

    // 已完成的条目由上一步与下面的 completed_rollback_steps 负责,这里只恢复
    // 仍停在第一阶段临时名上的条目。
    let completed_originals = completed_steps
        .iter()
        .map(|(_, original_path)| original_path.clone())
        .collect::<HashSet<_>>();
    let still_temporary = temporary_steps
        .iter()
        .filter(|(_, original_path)| !completed_originals.contains(original_path))
        .cloned();
    for (temporary_path, original_path) in still_temporary.chain(completed_rollback_steps) {
        if let Err(error) = rename_for_batch(rename, &temporary_path, &original_path) {
            unrestored.push(UnrestoredBatchRename {
                original: original_path,
                current: temporary_path,
                error,
            });
        }
    }

    unrestored
}

fn invalid_batch_rename_input(path: &Path, message: &'static str) -> FileError {
    FileError::InvalidInput {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "batch_rename_tests.rs"]
mod tests;
