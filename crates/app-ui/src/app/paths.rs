use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use file_core::TransferConflictStrategy;

use crate::operation_queue::QueuedTransfer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteTargetMode {
    Copy,
    Move,
}

pub(super) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn completed_path_text(path: &Path) -> String {
    let mut text = path_text(path);
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

/// 粘贴/拖放到 `directory` 的传输，目标沿用源名。同一批里目标重名的后来者目标
/// 不变、策略设为 KeepBoth，由引擎提交时续编号——界面不替它起名。
pub(super) fn transfers_into_directory(
    directory: &Path,
    sources: &[PathBuf],
    mode: PasteTargetMode,
) -> Vec<QueuedTransfer> {
    let mut reserved_targets = HashSet::new();
    sources
        .iter()
        .map(|source| {
            let target = child_path(directory, source);
            let transfer = QueuedTransfer::new(source.clone(), target.clone());
            if mode == PasteTargetMode::Move && target == *source {
                reserved_targets.insert(target);
                return transfer;
            }
            if reserved_targets.insert(target) {
                transfer
            } else {
                QueuedTransfer {
                    conflict_strategy: TransferConflictStrategy::KeepBoth,
                    ..transfer
                }
            }
        })
        .collect()
}

/// 移动到 `target_directory` 是否为空操作:源等于落点目录、落入自身子树、
/// 或已在落点目录内,移动落地不会产生任何变化。剪贴板移动粘贴与文件拖拽
/// 的落地过滤/落点安全判定共用此不变量——部分源为空操作时其余条目仍可移动。
pub(super) fn move_is_no_op(source: &Path, target_directory: &Path) -> bool {
    source == target_directory
        || target_directory.starts_with(source)
        || source
            .parent()
            .is_some_and(|parent| parent == target_directory)
}

fn child_path(directory: &Path, source: &Path) -> PathBuf {
    directory.join(transfer_source_name(source))
}

fn transfer_source_name(source: &Path) -> OsString {
    source
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("item"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_no_op_predicate_matches_directory_relationships() {
        let target_directory = Path::new("/data/project");
        let dragged_child = Path::new("/data/project/inner.txt");
        let outsider = Path::new("/data/other.txt");

        // 目录移入自身、源已在落点目录内:空操作。
        assert!(move_is_no_op(target_directory, target_directory));
        assert!(move_is_no_op(dragged_child, target_directory));
        // 落点目录在源自身子树内(目录移进自己的子目录):空操作。
        assert!(move_is_no_op(
            target_directory,
            &target_directory.join("inner")
        ));
        // 落点目录外的条目:可移动。
        assert!(!move_is_no_op(outsider, target_directory));
    }

    #[test]
    fn later_sources_with_the_same_name_keep_their_target_and_let_the_engine_number_them() {
        let sources = [
            PathBuf::from("/a/report.pdf"),
            PathBuf::from("/b/report.pdf"),
            PathBuf::from("/c/notes.txt"),
        ];

        let transfers =
            transfers_into_directory(Path::new("/target"), &sources, PasteTargetMode::Copy);

        let planned = transfers
            .iter()
            .map(|transfer| (transfer.target.clone(), transfer.conflict_strategy))
            .collect::<Vec<_>>();
        assert_eq!(
            planned,
            [
                (
                    PathBuf::from("/target/report.pdf"),
                    TransferConflictStrategy::Fail
                ),
                (
                    PathBuf::from("/target/report.pdf"),
                    TransferConflictStrategy::KeepBoth
                ),
                (
                    PathBuf::from("/target/notes.txt"),
                    TransferConflictStrategy::Fail
                ),
            ]
        );
    }
}
