use file_core::FileKind;
use iced::{Element, Task};

use super::FileBrowser;
use crate::model::{Message, PreviewSize, PreviewWindowProfile};
use crate::right_dock::DockPanel;

/// 面板内容区四周留白。
pub(crate) const PANEL_CONTENT_PADDING: f32 = 12.0;
/// 内容区宽度下限;拖窄到极限时保证媒体与滚动区仍有可读尺寸。
const MIN_PANEL_CONTENT_WIDTH: f32 = 120.0;
/// 高度估算下限,防止极端小窗时出现负值尺寸。
const MIN_PANEL_CONTENT_HEIGHT: f32 = 160.0;

// 会话呈现面类型已随预览状态机下沉 bennu-preview（engine 的
// PreviewLoadSurface）；此处 re-export 维持 crate::app::right_preview_panel::
// 调用路径（keyboard_navigation / preview_state / 测试）。
pub(crate) use bennu_preview::engine::PreviewLoadSurface;

impl FileBrowser {
    /// 单点收敛:预览面板显示着 ⇒ preview 状态与目标(活动窗格 `selected`,即最后点选的
    /// 单焦点项)一致；信息面板显示着 ⇒ 信息快照对应目标。唯一调用点是
    /// [`FileBrowser::update`] 主入口末尾,覆盖点击/键盘/删除/改名/tab 切换/窗格切换/
    /// 目录导航,以及启动偏好加载(同样经由消息处理抵达)。面板没显示(栏隐藏、关掉、
    /// 或藏在同格别的标签后面)时什么都不读。
    pub(crate) fn sync_right_dock_panels(&mut self) -> Task<Message> {
        self.sync_quick_actions_panel();
        let info = self.sync_info_panel();
        Task::batch([self.sync_preview_panel(), info])
    }

    /// 目录不进面板预览:文件夹树与侧栏视觉重复,目录目标预览区回空态;Space 独立窗口的
    /// 目录树预览不受影响。
    fn sync_preview_panel(&mut self) -> Task<Message> {
        if !self.right_dock.layout().shows(DockPanel::Preview) {
            return Task::none();
        }
        let Some(path) = self.tab().selected.clone() else {
            // 目标为空:清掉内容显示空态。clear_preview 自带固定守卫,
            // pin 住的独立窗口预览不会被此处误清。
            if self.preview_engine.preview.is_some() {
                self.preview_engine.clear_preview();
            }
            return Task::none();
        };
        // 包内成员不进面板预览:虚拟路径读不了文件,按只读浏览语义
        // 与目录目标同款清空为空态,不发起加载也不弹错。
        // 归档文件本身是 BrowsingRoot 但仍是真实文件,照常进面板预览。
        let kind = self.entry_kind(&path).unwrap_or(FileKind::Other);
        if kind == FileKind::Directory
            || file_core::archive_path_identity(&path)
                == file_core::ArchivePathIdentity::InsideArchive
        {
            // 必须连同“已一致”判定一起登记,否则收敛点每轮 update 都会重复清空/重载。
            self.preview_engine.preview_shown_path = Some(path);
            if self.preview_engine.preview.is_some() {
                self.preview_engine.clear_preview();
            }
            return Task::none();
        }
        // 必须同时校验 preview 非空:clear_preview 不重置 preview_shown_path,
        // “选中 A → 点空白清空 → 再选中 A”时路径相等但内容已丢失,必须重载。
        if self.preview_engine.preview.is_some()
            && self.preview_engine.preview_shown_path.as_deref() == Some(path.as_path())
        {
            return Task::none();
        }
        self.preview_engine.preview_load_surface = PreviewLoadSurface::RightDockedPanel;
        // open_preview_for_resolved_path 不登记目标路径(Space 流程由
        // open_preview 预先写入);面板流程在此等价登记,保证会话目标可查。
        self.preview_engine.preview_shown_path = Some(path.clone());
        self.open_preview_for_resolved_path(path, kind)
    }

    /// 预览面板内容视口的估算尺寸:宽度精确(决定媒体适配与分页文档页宽),
    /// 高度允许 ±数 px 误差(只影响垂直居中留白与滚动高度估算)。
    pub(crate) fn right_preview_panel_viewport(&self) -> PreviewSize {
        let width = (self.right_dock_effective_width()
            - super::right_dock::RESIZE_HANDLE_WIDTH
            - PANEL_CONTENT_PADDING * 2.0)
            .max(MIN_PANEL_CONTENT_WIDTH);
        let height = self
            .right_dock_panel_area(DockPanel::Preview)
            .map_or(0.0, |area| area.height - PANEL_CONTENT_PADDING * 2.0)
            .max(MIN_PANEL_CONTENT_HEIGHT);
        PreviewSize { width, height }
    }

