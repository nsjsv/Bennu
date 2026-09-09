//! 副本 / 保留两者 / 符号链接 / 收纳文件夹共用的命名规则:
//! 后缀插在「名字末尾与扩展名之间」,冲突时以空格加序号递增
//! (`report.pdf` → `report副本.pdf` → `report副本 2.pdf`)。
//! 目录名不分扩展名直接尾部追加;以 `.` 开头且无扩展名的隐藏文件
//! 整体视作主名(`.bashrc` → `.bashrc副本`)。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// 与 `operation_queue::NEW_DIRECTORY_NAME`(「New...」菜单的英文目录)区分:
/// 收纳选中项的新文件夹按工单固定使用中文名。
pub(crate) const GATHERED_FOLDER_BASE_NAME: &str = "新建文件夹";

const UNIQUE_NAME_LIMIT: usize = 1000;

/// 同一目录内「名字已被占用」的判定;悬空符号链接也算占用,
/// 避免对指向不存在目标的链接原地覆盖。
pub(crate) fn entry_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// 复制副本用的文件名;扩展名保持原样。
pub(crate) fn unique_duplicated_file_name(
    file_name: &OsStr,
    is_taken: impl FnMut(&OsStr) -> bool,
) -> OsString {
    unique_suffixed_name(file_name, "副本", true, is_taken)
}

/// 复制副本用的目录名;目录名整体追加后缀,不做扩展名拆分。
pub(crate) fn unique_duplicated_directory_name(
    name: &OsStr,
    is_taken: impl FnMut(&OsStr) -> bool,
) -> OsString {
    unique_suffixed_name(name, "副本", false, is_taken)
}

/// 符号链接用的文件名;`stem + "链接"` 插在扩展名之前。
pub(crate) fn unique_symlink_file_name(
    file_name: &OsStr,
    is_taken: impl FnMut(&OsStr) -> bool,
) -> OsString {
    unique_suffixed_name(file_name, "链接", true, is_taken)
}

/// 符号链接用的目录名;目录名尾部直接追加「链接」。
pub(crate) fn unique_symlink_directory_name(
    name: &OsStr,
    is_taken: impl FnMut(&OsStr) -> bool,
) -> OsString {
    unique_suffixed_name(name, "链接", false, is_taken)
}

/// `parent` 下第一个可用的「新建文件夹( 2/3…)」目录完整路径。
pub(crate) fn unique_gathered_folder_directory(parent: &Path) -> PathBuf {
    let name = unique_suffixed_name(
        OsStr::new(GATHERED_FOLDER_BASE_NAME),
        "",
        false,
        |name| entry_exists(&parent.join(name)),
    );
    parent.join(name)
}

/// 列出按占用顺序尝试的全部候选名;首个未被占用的即结果。
/// 供异步侧逐个探测使用(执行器里没有同步 FS 可用的闭包)。
pub(crate) fn suffixed_name_candidates(original: &OsStr, suffix: &str, split_extension: bool) -> Vec<OsString> {
    let (stem, extension) = split_name(original, split_extension);
    let mut candidates = Vec::with_capacity(UNIQUE_NAME_LIMIT);
    candidates.push(composed_name(&stem, suffix, extension.as_deref()));
    for index in 2..UNIQUE_NAME_LIMIT {
        candidates.push(composed_name(
            &stem,
            &format!("{suffix} {index}"),
            extension.as_deref(),
        ));
    }
    candidates
}

fn unique_suffixed_name(
    original: &OsStr,
    suffix: &str,
    split_extension: bool,
    mut is_taken: impl FnMut(&OsStr) -> bool,
) -> OsString {
    suffixed_name_candidates(original, suffix, split_extension)
        .into_iter()
        .find(|candidate| !is_taken(candidate))
        .unwrap_or_else(|| original.to_os_string())
}

/// `split_extension` 由调用方按「文件 / 目录」概念选择,而不是传布尔的
/// 场景分支:文件按 `Path::extension` 拆(`.bashrc` 无扩展名,整体是主名),
/// 目录永不拆。
fn split_name(original: &OsStr, split_extension: bool) -> (OsString, Option<OsString>) {
    if !split_extension {
        return (original.to_os_string(), None);
    }
    let path = Path::new(original);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => (
            stem.to_os_string(),
            Some(extension.to_os_string()),
        ),
        _ => (original.to_os_string(), None),
    }
}

