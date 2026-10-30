//! 收纳到新文件夹、高级新建（可连带移入）、撤销收纳：多个条目挪进或挪出新建目录，
//! 要么全部完成，要么回到操作前。中途失败或取消时逆序挪回已挪的条目、删掉本次新建的
//! 目录；回不去的条目与删不掉的目录逐个报告。

use std::ffi::OsStr;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use super::entry_moves::{
    move_without_replacing, EntryTreeFilesystem, LeftoverEntry, SystemEntryFilesystem,
    UnrestoredEntry,
};
use crate::entry_naming::{
    entry_name_candidates, validate_entry_name, EntryNameKind, InvalidEntryName,
};
use crate::FileError;

/// 多条目操作每一步前后与调用方的协作。`before_step` 等暂停结束，遇取消返回错误；
/// 返回错误即停止并回滚，与中途失败同样处理。`step_finished` 上报进度。
pub trait EntryStepPacer: Send {
    fn before_step(&mut self) -> impl Future<Output = Result<(), FileError>> + Send;
    fn step_finished(&mut self, completed: usize, total: usize) -> impl Future<Output = ()> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug)]
pub struct GatheredEntries {
    pub directory: PathBuf,
    pub moved: Vec<EntryMove>,
}

#[derive(Debug)]
pub struct CreatedDirectoryTree {
    /// 每个请求的叶子实际建成的目录（重名时续编号），与请求同序。
    pub leaves: Vec<PathBuf>,
    /// 本次新建的全部目录（含中间段），按创建顺序；复用的已有目录不在其中。
    pub created: Vec<PathBuf>,
    pub moved: Vec<EntryMove>,
}

#[derive(Debug)]
pub struct RestoredGatheredEntries {
    /// 条目都已挪回、但没能删掉的新建目录（多半是用户往里放了别的内容）。
    pub kept_directories: Vec<LeftoverEntry>,
}

/// 在 `preferred` 处（被占用则按"名字 2"续编号）新建目录，把 `sources` 逐个挪进去。
pub async fn gather_entries_into_new_directory(
    preferred: PathBuf,
    sources: Vec<PathBuf>,
    pacer: &mut impl EntryStepPacer,
) -> Result<GatheredEntries, FileError> {
    gather_entries_with(&mut SystemEntryFilesystem, &preferred, &sources, pacer).await
}

/// 按 `leaves`（相对 `parent`，段间 `/` 表示嵌套）建目录：中间段已存在则复用，叶子段
/// 被占用则续编号；再把 `gather_sources` 挪进第一个叶子实际建成的目录。
pub async fn create_directory_tree(
    parent: PathBuf,
    leaves: Vec<String>,
    gather_sources: Vec<PathBuf>,
    pacer: &mut impl EntryStepPacer,
) -> Result<CreatedDirectoryTree, FileError> {
    create_directory_tree_with(
        &mut SystemEntryFilesystem,
        &parent,
        &leaves,
        &gather_sources,
        pacer,
    )
    .await
}

/// 撤销收纳：把 `directory` 里的条目挪回 `restore_targets`，再逆序删掉
/// `created_directories`（按创建顺序给出）中已经空了的目录。
pub async fn restore_gathered_entries(
    directory: PathBuf,
    restore_targets: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    pacer: &mut impl EntryStepPacer,
) -> Result<RestoredGatheredEntries, FileError> {
    restore_gathered_entries_with(
        &mut SystemEntryFilesystem,
        &directory,
        &restore_targets,
        &created_directories,
        pacer,
    )
    .await
}

pub(crate) async fn gather_entries_with(
    filesystem: &mut impl EntryTreeFilesystem,
    preferred: &Path,
    sources: &[PathBuf],
    pacer: &mut impl EntryStepPacer,
) -> Result<GatheredEntries, FileError> {
    let (Some(parent), Some(name)) = (preferred.parent(), preferred.file_name()) else {
        return Err(invalid_input(preferred, "new folder path has no parent"));
    };
    validate_name(name)?;
    let names = entry_file_names(sources)?;

    pacer.before_step().await?;
    let directory = create_directory_at_available_name(filesystem, parent, name)?;
    let mut changes = AppliedChanges {
        created: vec![directory.clone()],
        moved: Vec::new(),
    };
    let moves = moves_into(&directory, sources, &names);
    if let Err(cause) = apply_moves(filesystem, &moves, &mut changes, pacer, 0, moves.len()).await {
        return Err(changes.roll_back(filesystem, cause));
    }
    Ok(GatheredEntries {
        directory,
        moved: changes.moved,
    })
}