    /// 面板内容:无预览状态时显示空态;有则复用独立窗口的渲染管线,
    /// 仅把视口尺寸换成面板自己的估算值。控件透明度归属预览窗口的指针跟踪状态,
    /// 面板会话等不到窗口底部的悬停显隐事件,透明度改由引擎按面板规则给
    /// (音视频/动图全显,图片指针在图上才显)。
    pub(crate) fn right_preview_panel_content(&self) -> Element<'_, Message> {
        let preview_engine = &self.preview_engine;
        if preview_engine.preview.is_none() {
            return container_centered(bennu_preview::panel_text::readable_text(tr!(
                "Select an item to preview"
            )));
        }
        crate::view::view_preview_window(
            preview_engine.preview.as_ref(),
            preview_engine.text_preview_document.as_ref(),
            preview_engine.sqlite_preview.as_ref(),
            self.right_preview_panel_viewport(),
            PreviewLoadSurface::RightDockedPanel,
            preview_engine.image_view(),
            preview_engine.audio_preview.as_ref(),
            preview_engine.audio_analysis.as_ref(),
            preview_engine.video_preview.as_ref(),
            preview_engine.media_settings,
            preview_engine.docked_panel_controls_opacity(),
            self.operation_progress_animation_frame,
            self.user_config.text_preview_view,
            bennu_preview::text_highlight::HighlightPalette::for_theme(
                &self.theme(self.main_window),
            ),
        )
    }

    /// 加载管线中的窗口呈现步骤:独立窗口会话照常确保/聚焦窗口,
    /// 面板会话原样跳过,窗口保持关闭、不缩放、不抢焦点。
    pub(super) fn preview_window_presentation_command(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> Task<Message> {
        self.preview_engine
            .preview_window_presentation_command(profile)
            .map(Message::Preview)
    }

    /// 当前预览表面的媒体区视口尺寸:独立窗口会话沿用窗口尺寸,
    /// 面板会话用面板估算值。预览内容的排版与缩放钳制共用这一个来源,
    /// 保证视图渲染基准与交互消息的处理基准一致。
    pub(super) fn preview_surface_viewport(&self) -> PreviewSize {
        match self.preview_engine.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.preview_engine.preview_size,
            PreviewLoadSurface::RightDockedPanel => self.right_preview_panel_viewport(),
        }
    }
}

