//! 右键解压：智能解压与「解压到 <包名>/」。
//!
//! 两项共用既有 inspect→密码→入队 管线，区别只在落法（`ArchiveExtractionPlacement`）。
//! 落在哪里由 file-core 在解压真正开始时定：排队期间同一个包可能已经被解过，
//! 入队时看到的空位到那时不一定还空着；而且只放进原来没有的位置，从不解进已有的文件夹。

use std::path::PathBuf;

use file_core::{ArchiveExtractionPlacement, ArchiveExtractionRequest};

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    /// 解压只落在真实目录里：回收站不是目录，压缩包里只读。
    fn current_location_accepts_extraction(&self) -> bool {
        self.tab().current_directory().is_some_and(|directory| {
            file_core::archive_path_identity(directory) == file_core::ArchivePathIdentity::RealFile
        })
    }

    /// 右键「智能解压到当前文件夹」。
    pub(super) fn smart_extract_selected(&mut self) -> iced::Task<Message> {
        self.extract_selected_archives(ArchiveExtractionPlacement::Smart)
    }

    /// 右键「解压到 <包名>/」。
    pub(super) fn extract_selected_to_archive_folder(&mut self) -> iced::Task<Message> {
        self.extract_selected_archives(ArchiveExtractionPlacement::ArchiveFolder)
    }

    fn extract_selected_archives(
        &mut self,
        placement: ArchiveExtractionPlacement,
    ) -> iced::Task<Message> {
        self.context_menu = None;
        if !self.current_location_accepts_extraction() {
            return iced::Task::none();
        }
        let commands = self
            .archive_selection_paths()
            .into_iter()
            .map(|archive| self.begin_archive_extraction(archive, placement))
            .collect::<Vec<_>>();
        iced::Task::batch(commands)
    }

    /// 中键点击压缩包条目的解压入口：与右键智能解压同一套入口契约
    /// （先清右键菜单、同一守卫），只是来源是单条目而不是选中集合——不清选中集，
    /// 中键解压不改变现有选择。被点击 pane 的激活在 update 分支完成，守卫因此按点击 pane 判定。
    pub(super) fn extract_archive_from_middle_click(
        &mut self,
        archive: PathBuf,
    ) -> iced::Task<Message> {
        self.context_menu = None;
        if !self.current_location_accepts_extraction() {
            return iced::Task::none();
        }
        // 视图入口按 kind 路由，扩展名判定在状态边界兜底：非压缩包
        // 路径没有解压语义，静默吞掉而不是带着错误请求走完后续流。
        if !file_core::is_supported_archive_path(&archive) {
            return iced::Task::none();
        }
        self.begin_archive_extraction(archive, ArchiveExtractionPlacement::Smart)
    }

    fn begin_archive_extraction(
        &mut self,
        archive: PathBuf,
        placement: ArchiveExtractionPlacement,
    ) -> iced::Task<Message> {
        let request = match ArchiveExtractionRequest::new(archive, placement) {
            Ok(request) => request,
            Err(error) => {
                self.show_global_error(error.to_string());
                return iced::Task::none();
            }
        };
        self.clear_state_for_archive_extraction();
        self.archive_extraction =
            Some(super::archive_extraction::ArchiveExtractionState::inspecting(request.clone()));
        crate::commands::inspect_archive_extraction_command(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BrowserLocation;
    use crate::model::BrowserPaneId;

    fn browser() -> FileBrowser {
        FileBrowser::new(crate::config::default_user_config()).0
    }

    #[test]
    fn extract_archive_from_middle_click_noops_in_trash_view() {
        let mut browser = browser();
        browser.tab_mut().location = BrowserLocation::Trash;

        drop(browser.extract_archive_from_middle_click(PathBuf::from("/workspace/bundle.zip")));

        // 与右键智能解压同规：回收站视图里中键解压不成立，无动作；
        // 菜单清空与守卫判定同属入口契约，守卫拒绝时也已生效。
        assert!(browser.archive_extraction.is_none());
        assert!(browser.context_menu.is_none());
    }

    #[test]
    fn extract_archive_from_middle_click_noops_inside_archive() {
        let mut browser = browser();
        // archive_path_identity 认的是真实存在的压缩包文件：路径不存在时它就是普通路径，
        // 这里的守卫也就拦不住，所以要真的放一个文件。
        let directory = tempfile::tempdir().expect("temp dir");
        let archive = directory.path().join("bundle.zip");
        std::fs::write(&archive, b"PK\x05\x06").expect("write archive");
        browser.tab_mut().location = BrowserLocation::Directory(archive.clone());

        drop(browser.extract_archive_from_middle_click(archive));

        // 包内只读：解压目的地落在虚拟路径上没有意义，无动作。
        assert!(browser.archive_extraction.is_none());
    }

    #[test]
    fn extract_archive_from_middle_click_noops_for_non_archive_path() {
        let mut browser = browser();

        drop(browser.extract_archive_from_middle_click(PathBuf::from("/workspace/notes.txt")));

        // 视图层按 kind 路由后，状态边界仍按扩展名兜底：非压缩包
        // 路径没有解压语义，不得进入 inspect 流。
        assert!(browser.archive_extraction.is_none());
    }

    #[test]
    fn archive_middle_press_activates_clicked_pane_before_guard() {
        let mut browser = browser();
        // Ctrl+中键先造出双栏布局，激活态落在新建的 SECOND pane。
        browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;
        drop(browser.update(Message::OpenDirectoryFromMiddleClick(
            BrowserPaneId::PRIMARY,
            PathBuf::from("/workspace/project"),
        )));
        assert!(matches!(
            browser.pane_layout,
            crate::model::BrowserPaneLayout::Split { .. }
        ));

        drop(browser.update(Message::ArchiveMiddlePressed(
            BrowserPaneId::PRIMARY,
            PathBuf::from("/workspace/bundle.zip"),
        )));

        // 守卫读 FileBrowser 级状态，必须先切回被点击的 PRIMARY 才判定；
        // 否则双栏下会用另一栏的回收站/包内上下文误拒或误放行。
        assert_eq!(browser.active_pane_id(), BrowserPaneId::PRIMARY);
    }
}
