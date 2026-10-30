//! 批量创建符号链接：要么全部建好，要么删掉本次已建的链接回到操作前。

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use super::entry_creation::{create_at_first_available_name, preferred_directory_and_name};
use super::entry_gathering::EntryStepPacer;
use super::entry_moves::{LeftoverEntry, SystemEntryFilesystem};
use crate::entry_naming::EntryNameKind;
use crate::FileError;

/// `preferred_link` 被占用时按"名字 2"续编号；`name_kind` 决定续编号是否拆扩展名，
/// 取链接指向的对象的类型（指向目录的链接按目录名编号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicLinkRequest {
    pub preferred_link: PathBuf,
    pub target: PathBuf,
    pub name_kind: EntryNameKind,
}

/// 建、读、删符号链接，可注入。
pub(crate) trait LinkFilesystem {
    /// 链接路径已存在时返回 `AlreadyExists`，不覆盖。
    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()>;
    fn read_link(&mut self, link: &Path) -> io::Result<PathBuf>;
    fn remove_link(&mut self, link: &Path) -> io::Result<()>;
}

impl LinkFilesystem for SystemEntryFilesystem {
    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn read_link(&mut self, link: &Path) -> io::Result<PathBuf> {
        std::fs::read_link(link)
    }

    fn remove_link(&mut self, link: &Path) -> io::Result<()> {
        std::fs::remove_file(link)
    }
}

/// 返回实际建成的链接路径，与请求同序。
pub async fn create_symbolic_links(
    requests: Vec<SymbolicLinkRequest>,
    pacer: &mut impl EntryStepPacer,
) -> Result<Vec<PathBuf>, FileError> {
    create_symbolic_links_with(&mut SystemEntryFilesystem, &requests, pacer).await
}

pub(crate) async fn create_symbolic_links_with(
    filesystem: &mut impl LinkFilesystem,
    requests: &[SymbolicLinkRequest],
    pacer: &mut impl EntryStepPacer,
) -> Result<Vec<PathBuf>, FileError> {
    let preferred_names = requests
        .iter()
        .map(|request| preferred_directory_and_name(&request.preferred_link))
        .collect::<Result<Vec<_>, _>>()?;
    let mut created = Vec::with_capacity(requests.len());
    for (index, (request, (directory, name))) in requests.iter().zip(preferred_names).enumerate() {
        match create_link(filesystem, request, directory, name, pacer).await {
            Ok(link) => created.push(CreatedLink {
                link,
                target: &request.target,
            }),
            Err(cause) => return Err(remove_created_links(filesystem, &created, cause)),
        }
        pacer.step_finished(index + 1, requests.len()).await;
    }
    Ok(created.into_iter().map(|created| created.link).collect())
}

struct CreatedLink<'a> {
    link: PathBuf,
    target: &'a Path,
}

async fn create_link(
    filesystem: &mut impl LinkFilesystem,
    request: &SymbolicLinkRequest,
    directory: &Path,
    name: &OsStr,
    pacer: &mut impl EntryStepPacer,
) -> Result<PathBuf, FileError> {
    pacer.before_step().await?;
    create_at_first_available_name(
        directory,
        name,
        request.name_kind,
        |link| filesystem.symlink(&request.target, link),
        |link, source| FileError::CreateSymbolicLink {
            link,
            target: request.target.clone(),
            source,
        },
    )
}

fn remove_created_links(
    filesystem: &mut impl LinkFilesystem,
    created: &[CreatedLink<'_>],
    cause: FileError,
) -> FileError {
    let leftover_entries = created
        .iter()
        .rev()
        .filter_map(|created| {
            let error = remove_own_link(filesystem, created).err()?;
            Some(LeftoverEntry {
                path: created.link.clone(),
                error,
            })
        })
        .collect::<Vec<_>>();
    if leftover_entries.is_empty() {
        cause
    } else {
        FileError::EntryMovesIncomplete {
            cause: Box::new(cause),
            unrestored: Vec::new(),
            leftover_entries,
        }
    }
}

/// 只删仍指向本次目标的链接：这期间它若被换成了别的东西，删掉就是删别人的。
fn remove_own_link(
    filesystem: &mut impl LinkFilesystem,
    created: &CreatedLink<'_>,
) -> io::Result<()> {
    let current_target = filesystem.read_link(&created.link)?;
    if current_target != created.target {
        return Err(io::Error::other(format!(
            "the link now points to {current_target:?}"
        )));
    }
    filesystem.remove_link(&created.link)
}

#[cfg(test)]
#[path = "symbolic_links_tests.rs"]
mod tests;
