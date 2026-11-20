//! 智能文件夹：存下的一次搜索，写法同超级输入框的 `f:`（词和 类型:/时间:/大小: 筛选），存在设置里。
//! 侧栏里点开就是这样一次搜索（范围是全部索引，同 `f:`）；右键移除。在框的文件模式里按 Ctrl+S
//! 存下。存的是字：人看得懂，筛选的读法以后变了也照样能打开。

use iced::Task;

use super::FileBrowser;
use crate::model::{ContextMenuState, Message, SmartFolderContextMenuState};

impl FileBrowser {
    /// 存下这个 `f:` 写法；已经有了就不重复。
    pub(super) fn save_smart_folder(&mut self, query: String) -> Task<Message> {
        let query = query.trim().to_owned();
        if query.is_empty() || self.user_config.smart_folders.contains(&query) {
            return Task::none();
        }
        self.user_config.smart_folders.push(query);
        Task::batch([
            self.persist_user_preferences_command(),
            self.show_hint(tr!("Saved as a smart folder in the sidebar")),
        ])
    }

    pub(super) fn open_smart_folder(&mut self, query: &str) -> Task<Message> {
        self.context_menu = None;
        match crate::super_box::read_find_query(query) {
            Ok(found) => self.open_search_with(found.terms, found.filters),
            // 存下以后筛选的写法变了：说明，不打开。
            Err(reason) => {
                self.show_global_error(tr!("Smart folder “%1”: %2", query, reason));
                Task::none()
            }
        }
    }

    pub(super) fn open_smart_folder_menu(&mut self, query: String) -> Task<Message> {
        self.context_menu = Some(ContextMenuState::SmartFolder(SmartFolderContextMenuState {
            query,
            position: self.cursor_or_origin(),
        }));
        Task::none()
    }

    pub(super) fn remove_smart_folder(&mut self, query: &str) -> Task<Message> {
        self.context_menu = None;
        self.user_config
            .smart_folders
            .retain(|saved| saved != query);
        self.persist_user_preferences_command()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::task_driving_tests::drive;
    use crate::app::SuperBoxMessage;
    use crate::model::BrowserLocation;
    use crate::shortcuts::ShortcutAction;

    fn browser() -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(std::env::temp_dir());
        browser
    }

    async fn ctrl_s_in_box(browser: &mut FileBrowser, text: &str) {
        drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
        let typed = browser.update(Message::SuperBox(SuperBoxMessage::InputChanged(
            text.to_owned(),
        )));
        drive(browser, typed).await;
        let saved = browser
            .super_box_key(
                &iced::keyboard::Key::Character("s".into()),
                iced::keyboard::Modifiers::CTRL,
            )
            .expect("the box takes the key");
        drop(saved);
    }

    #[tokio::test]
    async fn ctrl_s_in_files_mode_saves_the_query_once() {
        let mut browser = browser();
        ctrl_s_in_box(&mut browser, "f:会议 类型:文档").await;
        assert!(browser.super_box.is_none());
        assert_eq!(browser.user_config.smart_folders, ["会议 类型:文档"]);
        ctrl_s_in_box(&mut browser, "f:会议 类型:文档").await;
        assert_eq!(browser.user_config.smart_folders.len(), 1, "not twice");
        // 筛选写错的、只有筛选没有词的不存。
        ctrl_s_in_box(&mut browser, "f:类型:文档").await;
        assert_eq!(browser.user_config.smart_folders.len(), 1);
    }

    #[test]
    fn a_smart_folder_opens_its_search_and_can_be_removed() {
        let mut browser = browser();
        browser.user_config.smart_folders = vec!["会议 类型:文档".to_owned()];
        drop(browser.update(Message::SmartFolderOpened("会议 类型:文档".to_owned())));
        let search = browser.active_search().expect("the search page is open");
        assert_eq!(search.input, "会议");
        assert_eq!(search.filters.selected_entry_types.len(), 1);

        drop(browser.update(Message::SmartFolderRightClicked(
            "会议 类型:文档".to_owned(),
        )));
        assert!(matches!(
            browser.context_menu,
            Some(ContextMenuState::SmartFolder(_))
        ));
        drop(browser.update(Message::SmartFolderRemoved("会议 类型:文档".to_owned())));
        assert!(browser.user_config.smart_folders.is_empty());
        assert!(browser.context_menu.is_none());
    }

    #[tokio::test]
    async fn the_super_box_finds_smart_folders_by_name() {
        let mut browser = browser();
        browser.user_config.smart_folders = vec!["季度报告 时间:30天".to_owned()];
        drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
        let typed = browser.update(Message::SuperBox(SuperBoxMessage::InputChanged(
            "季度".to_owned(),
        )));
        drive(&mut browser, typed).await;
        let super_box = browser.super_box.as_ref().unwrap();
        let first = super_box.rows().first().cloned();
        let Some(crate::super_box::BoxRow::Entry(index)) = first else {
            panic!("{:?}", super_box.rows());
        };
        assert_eq!(
            super_box.entry(index).kind,
            crate::super_box::EntryKind::SmartFolder
        );
    }
}
