use std::borrow::Cow;

use iced::Task;

use super::FileBrowser;
use crate::commands::publish_desktop_notification_command;
use crate::config::UiLanguage;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::{sanitized_application_log_detail, Message};
use crate::operation_history::FileOperationCompletion;
use crate::operation_queue::{FileOperationTerminalStatus, QueuedFileOperation};

const NOTIFICATION_FILE_NAME_MAX_CHARS: usize = 96;
const NOTIFICATION_FAILURE_REASON_MAX_CHARS: usize = 160;

struct DesktopNotificationText {
    summary: String,
    body: String,
}

enum FileOperationNotificationCompletion<'a> {
    Completed,
    CompletedWithWarning(String),
    Failed(Cow<'a, str>),
}

impl FileBrowser {
    pub(super) fn file_operation_notification_command(
        &self,
        operation: &QueuedFileOperation,
        terminal_status: FileOperationTerminalStatus,
        completion: &FileOperationCompletion,
    ) -> Task<Message> {
        if self.system_focused_window.is_some() {
            return Task::none();
        }

        let notification_completion = match terminal_status {
            FileOperationTerminalStatus::Completed => {
                let warning = match completion {
                    FileOperationCompletion::Succeeded(outcome) => {
                        outcome.completion_warning(crate::localization::current_language())
                    }
                    _ => None,
                };
                match warning {
                    Some(warning) => {
                        FileOperationNotificationCompletion::CompletedWithWarning(warning)
                    }
                    None => FileOperationNotificationCompletion::Completed,
                }
            }
            FileOperationTerminalStatus::Failed => {
                let error = match completion {
                    FileOperationCompletion::Failed { error, .. }
                    | FileOperationCompletion::RecoveryInterrupted(error, _)
                    | FileOperationCompletion::RecoveryBlocked { error, .. } => {
                        Cow::Borrowed(error.as_str())
                    }
                    FileOperationCompletion::RenameRejected { reason, .. } => {
                        Cow::Owned(reason.message(self.active_language()))
                    }
                    FileOperationCompletion::Succeeded(_)
                    | FileOperationCompletion::Canceled(_) => {
                        unreachable!("failed queue status requires a failed operation completion")
                    }
                };
                FileOperationNotificationCompletion::Failed(error)
            }
            FileOperationTerminalStatus::Canceled => return Task::none(),
        };

        let Some(notification) = file_operation_notification_text(
            operation,
            notification_completion,
            self.active_language(),
        ) else {
            return Task::none();
        };

        publish_desktop_notification_command(notification.summary, notification.body)
    }

    pub(super) fn accept_desktop_notification_published(
        &mut self,
        outcome: Result<(), String>,
    ) -> Task<Message> {
        if let Err(error) = outcome {
            let log_error = sanitized_application_log_detail(&error);
            tracing::warn!(
                target: "app_ui::desktop_notifications",
                event = "desktop_notification_failed",
                error = %log_error,
                "desktop notification could not be published"
            );
        }
        Task::none()
    }
}

fn file_operation_notification_text(
    operation: &QueuedFileOperation,
    completion: FileOperationNotificationCompletion<'_>,
    language: UiLanguage,
) -> Option<DesktopNotificationText> {
    if !operation_supports_desktop_notification(operation) {
        return None;
    }

    let title = operation.title_in(language);
    let summary = match &completion {
        FileOperationNotificationCompletion::Completed
        | FileOperationNotificationCompletion::CompletedWithWarning(_) => {
            tr_in!(language, "%1 completed", title)
        }
        FileOperationNotificationCompletion::Failed(_) => tr_in!(language, "%1 failed", title),
    };
    let path_lines = operation.path_lines();
    let item_summary = if matches!(operation, QueuedFileOperation::EmptyTrash) {
        tr_in!(language, "Trash")
    } else if path_lines.total_items == 1 {
        format_middle_ellipsized_text(&path_lines.file_name, NOTIFICATION_FILE_NAME_MAX_CHARS)
    } else {
        tr_in!(language, "%1 items", path_lines.total_items)
    };
    let body = match completion {
        FileOperationNotificationCompletion::Completed => item_summary,
        FileOperationNotificationCompletion::CompletedWithWarning(warning) => format!(
            "{item_summary}\n{}",
            format_middle_ellipsized_text(&warning, NOTIFICATION_FAILURE_REASON_MAX_CHARS)
        ),
        FileOperationNotificationCompletion::Failed(error) => format!(
            "{item_summary}\n{}",
            bounded_notification_failure_reason(&error)
        ),
    };

    Some(DesktopNotificationText { summary, body })
}

