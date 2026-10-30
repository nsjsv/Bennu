use bennu_localization::UiLanguage;
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
/// 条目清单放在最前：任务行中间省略时仍能看到第一个条目的位置，完整文本经任务
/// "复制详情"取得。
fn batch_rename_failure_text(error: &FileError, language: UiLanguage) -> String {
    match incomplete_rename_texts(error, language) {
        Some((cause, items)) => tr_in!(
            language,
            "Batch rename failed and some items could not get their original names back: %1. Original error: %2",
            items,
            cause
        ),
        None => error.to_string(),
    }
}

/// 单个重命名两步改名后没能放回原名时同理，必须说出条目此刻在哪里。
pub(super) fn rename_failure_text(error: &FileError, language: UiLanguage) -> String {
    match incomplete_rename_texts(error, language) {
        Some((cause, items)) => tr_in!(
            language,
            "Rename failed and the item could not get its original name back: %1. Original error: %2",
            items,
            cause
        ),
        None => error.to_string(),
    }
}

fn incomplete_rename_texts(error: &FileError, language: UiLanguage) -> Option<(String, String)> {
    // 改名不新建目录，回滚报告里不会有残留目录。
    let FileError::EntryMovesIncomplete {
        cause, unrestored, ..
    } = error
    else {
        return None;
    };
    let items = unrestored
        .iter()
        .map(|item| {
            tr_in!(
                language,
                "%1 → %2 (%3)",
                item.original.display(),
                item.current.display(),
                item.error
            )
        })
        .collect::<Vec<_>>()
        .join(&tr_in!(language, "; "));
    Some((cause.to_string(), items))
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
            leftover_entries: Vec::new(),
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
