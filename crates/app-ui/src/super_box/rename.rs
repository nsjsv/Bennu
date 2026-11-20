//! 改名模式（`r:`）：对选中的项。打的字是新名字，也是模板：`{n}` 是序号（从 1 起，几十项时补成
//! 两位），`{name}` `{stem}` `{ext}` 同动作文件（`{stem}` `{ext}` 按最后一个点分开）。一项时直接写
//! 新名字就行。只看字和调用方给的“这里已经有哪些名字”，不读盘。

use std::path::{Path, PathBuf};

use file_core::validate_entry_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Renamed {
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
}

/// 选中的项按模板改成什么；名字不变的不算。都不变、名字不能用、改完重名、和这里别的项撞名时是给
/// 用户看的原因。`folder` 是当前文件夹：只改它里面的项，撞名也只按它判断。
pub(crate) fn plan_renames(
    template: &str,
    items: &[PathBuf],
    folder: Option<&Path>,
    taken: &dyn Fn(&str) -> bool,
) -> Result<Vec<Renamed>, String> {
    if items.is_empty() {
        return Err(tr!("Select the items to rename first"));
    }
    let Some(folder) =
        folder.filter(|folder| items.iter().all(|item| item.parent() == Some(folder)))
    else {
        return Err(tr!("Only items in the current folder can be renamed here"));
    };
    let width = items.len().to_string().len();
    let mut renamed = Vec::new();
    let mut new_names = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let name = expand(template.trim(), item, index + 1, width);
        if validate_entry_name(name.as_ref()).is_err() {
            return Err(tr!("“%1” cannot be used as a name", name));
        }
        if new_names.contains(&name) {
            return Err(tr!("Two items would both be named “%1”", name));
        }
        let renaming_away = |name: &str| {
            items
                .iter()
                .any(|item| item.file_name() == Some(name.as_ref()))
        };
        if item.file_name() != Some(name.as_ref()) && taken(&name) && !renaming_away(&name) {
            return Err(tr!("There is already an item named “%1” here", name));
        }
        if item.file_name() != Some(name.as_ref()) {
            renamed.push(Renamed {
                from: item.clone(),
                to: folder.join(&name),
            });
        }
        new_names.push(name);
    }
    if renamed.is_empty() {
        return Err(tr!("The names stay the same"));
    }
    Ok(renamed)
}

fn expand(template: &str, item: &Path, number: usize, width: usize) -> String {
    let text =
        |part: Option<&std::ffi::OsStr>| part.unwrap_or_default().to_string_lossy().into_owned();
    template
        .replace("{n}", &format!("{number:0width$}"))
        .replace("{name}", &text(item.file_name()))
        .replace("{stem}", &text(item.file_stem()))
        .replace("{ext}", &text(item.extension()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from("/w").join(name))
            .collect()
    }

    fn plan(template: &str, names: &[&str], existing: &[&str]) -> Result<Vec<Renamed>, String> {
        let taken = |name: &str| existing.contains(&name);
        plan_renames(template, &here(names), Some(Path::new("/w")), &taken)
    }

    fn targets(renamed: Vec<Renamed>) -> Vec<String> {
        renamed
            .into_iter()
            .map(|renamed| {
                renamed
                    .to
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn one_item_gets_the_new_name_and_several_follow_the_template() {
        assert_eq!(
            targets(plan("报告.pdf", &["a.pdf"], &[]).unwrap()),
            ["报告.pdf"]
        );
        let twelve: Vec<String> = (1..=12).map(|index| format!("img{index}.JPG")).collect();
        let twelve: Vec<&str> = twelve.iter().map(String::as_str).collect();
        let renamed = targets(plan("照片-{n}.{ext}", &twelve, &[]).unwrap());
        assert_eq!(renamed[0], "照片-01.JPG");
        assert_eq!(renamed[11], "照片-12.JPG");
        assert_eq!(
            targets(plan("{stem}-旧.{ext}", &["a.tar.gz"], &[]).unwrap()),
            ["a.tar-旧.gz"]
        );
    }

    #[test]
    fn clashing_unusable_or_unchanged_names_are_refused() {
        assert!(plan("同名.txt", &["a.txt", "b.txt"], &[])
            .unwrap_err()
            .contains("同名.txt"));
        assert!(plan("b.txt", &["a.txt"], &["a.txt", "b.txt"])
            .unwrap_err()
            .contains("b.txt"));
        // 换着改（a ↔ 用 b 的名字，b 自己也改走）不算撞。
        assert_eq!(
            targets(plan("{n}.txt", &["2.txt", "1.txt"], &["1.txt", "2.txt"]).unwrap()),
            ["1.txt", "2.txt"]
        );
        assert!(plan("a/b", &["a.txt"], &[]).is_err());
        assert!(
            plan("a.txt", &["a.txt"], &["a.txt"]).is_err(),
            "nothing changes"
        );
        let taken = |_: &str| false;
        assert!(plan_renames("x", &[], Some(Path::new("/w")), &taken).is_err());
        assert!(plan_renames("x", &here(&["a"]), Some(Path::new("/other")), &taken).is_err());
    }
}
