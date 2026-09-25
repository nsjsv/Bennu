//! 右键解压：智能解压（bandizip 语义）与「解压到 <包名>/」。
//!
//! 两项共用既有 inspect→密码→入队 管线；智能解压的唯一差异是
//! 目的地由「单根判定 + 重名递增」计算得出。

use std::path::{Path, PathBuf};

use file_core::ArchiveExtractionRequest;

use super::FileBrowser;
use crate::model::Message;

/// 智能解压目的地：包内所有成员在同一根目录下时直接落压缩包所在文件夹
/// （目录已存在，解压管线直接解入）；否则新建「<包名>/」文件夹
/// （已存在则追加 `(2)`、`(3)`… 序号）。
///
/// 目的地固定按压缩包父目录计算而不是视图当前目录：搜索结果等视图里
/// 右键时，视图根目录不是用户预期的解压位置。
pub(super) fn smart_extraction_destination(
    archive: &Path,
    single_root_name: Option<String>,
    destination_exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let target_directory = archive.parent().unwrap_or_else(|| Path::new("."));
    match single_root_name {
        Some(_) => target_directory.to_path_buf(),
        None => {
            let base = target_directory.join(archive_folder_name(archive));
            unique_folder_destination(base, 2, destination_exists)
        }
    }
}

/// 「解压到 <包名>/」的目的地：无条件在压缩包所在文件夹包一层，重名自动递增。
pub(super) fn archive_folder_destination(
    archive: &Path,
    destination_exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let base = archive
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(archive_folder_name(archive));
    unique_folder_destination(base, 2, destination_exists)
}

fn unique_folder_destination(
    base: PathBuf,
    start_counter: u32,
    destination_exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let mut candidate = base.clone();
    let mut counter = start_counter;
    while destination_exists(&candidate) {
        candidate = base.with_file_name(format!(
            "{} ({counter})",
            base.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archive".to_owned())
        ));
        counter += 1;
    }
    candidate
}

/// 剥掉压缩包扩展名作为落地文件夹名，与既有解压行为同名同规则。
fn archive_folder_name(archive: &Path) -> String {
    let file_name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_owned());
    let lowered = file_name.to_ascii_lowercase();
    for suffix in [".tar.gz", ".tgz", ".zip", ".tar", ".7z", ".rar"] {
        if lowered.ends_with(suffix) {
            return file_name[..file_name.len() - suffix.len()].to_owned();
        }
    }
    file_name
}

impl FileBrowser {
    /// 右键「智能解压到当前文件夹」入口：对每个选中的归档异步判定
    /// 单根，判定结果回流后计算目的地并走既有 inspect→密码→入队 流。
    pub(super) fn smart_extract_selected(&mut self) -> iced::Task<Message> {
        self.context_menu = None;
        if self.is_trash_view || self.current_directory_is_inside_archive() {
            return iced::Task::none();
        }
        let archives = self.archive_selection_paths();
        if archives.is_empty() {
            return iced::Task::none();
        }
        iced::Task::batch(
            archives
                .into_iter()
                .map(|archive| {
                    iced::Task::perform(
                        async move {
                            let single_root = file_core::single_root_member_name(&archive)
                                .await
                                .map_err(|error| error.to_string());
                            (archive, single_root)
                        },
                        move |(archive, single_root)| Message::SmartExtractDestinationResolved {
                            archive,
                            single_root,
                        },
                    )
                })
                .collect::<Vec<_>>(),
        )
    }

    /// 中键点击压缩包条目的解压入口：与右键智能解压同一套入口契约
    /// （先清右键菜单、同一守卫），随后走同一条单根判定流，只是来源是
    /// 单条目而不是选中集合——不清选中集，中键解压不改变现有选择。
    /// 被点击 pane 的激活在 update 分支完成，守卫因此按点击 pane 判定。
    pub(super) fn extract_archive_from_middle_click(
        &mut self,
        archive: PathBuf,
    ) -> iced::Task<Message> {
        self.context_menu = None;
        if self.is_trash_view || self.current_directory_is_inside_archive() {
            return iced::Task::none();
        }
        // 视图入口按 kind 路由，扩展名判定在状态边界兜底：非压缩包
        // 路径没有解压语义，静默吞掉而不是带着错误请求走完后续流。
        if !file_core::is_supported_archive_path(&archive) {
            return iced::Task::none();
        }
        iced::Task::perform(
            async move {
                let single_root = file_core::single_root_member_name(&archive)
                    .await
                    .map_err(|error| error.to_string());
                (archive, single_root)
            },
            move |(archive, single_root)| Message::SmartExtractDestinationResolved {
                archive,
                single_root,
            },
        )
    }

