//! 超级输入框补全的候选：在后台列一个文件夹，或者 PATH 里的命令，名字开头对得上（不分大小写）的。
//! 用法在 `crate::super_box::complete`。

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use iced::Task;

use super::super_box::SuperBoxMessage;
use crate::model::Message;
use crate::super_box::{Candidate, ListSource, Listing};

/// 最多留这么多：再多也挑不过来，只列出前面的。
const MOST_CANDIDATES: usize = 5000;

pub(super) fn list_super_box_candidates(
    generation: u64,
    source: ListSource,
    name: String,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || listing(source, &name))
                .await
                .unwrap_or_default()
        },
        move |listing| {
            Message::SuperBox(SuperBoxMessage::ListingLoaded {
                generation,
                listing,
            })
        },
    )
}

fn listing(source: ListSource, name: &str) -> Listing {
    let folded = name.to_lowercase();
    let wanted = |candidate: &str| candidate.to_lowercase().starts_with(&folded);
    let (folder, mut candidates) = match source {
        ListSource::Programs => (None, programs(&wanted)),
        ListSource::Folder(folder) => {
            let candidates = folder_entries(&folder, &wanted);
            (Some(folder), candidates)
        }
    };
    candidates.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let truncated = candidates.len() > MOST_CANDIDATES;
    candidates.truncate(MOST_CANDIDATES);
    Listing {
        folder,
        candidates,
        truncated,
    }
}

/// 名字不是 UTF-8 的跳过：框里打不出来。链接按它指向的算，指向文件夹的也补 `/`。
fn folder_entries(folder: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !wanted(&name) {
                return None;
            }
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir());
            Some(Candidate { name, is_dir })
        })
        .collect()
}

/// PATH 里能执行的文件，同名的只算一个。
fn programs(wanted: &dyn Fn(&str) -> bool) -> Vec<Candidate> {
    let mut names = BTreeSet::new();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for folder in std::env::split_paths(&path) {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if names.contains(&name) || !wanted(&name) {
                continue;
            }
            let runnable = std::fs::metadata(entry.path()).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            });
            if runnable {
                names.insert(name);
            }
        }
    }
    names
        .into_iter()
        .map(|name| Candidate {
            name,
            is_dir: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_lists_matching_names_in_any_case_with_folders_marked() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("Docs")).unwrap();
        std::fs::write(folder.path().join("doc.txt"), b"").unwrap();
        std::fs::write(folder.path().join("other"), b"").unwrap();
        std::os::unix::fs::symlink(folder.path().join("Docs"), folder.path().join("docs-link"))
            .unwrap();

        let listed = listing(ListSource::Folder(folder.path().to_path_buf()), "DOC");

        assert_eq!(listed.folder.as_deref(), Some(folder.path()));
        let names: Vec<_> = listed
            .candidates
            .iter()
            .map(|candidate| (candidate.name.as_str(), candidate.is_dir))
            .collect();
        assert_eq!(
            names,
            [("Docs", true), ("doc.txt", false), ("docs-link", true)]
        );
        assert!(!listed.truncated);
    }

    /// 打字时在后台列好，Tab 直接补；淡色建议也从同一份里来。
    #[tokio::test]
    async fn tab_in_the_box_completes_a_folder_and_a_command() {
        use crate::app::task_driving_tests::drive;
        use crate::shortcuts::ShortcutAction;
        use iced::keyboard::{key::Named, Key, Modifiers};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("Documents")).unwrap();
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        browser.tab_mut().location = crate::model::BrowserLocation::Directory(root.clone());
        let tab = Key::Named(Named::Tab);

        for (typed, suggestion, completed) in [
            ("g:Doc", "uments/", "Documents/"),
            ("s:cd Doc", "uments/", "cd Documents/"),
            ("s:ech", "o", "echo "),
        ] {
            // 每次新开一个框：有了胶囊就不能再打别的标签。
            browser.super_box = None;
            drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
            let changed = browser.update(Message::SuperBox(SuperBoxMessage::InputChanged(
                typed.to_owned(),
            )));
            drive(&mut browser, changed).await;
            let super_box = browser.super_box.as_ref().unwrap();
            assert_eq!(super_box.suggestion(), Some(suggestion), "{typed}");
            let pressed = browser.super_box_key(&tab, Modifiers::empty()).unwrap();
            drive(&mut browser, pressed).await;
            assert_eq!(browser.super_box.as_ref().unwrap().input(), completed);
        }
    }

    #[test]
    fn programs_come_from_path() {
        let listed = listing(ListSource::Programs, "sh");
        assert!(listed
            .candidates
            .iter()
            .any(|candidate| candidate.name == "sh"));
        assert!(listed.folder.is_none());
    }
}
