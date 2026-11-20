//! 在哪找服务菜单、信不信它们，同 KIO 的 `serviceMenuFilePaths` 和
//! `KDesktopFile::isAuthorizedDesktopFile`；`kservicemenurc` 里关掉的动作不要。

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::Metadata;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::mime::MimeDatabase;

use super::super::library::{read_trusted_text, ActionFileProblem};
use super::menu_file::{read_menu_file, MenuFolder, ReadContext};
use super::ServiceMenuAction;

/// 每个数据目录下服务菜单所在的两个位置，新的在前。
pub(crate) const MENU_FOLDERS: [&str; 2] = ["kio/servicemenus", "kservices5"];

/// 按 KIO 的顺序读出所有服务菜单。会读盘，只在后台线程调用。
pub(crate) fn load_service_menus(
    data_dirs: &[PathBuf],
    show_settings: &Path,
    types: &MimeDatabase,
    locale: Option<&str>,
) -> (Vec<Arc<ServiceMenuAction>>, Vec<ActionFileProblem>) {
    let hidden = std::fs::read_to_string(show_settings)
        .map(|text| hidden_actions(&text))
        .unwrap_or_default();
    let context = ReadContext {
        types,
        locale,
        hidden: &hidden,
    };
    let mut actions = Vec::new();
    let mut problems = Vec::new();
    let mut seen = HashSet::<OsString>::new();
    for (relative, folder) in [
        (MENU_FOLDERS[0], MenuFolder::Current),
        (MENU_FOLDERS[1], MenuFolder::Legacy),
    ] {
        for directory in data_dirs.iter().map(|data_dir| data_dir.join(relative)) {
            let files = match menu_files(&directory) {
                Ok(files) => files,
                Err(error) => {
                    problems.push(ActionFileProblem {
                        file: directory,
                        line: None,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            // 同名文件只用第一个（同 KIO 按文件名去重），用户的能盖掉系统的。
            for file in files {
                let name = file
                    .file_name()
                    .expect("listed files have names")
                    .to_owned();
                if !seen.insert(name.clone()) {
                    continue;
                }
                let problem = |reason: String| ActionFileProblem {
                    file: file.clone(),
                    line: None,
                    reason,
                };
                let Some(file_name) = name.to_str() else {
                    problems.push(problem(tr!("The file name has to be text")));
                    continue;
                };
                let text = match read_trusted_text(&file) {
                    Ok((text, metadata)) => match authorization_problem(&metadata, folder, &file) {
                        None => text,
                        Some(reason) => {
                            problems.push(problem(reason));
                            continue;
                        }
                    },
                    Err(reason) => {
                        problems.push(problem(reason));
                        continue;
                    }
                };
                match read_menu_file(&file, file_name, &text, folder, &context) {
                    Ok(menu) => {
                        actions.extend(menu.actions.into_iter().map(Arc::new));
                        problems.extend(menu.mistakes.into_iter().map(problem));
                    }
                    Err(reason) => problems.push(problem(reason)),
                }
            }
        }
    }
    (actions, problems)
}

/// 一个目录里的 `*.desktop`，按名字排序（KIO 用目录的原始顺序，每次不一定一样）。不进子目录。
/// 目录不存在时是空的。
fn menu_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "desktop")
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

/// `kio/servicemenus` 里的文件要有执行权限或者属于 root，`kservices5` 里的直接可信，
/// 同 `KDesktopFile::isAuthorizedDesktopFile`。读文件时已经查过属主和别人能不能改。
fn authorization_problem(metadata: &Metadata, folder: MenuFolder, file: &Path) -> Option<String> {
    let authorized =
        folder == MenuFolder::Legacy || metadata.uid() == 0 || metadata.mode() & 0o100 != 0;
    (!authorized).then(|| {
        tr!(
            "It is not executable, so it is not used. If you trust it, run: chmod +x %1",
            file.display()
        )
    })
}

/// `kservicemenurc` 的 `[Show]` 组里值为假的键：在 Dolphin 设置里关掉的动作。按行读，
/// 不交给 KeyFile：KConfig 的 `[$i]` 这类写法会让它整份读不出来。真假值的写法同 KConfig。
fn hidden_actions(text: &str) -> HashSet<String> {
    let mut hidden = HashSet::new();
    let mut in_show = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_show = line == "[Show]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_show) else {
            continue;
        };
        let value = value.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "false" | "off" | "no" | "0") {
            hidden.insert(key.trim().to_owned());
        }
    }
    hidden
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
