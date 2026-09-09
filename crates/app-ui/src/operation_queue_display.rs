use std::path::{Path, PathBuf};

use file_core::TrashRestoreEntry;

use crate::config::UiLanguage;
use crate::formatting::format_file_size;
use crate::operation_queue::{
    FileOperationStatus, FileOperationTask, QueuedFileOperation, QueuedTransfer,
    NEW_DIRECTORY_NAME, NEW_FILE_NAME,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileOperationPathLines {
    pub(crate) file_name: String,
    pub(crate) original_path: String,
    pub(crate) directory_path: String,
    pub(crate) total_items: usize,
}

impl QueuedFileOperation {
    pub(crate) fn path_lines(&self) -> FileOperationPathLines {
        match self {
            Self::Rename { path, new_name } => FileOperationPathLines {
                file_name: new_name.clone(),
                original_path: path_full_text(path),
                directory_path: path_parent_text(path),
                total_items: 1,
            },
            Self::BatchRename { items } => match items.as_slice() {
                [] => FileOperationPathLines::empty(),
                [item, ..] => FileOperationPathLines::from_paths(
                    &item.to,
                    &item.from,
                    parent_path(&item.to),
                    items.len(),
                ),
            },
            Self::CreateDirectory { parent } => {
                created_entry_path_lines(parent, NEW_DIRECTORY_NAME)
            }
            Self::CreateEmptyFile { parent } => created_entry_path_lines(parent, NEW_FILE_NAME),
            Self::Trash { paths } | Self::DeletePermanently { paths } => {
                path_lines_from_paths(paths)
            }
            Self::Restore { entries } | Self::DeleteTrashEntries { entries } => {
                path_lines_from_trash_originals(entries)
            }
            Self::EmptyTrash => FileOperationPathLines {
                file_name: "Trash".to_owned(),
                original_path: "Trash".to_owned(),
                directory_path: "Trash".to_owned(),
                total_items: 1,
            },
            Self::Copy { transfers, .. }
            | Self::Duplicate { transfers, .. }
            | Self::Move { transfers, .. } => path_lines_from_transfers(transfers),
            Self::GatherSelectionIntoNewFolder { directory, sources } => {
                path_lines_from_gather(directory, sources)
            }
            Self::UngatherNewFolder {
                directory,
                restore_targets,
            } => path_lines_from_gather(directory, restore_targets),
            Self::CreateSymbolicLinks { links } => {
                let link_paths = links
                    .iter()
                    .map(|link| link.link_path.clone())
                    .collect::<Vec<_>>();
                path_lines_from_paths(&link_paths)
            }
            Self::CreateArchive {
                sources, target, ..
            } => path_lines_from_archive(sources, target),
            Self::Convert { requests } => {
                let sources = requests
                    .iter()
                    .map(|request| request.source.clone())
                    .collect::<Vec<_>>();
                path_lines_from_paths(&sources)
            }
            Self::ExtractArchive { request } => {
                path_lines_from_extracted_archive(&request.archive, &request.destination)
            }
        }
    }
}

impl FileOperationPathLines {
    fn from_paths(
        file_name_path: &Path,
        original_path: &Path,
        directory_path: &Path,
        total_items: usize,
    ) -> Self {
        Self {
            file_name: path_label(file_name_path),
            original_path: path_full_text(original_path),
            directory_path: path_full_text(directory_path),
            total_items,
        }
    }

    fn empty() -> Self {
        Self {
            file_name: "No items".to_owned(),
            original_path: "No items".to_owned(),
            directory_path: "No items".to_owned(),
            total_items: 0,
        }
    }
}

pub(crate) fn file_operation_progress_text(
    task: &FileOperationTask,
    language: UiLanguage,
) -> Option<String> {
    let byte_detail = task.progress.bytes().map(|(completed_bytes, total_bytes)| {
        format!(
            "{} / {}",
            format_file_size(completed_bytes),
            format_file_size(total_bytes)
        )
    });
    let item_detail = task.progress.items().map(|(completed_items, total_items)| {
        if language == UiLanguage::Chinese {
            format!("{completed_items} / {total_items} 项")
        } else {
            format!("{completed_items} / {total_items} items")
        }
    });

    match (byte_detail, item_detail) {
        (Some(bytes), Some(items)) => Some(format!("{bytes} | {items}")),
        (Some(bytes), None) => Some(bytes),
        (None, Some(items)) => Some(items),
        (None, None)
            if matches!(
                task.status,
                FileOperationStatus::Running | FileOperationStatus::Canceling
            ) =>
        {
            Some(crate::localization::translate(language, "Processing...").into_owned())
        }
        (None, None) => None,
    }
}

pub(crate) fn file_operation_copy_text(task: &FileOperationTask, language: UiLanguage) -> String {
    let paths = task.operation.path_lines();
    let translate = |text| crate::localization::translate(language, text).into_owned();
    let progress = file_operation_progress_text(task, language).unwrap_or_else(|| {
        task.progress
            .fraction()
            .map(|fraction| format!("{:.0}%", fraction * 100.0))
            .unwrap_or_else(|| translate("Indeterminate"))
    });
    let mut lines = vec![
        format!(
            "{}: {}",
            translate("Task"),
            translate(task.operation.title())
        ),
        format!("{}: {}", translate("File name"), paths.file_name),
        format!("{}: {}", translate("Items"), paths.total_items),
        format!("{}: {}", translate("Original"), paths.original_path),
        format!("{}: {}", translate("Directory"), paths.directory_path),
        format!(
            "{}: {}",
            translate("Status"),
            translate(task.status_label())
        ),
        format!("{}: {progress}", translate("Progress")),
    ];
    if let Some(warning) = task.completion_warning.as_deref() {
        lines.push(format!(
            "{}: {}",
            translate("Warning"),
            crate::localization::trash_tracking_warning(language, warning)
        ));
    }
    if let Some(error) = task.error.as_deref() {
        lines.push(format!("{}: {}", translate("Error"), translate(error)));
    }
    lines.join("\n")
}

fn created_entry_path_lines(parent: &Path, name: &str) -> FileOperationPathLines {
    let target = parent.join(name);
    FileOperationPathLines::from_paths(&target, &target, parent, 1)
}

fn path_lines_from_paths(paths: &[PathBuf]) -> FileOperationPathLines {
    match paths {
        [] => FileOperationPathLines::empty(),
        [path, ..] => {
            FileOperationPathLines::from_paths(path, path, parent_path(path), paths.len())
        }
    }
}

fn path_lines_from_transfers(transfers: &[QueuedTransfer]) -> FileOperationPathLines {
    match transfers {
        [] => FileOperationPathLines::empty(),
        [transfer, ..] => FileOperationPathLines::from_paths(
            &transfer.target,
            &transfer.source,
            parent_path(&transfer.target),
            transfers.len(),
        ),
    }
}

fn path_lines_from_gather(directory: &Path, entry_paths: &[PathBuf]) -> FileOperationPathLines {
    match entry_paths {
        [] => FileOperationPathLines::from_paths(directory, directory, parent_path(directory), 1),
        [first, ..] => FileOperationPathLines::from_paths(
            directory,
            first,
            parent_path(directory),
            entry_paths.len(),
        ),
    }
}

fn path_lines_from_archive(sources: &[PathBuf], target: &Path) -> FileOperationPathLines {
    match sources {
        [] => FileOperationPathLines::from_paths(target, target, parent_path(target), 1),
        [source, ..] => {
            FileOperationPathLines::from_paths(target, source, parent_path(target), sources.len())
        }
    }
}

fn path_lines_from_extracted_archive(archive: &Path, destination: &Path) -> FileOperationPathLines {
    FileOperationPathLines::from_paths(destination, archive, parent_path(destination), 1)
}

fn path_lines_from_trash_originals(entries: &[TrashRestoreEntry]) -> FileOperationPathLines {
    match entries {
        [] => FileOperationPathLines::empty(),
        [entry, ..] => FileOperationPathLines::from_paths(
            &entry.original_path,
            &entry.original_path,
            parent_path(&entry.original_path),
            entries.len(),
        ),
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn path_full_text(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn parent_path(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn path_parent_text(path: &Path) -> String {
    path_full_text(parent_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_core::FileOperationVerification;

    #[test]
    fn transfer_path_lines_expose_name_original_and_directory() {
        let operation = QueuedFileOperation::Copy {
            transfers: vec![QueuedTransfer::new(
                PathBuf::from("/home/user/report.txt"),
                PathBuf::from("/tmp/report.txt"),
            )],
            verification: FileOperationVerification::default(),
        };

        let path_lines = operation.path_lines();

        assert_eq!(path_lines.file_name, "report.txt");
        assert_eq!(path_lines.original_path, "/home/user/report.txt");
        assert_eq!(path_lines.directory_path, "/tmp");
        assert_eq!(path_lines.total_items, 1);
    }

    #[test]
    fn copied_task_details_keep_full_paths_status_progress_and_error() {
        let long_parent = PathBuf::from(
            "/workspace/destination/another-very-long-directory-name-that-must-remain-complete",
        );
        let mut queue = crate::operation_queue::FileOperationQueue::new();
        assert!(queue
            .enqueue(QueuedFileOperation::CreateDirectory {
                parent: long_parent.clone(),
            })
            .error()
            .is_none());
        let task_id = queue.tasks()[0].id;
        let full_error =
            "a complete diagnostic that is intentionally longer than the task row error limit"
                .repeat(3);
        assert_eq!(
            queue
                .finish(
                    task_id,
                    crate::operation_queue::FileOperationFinish::Failed(full_error.clone()),
                )
                .0,
            Some(crate::operation_queue::FileOperationTerminalStatus::Failed)
        );

        let copied = file_operation_copy_text(&queue.tasks()[0], UiLanguage::English);

        assert!(copied.contains(long_parent.to_string_lossy().as_ref()));
        assert!(copied.contains("Status: Failed"));
        assert!(copied.contains("Progress: Indeterminate"));
        assert!(copied.contains(&full_error));
        assert!(!copied.contains('…'));
    }
}
