//! 批量创建符号链接：要么全部建好，要么删掉本次已建的链接回到操作前。

use std::io;
use std::path::{Path, PathBuf};

use super::entry_gathering::EntryStepPacer;
use super::entry_moves::{LeftoverEntry, SystemEntryFilesystem};
use crate::FileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicLinkRequest {
    pub link: PathBuf,
    pub target: PathBuf,
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

pub async fn create_symbolic_links(
    requests: Vec<SymbolicLinkRequest>,
    pacer: &mut impl EntryStepPacer,
) -> Result<(), FileError> {
    create_symbolic_links_with(&mut SystemEntryFilesystem, &requests, pacer).await
}

pub(crate) async fn create_symbolic_links_with(
    filesystem: &mut impl LinkFilesystem,
    requests: &[SymbolicLinkRequest],
    pacer: &mut impl EntryStepPacer,
) -> Result<(), FileError> {
    for (index, request) in requests.iter().enumerate() {
        if let Err(cause) = create_link(filesystem, request, pacer).await {
            return Err(remove_created_links(filesystem, &requests[..index], cause));
        }
        pacer.step_finished(index + 1, requests.len()).await;
    }
    Ok(())
}

async fn create_link(
    filesystem: &mut impl LinkFilesystem,
    request: &SymbolicLinkRequest,
    pacer: &mut impl EntryStepPacer,
) -> Result<(), FileError> {
    pacer.before_step().await?;
    filesystem
        .symlink(&request.target, &request.link)
        .map_err(|source| FileError::CreateSymbolicLink {
            link: request.link.clone(),
            target: request.target.clone(),
            source,
        })
}

fn remove_created_links(
    filesystem: &mut impl LinkFilesystem,
    created: &[SymbolicLinkRequest],
    cause: FileError,
) -> FileError {
    let leftover_entries = created
        .iter()
        .rev()
        .filter_map(|request| {
            let error = remove_own_link(filesystem, request).err()?;
            Some(LeftoverEntry {
                path: request.link.clone(),
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
    request: &SymbolicLinkRequest,
) -> io::Result<()> {
    let current_target = filesystem.read_link(&request.link)?;
    if current_target != request.target {
        return Err(io::Error::other(format!(
            "the link now points to {current_target:?}"
        )));
    }
    filesystem.remove_link(&request.link)
}

#[cfg(test)]
#[path = "symbolic_links_tests.rs"]
mod tests;
