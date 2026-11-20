//! “选中重复的文件”：当前文件夹里列出来的文件，在后台找内容相同的（`crate::duplicates`），每组留
//! 最早的一份，其余的选中，底栏说选中了几个。不删：用户看过再按 Delete 进回收站，或者接着用管道。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    pub(super) fn select_duplicate_files(&mut self) -> Task<Message> {
        let tab = self.tab();
        let Some(folder) = tab.current_directory().map(PathBuf::from) else {
            return Task::none();
        };
        let files: Vec<PathBuf> = tab
            .entries
            .iter()
            .filter(|entry| {
                entry.path().parent() == Some(folder.as_path())
                    && entry.kind() == file_core::FileKind::File
            })
            .map(|entry| entry.path().to_path_buf())
            .collect();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || crate::duplicates::redundant_copies(files))
                    .await
                    .unwrap_or_default()
            },
            move |copies| Message::DuplicatesFound {
                folder: folder.clone(),
                copies,
            },
        )
    }

    /// 算完时已经换了文件夹就不选。
    pub(super) fn accept_duplicates_found(
        &mut self,
        folder: PathBuf,
        copies: Vec<PathBuf>,
    ) -> Task<Message> {
        if self.tab().current_directory() != Some(folder.as_path()) {
            return Task::none();
        }
        if copies.is_empty() {
            return self.show_hint(tr!("No duplicate files here"));
        }
        let count = copies.len();
        self.select_operation_result_paths(copies);
        self.show_hint(tr!(
            "Selected %1 duplicate files; the oldest copy of each is left",
            count
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::app::task_driving_tests::drive;
    use crate::model::{BrowserLocation, NavigationMode};
    use crate::shortcuts::ShortcutAction;

    fn write(path: &Path, content: &[u8], seconds_ago: u64) {
        std::fs::write(path, content).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds_ago))
            .unwrap();
    }

    async fn browser_listing(directory: &Path) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
        let load = browser.navigate_to(directory.to_path_buf(), NavigationMode::RecordHistory);
        drive(&mut browser, load).await;
        browser
    }

    #[tokio::test]
    async fn the_newer_copies_are_selected_and_the_bottom_bar_says_how_many() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        write(&root.join("a.jpg"), b"same", 300);
        write(&root.join("b.jpg"), b"same", 100);
        write(&root.join("c.jpg"), b"same", 200);
        write(&root.join("d.jpg"), b"diff", 400);
        write(&root.join("e.jpg"), b"longer", 500);
        let mut browser = browser_listing(&root).await;

        let found = browser.invoke_shortcut(ShortcutAction::SelectDuplicates);
        drive(&mut browser, found).await;

        let mut selected: Vec<_> = browser.tab().selected_paths.iter().cloned().collect();
        selected.sort();
        assert_eq!(selected, [root.join("b.jpg"), root.join("c.jpg")]);
        // 底栏的说明（drive 跑完时它已经到点收起了，这里直接看）。
        drop(browser.accept_duplicates_found(root.clone(), selected));
        assert!(browser
            .action_hint
            .as_ref()
            .is_some_and(|hint| hint.text.contains('2')));
        drop(browser.accept_duplicates_found(root.join("别处"), vec![root.join("a.jpg")]));
        assert!(
            !browser.is_path_selected(&root.join("a.jpg")),
            "a result for another folder is dropped"
        );
    }

    #[tokio::test]
    async fn large_files_are_looked_for_here_from_the_largest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut browser = browser_listing(&root).await;

        drop(browser.invoke_shortcut(ShortcutAction::FindLargeFiles));

        let search = browser.active_search().expect("the search page is open");
        assert_eq!(search.filters.custom_size_min, "100m");
        assert_eq!(
            search.root.selected_scope(),
            crate::model::SearchDirectoryScope::CurrentFolder
        );
        assert_eq!(
            search.order(),
            file_search::SearchOrder::Field {
                field: file_search::SearchSortField::Size,
                direction: file_search::SearchSortDirection::Descending,
                directories_first: false,
            }
        );
    }
}