fn composed_name(stem: &OsStr, middle: &str, extension: Option<&OsStr>) -> OsString {
    let mut name = OsString::from(stem);
    name.push(middle);
    if let Some(extension) = extension {
        name.push(".");
        name.push(extension);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn taken_set<'a>(names: &'a [&'a str]) -> impl FnMut(&OsStr) -> bool + 'a {
        let taken: HashSet<OsString> = names
            .iter()
            .map(|name| OsString::from(name))
            .collect();
        move |candidate: &OsStr| taken.contains(candidate)
    }

    #[test]
    fn duplicate_suffix_goes_between_name_and_extension() {
        assert_eq!(
            unique_duplicated_file_name(OsStr::new("report.pdf"), taken_set(&[])),
            OsString::from("report副本.pdf")
        );
    }

    #[test]
    fn duplicate_increments_with_space_before_extension() {
        let is_taken = taken_set(&["report副本.pdf"]);
        assert_eq!(
            unique_duplicated_file_name(OsStr::new("report.pdf"), is_taken),
            OsString::from("report副本 2.pdf")
        );
        let is_taken = taken_set(&["report副本.pdf", "report副本 2.pdf"]);
        assert_eq!(
            unique_duplicated_file_name(OsStr::new("report.pdf"), is_taken),
            OsString::from("report副本 3.pdf")
        );
    }

    #[test]
    fn duplicate_without_extension_appends_suffix_directly() {
        assert_eq!(
            unique_duplicated_file_name(OsStr::new("notes"), taken_set(&[])),
            OsString::from("notes副本")
        );
        let is_taken = taken_set(&["notes副本"]);
        assert_eq!(
            unique_duplicated_file_name(OsStr::new("notes"), is_taken),
            OsString::from("notes副本 2")
        );
    }

    #[test]
    fn directory_names_never_split_extensions() {
        assert_eq!(
            unique_duplicated_directory_name(OsStr::new("data"), taken_set(&[])),
            OsString::from("data副本")
        );
        // 目录即使带点号也整体追加,不把点后内容当扩展名。
        assert_eq!(
            unique_duplicated_directory_name(OsStr::new("data.v2"), taken_set(&[])),
            OsString::from("data.v2副本")
        );
        let is_taken = taken_set(&["data副本"]);
        assert_eq!(
            unique_duplicated_directory_name(OsStr::new("data"), is_taken),
            OsString::from("data副本 2")
        );
    }

    #[test]
    fn hidden_extensionless_files_use_whole_name_as_stem() {
        assert_eq!(
            unique_duplicated_file_name(OsStr::new(".bashrc"), taken_set(&[])),
            OsString::from(".bashrc副本")
        );
        let is_taken = taken_set(&[".bashrc副本"]);
        assert_eq!(
            unique_duplicated_file_name(OsStr::new(".bashrc"), is_taken),
            OsString::from(".bashrc副本 2")
        );
    }

    #[test]
    fn symlink_suffix_follows_the_same_rules() {
        assert_eq!(
            unique_symlink_file_name(OsStr::new("report.pdf"), taken_set(&[])),
            OsString::from("report链接.pdf")
        );
        assert_eq!(
            unique_symlink_directory_name(OsStr::new("data"), taken_set(&[])),
            OsString::from("data链接")
        );
        let is_taken = taken_set(&["report链接.pdf"]);
        assert_eq!(
            unique_symlink_file_name(OsStr::new("report.pdf"), is_taken),
            OsString::from("report链接 2.pdf")
        );
    }

    #[test]
    fn gathered_folder_base_name_increments_like_other_suffixes() {
        let parent = Path::new("/workspace");
        assert_eq!(
            unique_gathered_folder_directory(parent),
            PathBuf::from("/workspace/新建文件夹")
        );
        let is_taken = taken_set(&["新建文件夹"]);
        assert_eq!(
            unique_suffixed_name(OsStr::new(GATHERED_FOLDER_BASE_NAME), "", false, is_taken),
            OsString::from("新建文件夹 2")
        );
    }

    #[test]
    fn candidate_lists_are_bounded_and_ordered() {
        let candidates = suffixed_name_candidates(OsStr::new("report.pdf"), "副本", true);
        assert_eq!(candidates[0], OsString::from("report副本.pdf"));
        assert_eq!(candidates[1], OsString::from("report副本 2.pdf"));
        assert_eq!(candidates.len(), UNIQUE_NAME_LIMIT - 1);
    }
}