fn operation_supports_desktop_notification(operation: &QueuedFileOperation) -> bool {
    match operation {
        QueuedFileOperation::BatchRename { .. }
        | QueuedFileOperation::Trash { .. }
        | QueuedFileOperation::Restore { .. }
        | QueuedFileOperation::DeleteTrashEntries { .. }
        | QueuedFileOperation::DeletePermanently { .. }
        | QueuedFileOperation::EmptyTrash
        | QueuedFileOperation::Copy { .. }
        | QueuedFileOperation::Duplicate { .. }
        | QueuedFileOperation::Move { .. }
        | QueuedFileOperation::GatherSelectionIntoNewFolder { .. }
        | QueuedFileOperation::UngatherNewFolder { .. }
        | QueuedFileOperation::CreateSymbolicLinks { .. }
        | QueuedFileOperation::CreateArchive { .. }
        | QueuedFileOperation::ExtractArchive { .. }
        | QueuedFileOperation::ExtractArchiveMembers { .. }
        | QueuedFileOperation::Convert { .. } => true,
        QueuedFileOperation::Rename { .. }
        | QueuedFileOperation::CreateDirectory { .. }
        | QueuedFileOperation::CreateDirectories { .. }
        | QueuedFileOperation::CreateEmptyFile { .. } => false,
    }
}

