use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use file_core::numbered_duplicate_name;

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

pub(super) fn transfer_targets(
    directory: &Path,
    sources: &[PathBuf],
    mode: PasteTargetMode,
) -> Vec<(PathBuf, PathBuf)> {
    let mut reserved_targets = HashSet::new();
    sources
        .iter()
        .map(|source| {
            let candidate = child_path(directory, source);
            if mode == PasteTargetMode::Move && candidate == *source {
                reserved_targets.insert(candidate.clone());
                return (source.clone(), source.clone());
            }
            let target = if reserved_targets.contains(&candidate) {
                unique_alternate_path(&candidate, &mut reserved_targets)
            } else {
                reserved_targets.insert(candidate.clone());
                candidate
            };
            (source.clone(), target)
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

pub(super) fn unique_alternate_path(
    target: &Path,
    reserved_targets: &mut HashSet<PathBuf>,
) -> PathBuf {
    if !reserved_targets.contains(target) {
        let target = target.to_path_buf();
        reserved_targets.insert(target.clone());
        return target;
    }

    let parent = target.parent().unwrap_or_else(|| Path::new(""));
    let name = target
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("item"));

    for index in 2..1001 {
        let candidate = parent.join(numbered_duplicate_name(&name, index));
        if !reserved_targets.contains(&candidate) {
            reserved_targets.insert(candidate.clone());
            return candidate;
        }
    }

    target.to_path_buf()
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
}
