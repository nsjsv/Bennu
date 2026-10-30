//! 收纳、高级新建、撤销收纳的任务适配。全有或全无与回滚由 file-core 负责，
//! 这里只接上暂停/取消与进度，并把结果映射成任务结果。

use std::future::Future;
use std::path::PathBuf;

use file_core::{
    create_directory_tree, gather_entries_into_new_directory, restore_gathered_entries, EntryMove,
    EntryStepPacer, FileError, FileOperationControls,
};
use iced::futures::channel::mpsc::Sender as IcedSender;

use super::send_file_operation_progress;
use crate::model::Message;
use crate::operation_history::{CompletedTransfer, FileOperationOutcome, KeptNewFolder};
use crate::operation_progress::FileOperationProgressUpdate;

/// 每一步前等暂停结束、遇取消报错；每一步后把条目进度发给任务行。
struct QueuedStepPacer<'a> {
    controls: &'a mut FileOperationControls,
    output: &'a mut IcedSender<Message>,
    task_id: u64,
}

impl EntryStepPacer for QueuedStepPacer<'_> {
    fn before_step(&mut self) -> impl Future<Output = Result<(), FileError>> + Send {
        self.controls.wait_until_running()
    }

    fn step_finished(&mut self, completed: usize, total: usize) -> impl Future<Output = ()> + Send {
        send_file_operation_progress(
            self.output,
            self.task_id,
            FileOperationProgressUpdate::IndeterminateItems { completed, total },
        )
    }
}

pub(super) async fn run_queued_create_directories(
    parent: PathBuf,
    names: Vec<String>,
    gather_sources: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let mut pacer = QueuedStepPacer {
        controls: &mut controls,
        output,
        task_id,
    };
    let tree = create_directory_tree(parent.clone(), names.clone(), gather_sources, &mut pacer)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::CreateDirectories {
        parent,
        leaves: names,
        top_level: top_level_directories(&tree.created),
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
    let mut pacer = QueuedStepPacer {
        controls: &mut controls,
        output,
        task_id,
    };
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
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let mut pacer = QueuedStepPacer {
        controls: &mut controls,
        output,
        task_id,
    };
    // 收纳只新建了这一个目录。
    let created_directories = vec![directory.clone()];
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

/// 撤销回收集:新建目录里去掉是另一个新建目录后代的项,
/// 让顶层目录整棵进回收站,不留下空壳中间层。
fn top_level_directories(created: &[PathBuf]) -> Vec<PathBuf> {
    created
        .iter()
        .filter(|path| {
            !created
                .iter()
                .any(|other| *other != **path && path.strip_prefix(other).is_ok())
        })
        .cloned()
        .collect()
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