/// 同 KDE 作业通知：失败原因就是任务的完整错误，压成一行、太长才截断。
fn bounded_notification_failure_reason(error: &str) -> String {
    let reason = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.chars().count() <= NOTIFICATION_FAILURE_REASON_MAX_CHARS {
        return reason;
    }

    let mut bounded = reason
        .chars()
        .take(NOTIFICATION_FAILURE_REASON_MAX_CHARS - 1)
        .collect::<String>();
    bounded.push('…');
    bounded
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{
        ArchiveCompressionLevel, ArchiveExtractionRequest, ArchiveFormat, BatchRenameItem,
        FileOperationVerification, TrashRestoreEntry,
    };
    use iced::window;
    use iced_runtime::task::into_stream;

    use super::*;
    use crate::config;
    use crate::operation_history::FileOperationOutcome;
    use crate::operation_queue::QueuedTransfer;

    fn copy_operation(paths: &[(&str, &str)]) -> QueuedFileOperation {
        QueuedFileOperation::Copy {
            transfers: paths
                .iter()
                .map(|(source, target)| {
                    QueuedTransfer::new(PathBuf::from(source), PathBuf::from(target))
                })
                .collect(),
            verification: FileOperationVerification::default(),
        }
    }

    fn restore_entry() -> TrashRestoreEntry {
        TrashRestoreEntry::from_historical_paths(
            PathBuf::from("/trash/report.txt"),
            PathBuf::from("/trash/report.trashinfo"),
            PathBuf::from("/home/user/report.txt"),
        )
    }

    fn notifying_operations() -> Vec<QueuedFileOperation> {
        vec![
            QueuedFileOperation::BatchRename {
                items: vec![BatchRenameItem {
                    from: PathBuf::from("/tmp/old.txt"),
                    to: PathBuf::from("/tmp/new.txt"),
                }],
            },
            QueuedFileOperation::Trash {
                paths: vec![PathBuf::from("/tmp/report.txt")],
            },
            QueuedFileOperation::Restore {
                entries: vec![restore_entry()],
            },
            QueuedFileOperation::DeleteTrashEntries {
                entries: vec![restore_entry()],
            },
            QueuedFileOperation::DeletePermanently {
                paths: vec![PathBuf::from("/tmp/report.txt")],
            },
            QueuedFileOperation::EmptyTrash,
            copy_operation(&[("/tmp/report.txt", "/var/tmp/report.txt")]),
            QueuedFileOperation::Move {
                transfers: vec![QueuedTransfer::new(
                    PathBuf::from("/tmp/report.txt"),
                    PathBuf::from("/var/tmp/report.txt"),
                )],
                verification: FileOperationVerification::default(),
            },
            QueuedFileOperation::CreateArchive {
                sources: vec![PathBuf::from("/tmp/report.txt")],
                target: PathBuf::from("/tmp/report.zip"),
                format: ArchiveFormat::Zip,
                compression_level: ArchiveCompressionLevel::Balanced,
                password: None,
            },
            QueuedFileOperation::ExtractArchive {
                request: ArchiveExtractionRequest {
                    archive: PathBuf::from("/tmp/report.zip"),
                    destination: PathBuf::from("/tmp/report"),
                    password: None,
                },
            },
        ]
    }

    #[test]
    fn only_potentially_long_operations_build_notifications() {
        for operation in notifying_operations() {
            assert!(file_operation_notification_text(
                &operation,
                FileOperationNotificationCompletion::Completed,
                UiLanguage::English,
            )
            .is_some());
        }

        for operation in [
            QueuedFileOperation::Rename {
                path: PathBuf::from("/tmp/old.txt"),
                new_name: "new.txt".into(),
            },
            QueuedFileOperation::CreateDirectory {
                parent: PathBuf::from("/tmp"),
            },
            QueuedFileOperation::CreateEmptyFile {
                parent: PathBuf::from("/tmp"),
            },
        ] {
            assert!(file_operation_notification_text(
                &operation,
                FileOperationNotificationCompletion::Completed,
                UiLanguage::English,
            )
            .is_none());
        }
    }

    #[test]
    fn notification_text_uses_current_language_and_item_count() {
        let operation = copy_operation(&[
            ("/tmp/first.txt", "/var/tmp/first.txt"),
            ("/tmp/second.txt", "/var/tmp/second.txt"),
        ]);

        let english = file_operation_notification_text(
            &operation,
            FileOperationNotificationCompletion::Completed,
            UiLanguage::English,
        )
        .expect("English notification");
        let chinese = file_operation_notification_text(
            &operation,
            FileOperationNotificationCompletion::Completed,
            UiLanguage::Chinese,
        )
        .expect("Chinese notification");

        assert_eq!(english.summary, "Copy completed");
        assert_eq!(english.body, "2 items");
        assert_eq!(chinese.summary, "复制已完成");
        assert_eq!(chinese.body, "2 个项目");

        let empty_trash = file_operation_notification_text(
            &QueuedFileOperation::EmptyTrash,
            FileOperationNotificationCompletion::Completed,
            UiLanguage::Chinese,
        )
        .expect("empty trash notification");
        assert_eq!(empty_trash.body, "回收站");
    }

    #[test]
    fn failed_notification_shows_the_whole_error_on_one_line() {
        let operation = copy_operation(&[("/home/user/report.txt", "/tmp/report.txt")]);
        let error = "could not copy \"/home/user/report.txt\" to \"/tmp/report.txt\":\n  Permission denied (os error 13)";
        let english = file_operation_notification_text(
            &operation,
            FileOperationNotificationCompletion::Failed(error.into()),
            UiLanguage::English,
        )
        .expect("failed notification");
        let chinese = file_operation_notification_text(
            &operation,
            FileOperationNotificationCompletion::Failed(error.into()),
            UiLanguage::Chinese,
        )
        .expect("failed notification");

        let reason = "could not copy \"/home/user/report.txt\" to \"/tmp/report.txt\": Permission denied (os error 13)";
        assert_eq!(english.summary, "Copy failed");
        assert_eq!(english.body, format!("report.txt\n{reason}"));
        assert_eq!(chinese.summary, "复制失败");
        assert_eq!(chinese.body, format!("report.txt\n{reason}"));
    }

    #[test]
    fn failure_reason_bound_is_unicode_safe() {
        let reason = "界".repeat(NOTIFICATION_FAILURE_REASON_MAX_CHARS + 10);
        let bounded = bounded_notification_failure_reason(&reason);

        assert_eq!(
            bounded.chars().count(),
            NOTIFICATION_FAILURE_REASON_MAX_CHARS
        );
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn notification_command_requires_application_to_be_unfocused_and_terminal_status() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let operation = copy_operation(&[("/tmp/report.txt", "/var/tmp/report.txt")]);
        let completion = FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory);

        assert!(into_stream(browser.file_operation_notification_command(
            &operation,
            FileOperationTerminalStatus::Completed,
            &completion,
        ))
        .is_some());

        for focused_window in [browser.main_window, window::Id::unique()] {
            browser.system_focused_window = Some(focused_window);
            assert!(into_stream(browser.file_operation_notification_command(
                &operation,
                FileOperationTerminalStatus::Completed,
                &completion,
            ))
            .is_none());
        }

        browser.system_focused_window = None;
        assert!(into_stream(browser.file_operation_notification_command(
            &operation,
            FileOperationTerminalStatus::Canceled,
            &completion,
        ))
        .is_none());
    }

    #[test]
    fn notification_publish_failure_does_not_replace_global_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.show_global_error("existing error");

        drop(
            browser
                .accept_desktop_notification_published(Err("notify-send unavailable".to_owned())),
        );

        assert_eq!(browser.current_error(), Some("existing error"));
    }
}
