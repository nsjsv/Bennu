use bennu_localization::{UiLanguage, UnrestoredRenameText};
use file_core::{batch_rename_paths, BatchRenameItem, FileError, FileOperationControls};

use crate::operation_history::FileOperationOutcome;

pub(super) async fn run_queued_batch_rename(
    items: Vec<BatchRenameItem>,
    mut controls: FileOperationControls,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    let renames = batch_rename_paths(items).await.map_err(|error| {
        batch_rename_failure_text(&error, crate::localization::current_language())
    })?;
    Ok(FileOperationOutcome::BatchRename { renames })
}

/// 回滚没能恢复原名的条目必须逐个告诉用户它们现在在哪里,否则停在隐藏
/// 临时名上的文件在默认视图里等于"消失";其余失败沿用错误原文。
fn batch_rename_failure_text(error: &FileError, language: UiLanguage) -> String {
    let FileError::EntryMovesIncomplete { cause, unrestored } = error else {
        return error.to_string();
    };
    let unrestored = unrestored
        .iter()
        .map(|item| UnrestoredRenameText {
            original: item.original.display().to_string(),
            current: item.current.display().to_string(),
            reason: item.error.to_string(),
        })
        .collect::<Vec<_>>();
    bennu_localization::batch_rename_incomplete(language, &cause.to_string(), &unrestored)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use file_core::UnrestoredEntry;

    use super::*;

    fn incomplete_error() -> FileError {
        FileError::EntryMovesIncomplete {
            cause: Box::new(FileError::Cancelled),
            unrestored: vec![UnrestoredEntry {
                original: PathBuf::from("/docs/a.txt"),
                current: PathBuf::from("/docs/.bennu-batch-rename-0.tmp"),
                error: FileError::Rename {
                    from: PathBuf::from("/docs/.bennu-batch-rename-0.tmp"),
                    to: PathBuf::from("/docs/a.txt"),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
                },
            }],
        }
    }

    #[test]
    fn incomplete_batch_rename_lists_where_each_unrestored_item_is() {
        let text = batch_rename_failure_text(&incomplete_error(), UiLanguage::English);

        assert!(text.starts_with(
            "Batch rename failed and some items could not get their original names back: \
             /docs/a.txt → /docs/.bennu-batch-rename-0.tmp ("
        ));
        assert!(text.ends_with("Original error: operation cancelled"));
    }

    #[test]
    fn incomplete_batch_rename_lists_locations_in_chinese() {
        let text = batch_rename_failure_text(&incomplete_error(), UiLanguage::Chinese);

        assert!(text.starts_with(
            "批量重命名失败，部分项目未能恢复原名：/docs/a.txt → /docs/.bennu-batch-rename-0.tmp（"
        ));
        assert!(text.ends_with("原始错误：operation cancelled"));
    }

    #[test]
    fn other_batch_rename_failures_keep_the_error_text() {
        assert_eq!(
            batch_rename_failure_text(&FileError::Cancelled, UiLanguage::English),
            "operation cancelled"
        );
    }
}