    /// 智能解压判定回流：按单根/多根计算目的地后启动既有解压流。
    pub(super) fn accept_smart_extract_destination(
        &mut self,
        archive: PathBuf,
        single_root: Result<Option<String>, String>,
    ) -> iced::Task<Message> {
        let single_root_name = match single_root {
            Ok(single_root) => single_root,
            Err(error) => {
                // 列成员失败（坏包/无 7z 命令）按多根语义落到包名文件夹，
                // 让后续解压环节给出结构化错误，而不是静默丢弃。
                tracing::warn!(
                    target: "app_ui::archive_extraction",
                    event = "smart_extract_listing_failed",
                    error = %error,
                    "smart extract fell back to the archive-named folder"
                );
                None
            }
        };
        let destination = smart_extraction_destination(&archive, single_root_name, |candidate| {
            candidate.is_dir()
        });
        self.begin_archive_extraction(archive, destination)
    }

    /// 右键「解压到 <包名>/」入口：无条件包一层后走既有解压流。
    pub(super) fn extract_selected_to_archive_folder(&mut self) -> iced::Task<Message> {
        self.context_menu = None;
        if self.is_trash_view || self.current_directory_is_inside_archive() {
            return iced::Task::none();
        }
        let archives = self.archive_selection_paths();
        if archives.is_empty() {
            return iced::Task::none();
        }
        let commands = archives
            .into_iter()
            .map(|archive| {
                let destination =
                    archive_folder_destination(&archive, |candidate| candidate.is_dir());
                self.begin_archive_extraction(archive, destination)
            })
            .collect::<Vec<_>>();
        iced::Task::batch(commands)
    }

    fn begin_archive_extraction(
        &mut self,
        archive: PathBuf,
        destination: PathBuf,
    ) -> iced::Task<Message> {
        let request = match ArchiveExtractionRequest::from_archive_path(&archive, None) {
            Ok(request) => request.with_destination(destination),
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
    use crate::model::BrowserPaneId;

    fn no_existing(_: &Path) -> bool {
        false
    }

    #[test]
    fn single_root_extracts_into_archive_parent_directory() {
        let destination = smart_extraction_destination(
            Path::new("/home/u/docs/photos.zip"),
            Some("photos".to_owned()),
            no_existing,
        );
        assert_eq!(destination, PathBuf::from("/home/u/docs"));
    }

    #[test]
    fn scattered_members_create_archive_named_folder_beside_archive() {
        let destination =
            smart_extraction_destination(Path::new("/home/u/docs/photos.zip"), None, no_existing);
        assert_eq!(destination, PathBuf::from("/home/u/docs/photos"));
    }

    #[test]
    fn duplicate_destination_appends_counter() {
        let destination =
            smart_extraction_destination(Path::new("/home/u/docs/photos.zip"), None, |candidate| {
                candidate
                    .file_name()
                    .is_none_or(|name| name != "photos (3)")
            });
        assert_eq!(destination, PathBuf::from("/home/u/docs/photos (3)"));
    }

    #[test]
    fn archive_folder_destination_always_wraps_a_layer() {
        let destination =
            archive_folder_destination(Path::new("/home/u/docs/photos.zip"), no_existing);
        assert_eq!(destination, PathBuf::from("/home/u/docs/photos"));
    }

    fn browser() -> FileBrowser {
        FileBrowser::new(crate::config::default_user_config()).0
    }

    #[test]
    fn extract_archive_from_middle_click_noops_in_trash_view() {
        let mut browser = browser();
        browser.is_trash_view = true;

        drop(browser.extract_archive_from_middle_click(PathBuf::from("/workspace/bundle.zip")));

        // 与右键智能解压同规：回收站视图里中键解压不成立，无动作；
        // 菜单清空与守卫判定同属入口契约，守卫拒绝时也已生效。
        assert!(browser.archive_extraction.is_none());
        assert!(browser.context_menu.is_none());
    }

    #[test]
    fn extract_archive_from_middle_click_noops_inside_archive() {
        let mut browser = browser();
        // 把当前目录指到压缩包路径上即可让 archive_path_identity 判定
        // 进入包内/包根语义，无需真实文件存在。
        browser.current_dir = PathBuf::from("/workspace/bundle.zip");

        drop(browser.extract_archive_from_middle_click(PathBuf::from("/workspace/bundle.zip")));

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
