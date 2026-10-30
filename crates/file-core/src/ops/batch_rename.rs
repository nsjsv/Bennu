use std::collections::HashSet;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use super::entry_moves::{
    move_without_replacing, park_at_temporary_name, EntryFilesystem, SystemEntryFilesystem,
    UnrestoredEntry,
};
use crate::entry_naming::validate_entry_name;
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

#[derive(Debug, Clone)]
struct BatchRenameStep {
    source: PathBuf,
    temporary: PathBuf,
    target: PathBuf,
}

pub async fn batch_rename_paths(
    items: Vec<BatchRenameItem>,
) -> Result<Vec<CompletedBatchRename>, FileError> {
    // 校验、两阶段改名与回滚都是同步文件系统调用,整批放进一个阻塞任务;
    // 逐个 tokio::fs 调用只会多出线程切换。
    let execution =
        tokio::task::spawn_blocking(move || run_batch_rename(&mut SystemEntryFilesystem, items))
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

/// 文件系统操作经 `EntryFilesystem` 注入:回滚失败只会在"第 k 次改名失败"时出现,
/// 真实文件系统无法在两次改名之间确定性地制造,测试经同一接口注入,正式与测试
/// 共用全部执行和回滚逻辑。
fn run_batch_rename(
    filesystem: &mut impl EntryFilesystem,
    items: Vec<BatchRenameItem>,
) -> Result<Vec<CompletedBatchRename>, FileError> {
    validate_batch_rename_items(filesystem, &items)?;
    let moving = items
        .into_iter()
        .filter(|item| item.from != item.to)
        .collect::<Vec<_>>();
    execute_batch_rename(filesystem, &moving)?;

    Ok(moving
        .into_iter()
        .map(|item| CompletedBatchRename {
            from: item.from,
            to: item.to,
        })
        .collect())
}

/// 先把所有源挪到临时名,再从临时名落到目标名:互换、环形改名的目标在第二阶段
/// 都已空出。两个阶段都只用 no-replace 改名,执行期间别处新建的同名条目不会被覆盖。
fn execute_batch_rename(
    filesystem: &mut impl EntryFilesystem,
    items: &[BatchRenameItem],
) -> Result<(), FileError> {
    let mut steps = Vec::with_capacity(items.len());
    for item in items {
        match park_source(filesystem, &item.from) {
            Ok(temporary) => steps.push(BatchRenameStep {
                source: item.from.clone(),
                temporary,
                target: item.to.clone(),
            }),
            Err(cause) => {
                let parked = steps
                    .iter()
                    .rev()
                    .map(|step| (step.temporary.clone(), step.source.clone()));
                let unrestored = restore_parked(filesystem, parked);
                return Err(batch_rename_failure(cause, unrestored));
            }
        }
    }

    for (landed, step) in steps.iter().enumerate() {
        if let Err(cause) = move_without_replacing(filesystem, &step.temporary, &step.target) {
            let unrestored = roll_back_landed(filesystem, &steps[..landed], &steps[landed..]);
            return Err(batch_rename_failure(cause, unrestored));
        }
    }

    Ok(())
}

/// 回滚全部成功时用户的文件都已回到原名,只需报告最初的失败。
fn batch_rename_failure(cause: FileError, unrestored: Vec<UnrestoredEntry>) -> FileError {
    if unrestored.is_empty() {
        cause
    } else {
        FileError::EntryMovesIncomplete {
            cause: Box::new(cause),
            unrestored,
            leftover_entries: Vec::new(),
        }
    }
}

/// 输入在边界上校验一次。目标是否被占用以第二阶段的 no-replace 改名为准;这里先按
/// 对象身份挡掉已经看得见的冲突,免得整批挪到临时名再回滚。
fn validate_batch_rename_items(
    filesystem: &mut impl EntryFilesystem,
    items: &[BatchRenameItem],
) -> Result<(), FileError> {
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    for item in items {
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
        // 取最后一个路径成分而不是 file_name():后者遇到 ".." 返回 None,会被报成空名。
        let target_name = item
            .to
            .components()
            .next_back()
            .map_or(OsStr::new(""), |component| component.as_os_str());
        validate_entry_name(target_name).map_err(|reason| FileError::InvalidEntryName {
            name: target_name.to_os_string(),
            reason,
        })?;
        if !sources.insert(&item.from) {
            return Err(invalid_batch_rename_input(
                &item.from,
                "source appears more than once",
            ));
        }
        if !targets.insert(&item.to) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target appears more than once",
            ));
        }
    }

    let moving = items
        .iter()
        .filter(|item| item.from != item.to)
        .collect::<Vec<_>>();
    // 目标名被本批要挪走的对象占着不算冲突:互换、环形改名,或大小写不敏感的文件
    // 系统上只改大小写(新旧名字指向同一个对象)。同一对象的另一个硬链接名在这里
    // 放行,第二阶段 no-replace 撞上后回滚并按已占用报告。
    let mut moving_objects = HashSet::new();
    for item in &moving {
        match filesystem.object_identity(&item.from) {
            Ok(identity) => moving_objects.insert(identity),
            Err(source) => {
                return Err(FileError::Metadata {
                    path: item.from.clone(),
                    source,
                })
            }
        };
    }
    for item in &moving {
        match filesystem.object_identity(&item.to) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(occupant) if moving_objects.contains(&occupant) => {}
            Ok(_) => {
                return Err(FileError::EntryNameTaken {
                    path: item.to.clone(),
                })
            }
            Err(source) => {
                return Err(FileError::Metadata {
                    path: item.to.clone(),
                    source,
                })
            }
        }
    }

    Ok(())
}

fn park_source(filesystem: &mut impl EntryFilesystem, source: &Path) -> Result<PathBuf, FileError> {
    let directory = source
        .parent()
        .expect("validated batch rename paths have a parent directory");
    park_at_temporary_name(filesystem, source, directory)
}

/// 第二阶段失败:已落到目标名的先挪到新的临时名,腾出目标名(互换、环形改名里它
/// 可能正是别的条目的原名),再把所有临时名放回原名。
fn roll_back_landed(
    filesystem: &mut impl EntryFilesystem,
    landed: &[BatchRenameStep],
    pending: &[BatchRenameStep],
) -> Vec<UnrestoredEntry> {
    let mut unrestored = Vec::new();
    let mut parked = pending
        .iter()
        .map(|step| (step.temporary.clone(), step.source.clone()))
        .collect::<Vec<_>>();
    for step in landed.iter().rev() {
        match park_source(filesystem, &step.target) {
            Ok(temporary) => parked.push((temporary, step.source.clone())),
            Err(error) => unrestored.push(UnrestoredEntry {
                original: step.source.clone(),
                current: step.target.clone(),
                error,
            }),
        }
    }
    unrestored.extend(restore_parked(filesystem, parked));
    unrestored
}

/// 放回原名同样不覆盖:原名在此期间被别处占用时,条目留在临时名上并报告。
fn restore_parked(
    filesystem: &mut impl EntryFilesystem,
    parked: impl IntoIterator<Item = (PathBuf, PathBuf)>,
) -> Vec<UnrestoredEntry> {
    parked
        .into_iter()
        .filter_map(|(temporary, original)| {
            let error = move_without_replacing(filesystem, &temporary, &original).err()?;
            Some(UnrestoredEntry {
                original,
                current: temporary,
                error,
            })
        })
        .collect()
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