fn container_centered<'a>(
    content: iced::widget::Text<'a, iced::Theme, iced::Renderer>,
) -> Element<'a, Message> {
    iced::widget::container(content)
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .center_x(iced::Length::Fill)
        .center_y(iced::Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::*;
    use crate::config;
    use crate::model::{PreviewContent, PreviewState};

    fn test_file_entry(path: &Path) -> DirectoryEntry {
        DirectoryEntry::new(
            path.to_path_buf(),
            FileKind::File,
            EntryMetadata {
                len: 0,
                modified: None,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    fn panel_open_browser() -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.right_dock.toggle_visible();
        browser
    }

    #[test]
    fn sync_loads_selected_target_only_while_panel_is_open() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&path)].into();
        browser.tab_mut().selected = Some(path.clone());

        drop(browser.sync_right_dock_panels());

        assert_eq!(
            browser.preview_engine.preview_shown_path,
            Some(path.clone())
        );
        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(_))
        ));
        assert_eq!(
            browser.preview_engine.preview_load_surface,
            PreviewLoadSurface::RightDockedPanel
        );

        // 面板关闭后同一收敛点必须原样空操作,预览不得被改动。
        let mut closed_browser = browser;
        closed_browser.right_dock.toggle_visible();
        drop(closed_browser.sync_right_dock_panels());
        assert_eq!(closed_browser.preview_engine.preview_shown_path, Some(path));
    }

    #[test]
    fn sync_with_no_target_clears_stale_preview_content() {
        let mut browser = panel_open_browser();
        browser.preview_engine.preview = Some(PreviewState::Error("stale".to_owned()));
        browser.preview_engine.preview_shown_path = Some(PathBuf::from("/workspace/report.txt"));

        drop(browser.sync_right_dock_panels());

        assert!(browser.preview_engine.preview.is_none());
    }

    #[test]
    fn sync_reloads_target_after_content_was_cleared_at_same_path() {
        // 回归:clear_preview 不重置 preview_shown_path;“选中 A → 点空白
        // 清空 → 再选中 A”时若只比较路径会误判“已一致”,面板停留在空态。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&path)].into();
        browser.preview_engine.preview_shown_path = Some(path.clone());
        browser.preview_engine.preview = None;
        browser.tab_mut().selected = Some(path.clone());

        drop(browser.sync_right_dock_panels());

        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(_))
        ));
    }

    #[test]
    fn directory_target_skips_preview_and_stays_converged() {
        // 目录不进面板预览(文件夹树与侧栏视觉重复):预览区回空态,
        // 信息区仍显示目录条目;目标必须登记,否则每轮 update 重复清空。
        let folder = PathBuf::from("/workspace/Projects");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![DirectoryEntry::new(
            folder.clone(),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        )]
        .into();
        browser.tab_mut().selected = Some(folder.clone());
        browser.preview_engine.preview = Some(PreviewState::Error("previous".to_owned()));

        drop(browser.sync_right_dock_panels());

        assert_eq!(
            browser.preview_engine.preview_shown_path,
            Some(folder.clone())
        );
        assert!(browser.preview_engine.preview.is_none());

        // 再次收敛必须原样空操作(不重载、不清守卫外的状态)。
        drop(browser.sync_right_dock_panels());
        assert!(browser.preview_engine.preview.is_none());
        assert_eq!(browser.preview_engine.preview_shown_path, Some(folder));
    }

    #[test]
    fn file_target_after_directory_target_loads_again() {
        let folder = PathBuf::from("/workspace/Projects");
        let file = PathBuf::from("/workspace/Projects/main.rs");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![
            DirectoryEntry::new(
                folder.clone(),
                FileKind::Directory,
                EntryMetadata::default(),
                false,
                false,
                false,
            ),
            test_file_entry(&file),
        ]
        .into();
        browser.tab_mut().selected = Some(folder);
        drop(browser.sync_right_dock_panels());
        assert!(browser.preview_engine.preview.is_none());

        browser.tab_mut().selected = Some(file.clone());
        drop(browser.sync_right_dock_panels());
        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(_))
        ));
    }

    #[test]
    fn sync_keeps_ready_preview_when_target_already_matches() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.preview_engine.preview_shown_path = Some(path.clone());
        browser.preview_engine.preview = Some(PreviewState::Ready(PreviewContent::Folder(
            Box::new(bennu_preview::folder_preview::FolderPreview::new(
                PathBuf::from("/workspace"),
                Vec::new(),
                file_core::ScanOptions::default(),
            )),
        )));
        browser.tab_mut().selected = Some(path);

        drop(browser.sync_right_dock_panels());

        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Ready(PreviewContent::Folder(_)))
        ));
    }

    #[test]
    fn a_preview_behind_another_tab_loads_nothing_until_it_is_shown() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&path)].into();
        browser.tab_mut().selected = Some(path.clone());
        // 信息叠进预览那格并成为当前标签：预览藏在后面。
        browser
            .right_dock
            .move_panel(DockPanel::Info, crate::right_dock::DropTarget::Join(0));

        drop(browser.sync_right_dock_panels());
        assert!(browser.preview_engine.preview.is_none());

        browser.right_dock.select(DockPanel::Preview);
        drop(browser.sync_right_dock_panels());
        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(_))
        ));
    }

    #[test]
    fn selected_multi_click_focus_is_the_convergence_target() {
        // “最后点选的那个”由 selection 层的 selected 单焦点语义保证;
        // 这里只锁住收敛点读取的就是 selected 而非多选集合。
        let first = PathBuf::from("/workspace/a.txt");
        let second = PathBuf::from("/workspace/b.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&first), test_file_entry(&second)].into();
        *browser.tab_mut().selected_paths = HashSet::from([first.clone(), second.clone()]);
        browser.tab_mut().selected = Some(second.clone());

        drop(browser.sync_right_dock_panels());

        assert_eq!(browser.preview_engine.preview_shown_path, Some(second));
    }

    #[test]
    fn space_opens_standalone_window_while_panel_session_is_active() {
        // 回归:面板加载也置 preview=Some,Space 的 toggle 判定若借用
        // preview.is_some() 会误判“已显示”而永远打不开独立窗口。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&path)].into();
        browser.tab_mut().selected = Some(path.clone());
        browser.hovered_entry = Some(path.clone());
        drop(browser.sync_right_dock_panels());
        assert!(browser.preview_engine.preview.is_some());
        assert!(browser.preview_engine.preview_window.is_none());

        drop(browser.request_preview());

        assert_eq!(
            browser.preview_engine.preview_load_surface,
            PreviewLoadSurface::StandaloneWindow
        );
        assert!(browser.preview_engine.preview_window.is_some());
        assert_eq!(browser.preview_engine.preview_shown_path, Some(path));
    }

    #[test]
    fn space_closes_active_standalone_preview_window() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().selected = Some(path.clone());
        browser.hovered_entry = Some(path.clone());
        browser.preview_engine.preview_shown_path = Some(path.clone());
        browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
        browser.preview_engine.preview_window = Some(iced::window::Id::unique());

        drop(browser.request_preview());

        assert!(browser.preview_engine.preview_window.is_none());
        assert!(browser.preview_engine.preview.is_none());
    }

    #[test]
    fn space_without_hovered_entry_opens_selected_preview() {
        // 键盘选中即够:纯键盘操作没有 hovered_entry,Space 照样按选中项
        // 开独立窗口;面板会话(无独立窗口)不被清空,而是切到独立窗口。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.tab_mut().entries = vec![test_file_entry(&path)].into();
        browser.tab_mut().selected = Some(path.clone());
        browser.preview_engine.preview_shown_path = Some(path.clone());
        browser.preview_engine.preview = Some(PreviewState::Loading(path.clone()));

        drop(browser.request_preview());

        assert_eq!(
            browser.preview_engine.preview_load_surface,
            PreviewLoadSurface::StandaloneWindow
        );
        assert!(browser.preview_engine.preview_window.is_some());
        assert_eq!(browser.preview_engine.preview_shown_path, Some(path));
    }
}
