//! 收纳、高级新建、撤销收纳的任务适配。全有或全无与回滚由 file-core 负责，
//! 这里只接上暂停/取消与进度，并把结果映射成任务结果。

use std::path::PathBuf;

use file_core::{
    create_directory_tree, gather_entries_into_new_directory, restore_gathered_entries, EntryMove,
    FileOperationControls,
};
use iced::futures::channel::mpsc::Sender as IcedSender;

use super::step_pacer::QueuedStepPacer;
use crate::model::Message;
use crate::operation_history::{CompletedTransfer, FileOperationOutcome, KeptNewFolder};

pub(super) async fn run_queued_create_directories(
    parent: PathBuf,
    names: Vec<String>,
    gather_sources: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let mut pacer = QueuedStepPacer::new(&mut controls, output, task_id);
    let tree = create_directory_tree(parent.clone(), names.clone(), gather_sources, &mut pacer)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::CreateDirectories {
        parent,
        leaves: names,
        leaf_paths: tree.leaves,
        created: tree.created,
        moved: completed_transfers(tree.moved),
    })
}

pub(super) async fn run_queued_gather_selection_into_new_folder(
    directory: PathBuf,
    sources: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let mut pacer = QueuedStepPacer::new(&mut controls, output, task_id);
    let gathered = gather_entries_into_new_directory(directory, sources, &mut pacer)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::GatheredIntoNewFolder {
        directory: gathered.directory,
        moved: completed_transfers(gathered.moved),
    })
}

pub(super) async fn run_queued_ungather_new_folder(
    directory: PathBuf,
    restore_targets: Vec<PathBuf>,
    created_directories: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let mut pacer = QueuedStepPacer::new(&mut controls, output, task_id);
    let restored =
        restore_gathered_entries(directory, restore_targets, created_directories, &mut pacer)
            .await
            .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::NewFolderUngathered {
        kept_directories: restored
            .kept_directories
            .into_iter()
            .map(|kept| KeptNewFolder {
                path: kept.path,
                reason: kept.error.to_string(),
            })
            .collect(),
    })
}

fn completed_transfers(moves: Vec<EntryMove>) -> Vec<CompletedTransfer> {
    moves
        .into_iter()
        .map(|entry_move| CompletedTransfer {
            source: entry_move.from,
            target: entry_move.to,
        })
        .collect()
}