pub(crate) async fn create_directory_tree_with(
    filesystem: &mut impl EntryTreeFilesystem,
    parent: &Path,
    leaves: &[String],
    gather_sources: &[PathBuf],
    pacer: &mut impl EntryStepPacer,
) -> Result<CreatedDirectoryTree, FileError> {
    let leaf_segments = leaves
        .iter()
        .map(|leaf| directory_segments(leaf))
        .collect::<Result<Vec<_>, _>>()?;
    let names = entry_file_names(gather_sources)?;
    if leaves.is_empty() && !gather_sources.is_empty() {
        return Err(invalid_input(parent, "items to move need a new folder"));
    }

    let total = leaves.len() + gather_sources.len();
    let mut changes = AppliedChanges::default();
    let mut leaf_paths = Vec::with_capacity(leaves.len());
    let applied = async {
        for (index, segments) in leaf_segments.iter().enumerate() {
            pacer.before_step().await?;
            leaf_paths.push(create_directory_chain(
                filesystem,
                parent,
                segments,
                &mut changes.created,
            )?);
            pacer.step_finished(index + 1, total).await;
        }
        if let Some(destination) = leaf_paths.first() {
            let moves = moves_into(destination, gather_sources, &names);
            apply_moves(filesystem, &moves, &mut changes, pacer, leaves.len(), total).await?;
        }
        Ok::<(), FileError>(())
    }
    .await;
    if let Err(cause) = applied {
        return Err(changes.roll_back(filesystem, cause));
    }
    Ok(CreatedDirectoryTree {
        leaves: leaf_paths,
        created: changes.created,
        moved: changes.moved,
    })
}

pub(crate) async fn restore_gathered_entries_with(
    filesystem: &mut impl EntryTreeFilesystem,
    directory: &Path,
    restore_targets: &[PathBuf],
    created_directories: &[PathBuf],
    pacer: &mut impl EntryStepPacer,
) -> Result<RestoredGatheredEntries, FileError> {
    let names = entry_file_names(restore_targets)?;
    let moves = names
        .iter()
        .zip(restore_targets)
        .map(|(name, target)| EntryMove {
            from: directory.join(name),
            to: target.clone(),
        })
        .collect::<Vec<_>>();
    // 失败时只把已挪回的条目再挪进目录；目录本来就在，不删。
    let mut changes = AppliedChanges::default();
    if let Err(cause) = apply_moves(filesystem, &moves, &mut changes, pacer, 0, moves.len()).await {
        return Err(changes.roll_back(filesystem, cause));
    }

    let kept_directories = created_directories
        .iter()
        .rev()
        .filter_map(|created| {
            let error = filesystem.remove_dir(created).err()?;
            Some(LeftoverEntry {
                path: created.clone(),
                error,
            })
        })
        .collect();
    Ok(RestoredGatheredEntries { kept_directories })
}

/// 本次操作已经做出的改动，失败时按相反顺序撤回。
#[derive(Default)]
struct AppliedChanges {
    created: Vec<PathBuf>,
    moved: Vec<EntryMove>,
}

impl AppliedChanges {
    fn roll_back(self, filesystem: &mut impl EntryTreeFilesystem, cause: FileError) -> FileError {
        let mut unrestored = Vec::new();
        for entry_move in self.moved.iter().rev() {
            if let Err(error) = move_without_replacing(filesystem, &entry_move.to, &entry_move.from)
            {
                unrestored.push(UnrestoredEntry {
                    original: entry_move.from.clone(),
                    current: entry_move.to.clone(),
                    error,
                });
            }
        }
        let mut leftover_entries = Vec::new();
        for created in self.created.iter().rev() {
            // 里面还留着没挪回去的条目：目录只能留下，条目的位置已经在 unrestored 里。
            if unrestored
                .iter()
                .any(|entry| entry.current.starts_with(created))
            {
                continue;
            }
            if let Err(error) = filesystem.remove_dir(created) {
                leftover_entries.push(LeftoverEntry {
                    path: created.clone(),
                    error,
                });
            }
        }
        if unrestored.is_empty() && leftover_entries.is_empty() {
            cause
        } else {
            FileError::EntryMovesIncomplete {
                cause: Box::new(cause),
                unrestored,
                leftover_entries,
            }
        }
    }
}

async fn apply_moves(
    filesystem: &mut impl EntryTreeFilesystem,
    moves: &[EntryMove],
    changes: &mut AppliedChanges,
    pacer: &mut impl EntryStepPacer,
    completed_before: usize,
    total: usize,
) -> Result<(), FileError> {
    for (offset, entry_move) in moves.iter().enumerate() {
        pacer.before_step().await?;
        move_without_replacing(filesystem, &entry_move.from, &entry_move.to)?;
        changes.moved.push(entry_move.clone());
        pacer
            .step_finished(completed_before + offset + 1, total)
            .await;
    }
    Ok(())
}

fn moves_into(directory: &Path, sources: &[PathBuf], names: &[&OsStr]) -> Vec<EntryMove> {
    sources
        .iter()
        .zip(names)
        .map(|(source, name)| EntryMove {
            from: source.clone(),
            to: directory.join(name),
        })
        .collect()
}

/// 名字已经校验过。候选上逐个原子创建，`AlreadyExists` 就换下一个，不先查再建。
fn create_directory_at_available_name(
    filesystem: &mut impl EntryTreeFilesystem,
    parent: &Path,
    name: &OsStr,
) -> Result<PathBuf, FileError> {
    for candidate in entry_name_candidates(name, EntryNameKind::Directory) {
        let path = parent.join(candidate);
        match filesystem.create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(FileError::CreateDirectory { path, source }),
        }
    }
    Err(FileError::EntryNameCandidatesExhausted {
        directory: parent.to_path_buf(),
        base: name.to_os_string(),
    })
}

/// 建一条嵌套链，新建的目录记进 `created`，返回叶子实际建成的路径。
fn create_directory_chain(
    filesystem: &mut impl EntryTreeFilesystem,
    parent: &Path,
    segments: &[&str],
    created: &mut Vec<PathBuf>,
) -> Result<PathBuf, FileError> {
    let (leaf, intermediates) = segments
        .split_last()
        .expect("directory_segments returns at least one segment");
    let mut current = parent.to_path_buf();
    for segment in intermediates {
        let next = current.join(segment);
        match filesystem.create_dir(&next) {
            Ok(()) => created.push(next.clone()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match filesystem.is_directory(&next) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(FileError::CreateDirectory {
                            path: next,
                            source: io::Error::new(
                                io::ErrorKind::NotADirectory,
                                "exists and is not a directory",
                            ),
                        })
                    }
                    Err(source) => return Err(FileError::Metadata { path: next, source }),
                }
            }
            Err(source) => return Err(FileError::CreateDirectory { path: next, source }),
        }
        current = next;
    }
    let leaf_path = create_directory_at_available_name(filesystem, &current, OsStr::new(leaf))?;
    created.push(leaf_path.clone());
    Ok(leaf_path)
}

/// 叶子路径按 `/` 拆段，去掉段两端空白与空段（界面输入的约定）；每段都必须是合法名字。
fn directory_segments(leaf: &str) -> Result<Vec<&str>, FileError> {
    let segments = leaf
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Err(FileError::InvalidEntryName {
            name: leaf.into(),
            reason: InvalidEntryName::Empty,
        });
    }
    for segment in &segments {
        validate_name(OsStr::new(segment))?;
    }
    Ok(segments)
}

fn entry_file_names(paths: &[PathBuf]) -> Result<Vec<&OsStr>, FileError> {
    paths
        .iter()
        .map(|path| {
            path.file_name()
                .ok_or_else(|| invalid_input(path, "path has no file name"))
        })
        .collect()
}

fn validate_name(name: &OsStr) -> Result<(), FileError> {
    validate_entry_name(name).map_err(|reason| FileError::InvalidEntryName {
        name: name.to_os_string(),
        reason,
    })
}

fn invalid_input(path: &Path, message: &str) -> FileError {
    FileError::InvalidInput {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "entry_gathering_tests.rs"]
mod tests;
