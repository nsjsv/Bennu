use file_core::FileKind;
use iced::{Element, Point, Task};

use super::FileBrowser;
use crate::config;
use crate::model::{Message, PreviewSize, PreviewWindowProfile, ScrollbarRegion};

/// 面板左缘拖拽热区宽度;与终端面板顶部手柄同级观感。
pub(crate) const PANEL_RESIZE_HANDLE_WIDTH: f32 = 6.0;
/// 面板内容区四周留白。
pub(crate) const PANEL_CONTENT_PADDING: f32 = 12.0;
/// 预览/信息区分隔条高度复用窗格分隔条宽度,手感一致。
pub(crate) const PANEL_RATIO_DIVIDER_HEIGHT: f32 =
    crate::model::SPLIT_DIVIDER_WIDTH;
/// 文件信息区保底高度;拖到极限时四行元数据仍完整可读。
const MIN_INFO_AREA_HEIGHT: f32 = 120.0;
/// 面板最宽不得超过窗格区宽度减去浏览器保底区;与侧栏窗口钳制同源。
const MIN_BROWSER_AREA_WIDTH: f32 = crate::config::MIN_COLUMN_WIDTH;
/// 内容区宽度下限;拖窄到极限时保证媒体与滚动区仍有可读尺寸。
const MIN_PANEL_CONTENT_WIDTH: f32 = 120.0;
/// 高度估算下限,防止极端小窗时出现负值尺寸。
const MIN_PANEL_CONTENT_HEIGHT: f32 = 160.0;

/// 当前预览会话由哪个呈现面发起。加载管线的窗口尺寸/聚焦动作只归
/// 独立窗口会话所有;面板会话绝不弹出、缩放或抢占 Space 独立窗口。
/// 异步加载回流(图片尺寸、视频帧、加载失败等)无法从消息参数得知
/// 发起方,因此必须在会话开始时记入状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewLoadSurface {
    StandaloneWindow,
    RightDockedPanel,
}

/// 面板左缘拖拽的起始锚点:指针横坐标与拖拽开始时的面板宽度。
#[derive(Debug, Clone, Copy)]
pub(super) struct RightPreviewPanelResizeDrag {
    cursor_start_x: f32,
    width_start: f32,
}

/// 预览/信息区分隔条拖拽的起始锚点:指针纵坐标与拖拽开始时的预览区比例。
#[derive(Debug, Clone, Copy)]
pub(super) struct RightPreviewPanelRatioDrag {
    cursor_start_y: f32,
    ratio_start: f32,
}

impl FileBrowser {
    pub(crate) fn toggle_right_preview_panel(&mut self) -> Task<Message> {
        self.right_preview_panel_open = !self.right_preview_panel_open;
        if !self.right_preview_panel_open {
            // 关闭即结束进行中的拖拽;预览状态按需求保留,重开立即恢复显示。
            self.right_preview_panel_resize_drag = None;
            self.right_preview_ratio_resize_drag = None;
        }
        self.user_config.right_preview_panel_open = self.right_preview_panel_open;
        self.persist_user_preferences_command()
    }

    /// D1 单点收敛:面板开启 ⇒ preview 状态与预览目标(活动窗格 `selected`,
    /// 即最后点选的单焦点项)一致。唯一调用点是 [`FileBrowser::update`]
    /// 主入口末尾,覆盖点击/键盘/删除/改名/tab 切换/窗格切换/目录导航,
    /// 以及启动偏好加载(同样经由消息处理抵达)。
    ///
    /// 目录不进面板预览:文件夹树与侧栏视觉重复,目录目标只保留信息区
    /// (名称/类型),预览区回空态;Space 独立窗口的目录树预览不受影响。
    pub(crate) fn sync_right_preview_panel(&mut self) -> Task<Message> {
        if !self.right_preview_panel_open {
            return Task::none();
        }
        let Some(path) = self.selected.clone() else {
            // 目标为空:清掉内容显示空态。clear_preview 自带固定守卫,
            // pin 住的独立窗口预览不会被此处误清。
            self.right_preview_panel_info = None;
            if self.preview.is_some() {
                self.clear_preview();
            }
            return Task::none();
        };
        let info_command = self.refresh_right_preview_panel_info(path.clone());
        let kind = self.entry_kind(&path).unwrap_or(FileKind::Other);
        if kind == FileKind::Directory {
            // 目录目标:清空内容并登记目标。必须连同“已一致”判定一起
            // 登记,否则收敛点每轮 update 都会重复清空/重载。
            self.preview_shown_path = Some(path);
            if self.preview.is_some() {
                self.clear_preview();
            }
            return info_command;
        }
        // 必须同时校验 preview 非空:clear_preview 不重置 preview_shown_path,
        // “选中 A → 点空白清空 → 再选中 A”时路径相等但内容已丢失,必须重载。
        if self.preview.is_some() && self.preview_shown_path.as_deref() == Some(path.as_path()) {
            return info_command;
        }
        self.preview_load_surface = PreviewLoadSurface::RightDockedPanel;
        // open_preview_for_resolved_path 不登记目标路径(Space 流程由
        // open_preview 预先写入);面板流程在此等价登记,保证会话目标可查。
        self.preview_shown_path = Some(path.clone());
        Task::batch([
            self.open_preview_for_resolved_path(path, kind),
            info_command,
        ])
    }

    /// 目标路径变化才重新读元数据;快照按 path 门控渲染,过期快照
    /// 在新快照到达前不显示。
    fn refresh_right_preview_panel_info(&mut self, path: std::path::PathBuf) -> Task<Message> {
        if self
            .right_preview_panel_info
            .as_ref()
            .is_some_and(|snapshot| snapshot.path == path)
        {
            return Task::none();
        }
        crate::commands::right_preview_panel_info_command(path)
    }

    pub(super) fn accept_right_preview_panel_info(
        &mut self,
        path: &std::path::Path,
        snapshot: Result<Box<crate::model::RightPreviewPanelInfoSnapshot>, String>,
    ) {
        // 只有快照仍对应当前预览目标才落地,防陈旧回写。
        if self.selected.as_deref() != Some(path) {
            return;
        }
        self.right_preview_panel_info = match snapshot {
            Ok(snapshot) => Some(*snapshot),
            Err(_) => None,
        };
    }

    pub(super) fn start_right_preview_panel_resize_drag(&mut self) -> Task<Message> {
        if !self.right_preview_panel_open {
            return Task::none();
        }
        self.right_preview_panel_resize_drag = Some(RightPreviewPanelResizeDrag {
            cursor_start_x: self.cursor_position.x,
            width_start: self.right_preview_panel_width,
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        Task::none()
    }

    /// 左缘拖拽:指针左移加宽、右移收窄,全程夹取在配置区间与当前
    /// 窗口可容纳宽度之内(与侧栏拖拽同一策略)。
    pub(super) fn update_right_preview_panel_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.right_preview_panel_resize_drag else {
            return;
        };
        self.right_preview_panel_width = self.right_preview_panel_width_for_window(
            drag.width_start - (position.x - drag.cursor_start_x),
        );
    }

    /// 运行期生效宽度:存储宽度再按当前窗口收口。面板与浏览器共享
    /// 一行,超窗的面板会把浏览器区整个挤没(截图回归),因此浏览器
    /// 至少保留 MIN_BROWSER_AREA_WIDTH。
    pub(crate) fn right_preview_panel_effective_width(&self) -> f32 {
        self.right_preview_panel_width_for_window(self.right_preview_panel_width)
    }

    fn right_preview_panel_width_for_window(&self, width: f32) -> f32 {
        let max_width = (self.main_window_width
            - self.sidebar_width
            - MIN_BROWSER_AREA_WIDTH)
            .max(1.0);
        config::normalize_right_preview_panel_width(width).min(max_width)
    }

    pub(super) fn start_right_preview_panel_ratio_resize_drag(&mut self) -> Task<Message> {
        if !self.right_preview_panel_open {
            return Task::none();
        }
        self.right_preview_ratio_resize_drag = Some(RightPreviewPanelRatioDrag {
            cursor_start_y: self.cursor_position.y,
            ratio_start: self.right_preview_panel_effective_preview_ratio(),
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        Task::none()
    }

    /// 分隔条拖拽:指针下移预览区加高、上移压缩,比例先按存储区间夹取,
    /// 读取侧再按当前窗高保底信息区(见 [`Self::right_preview_panel_effective_preview_ratio`])。
    pub(super) fn update_right_preview_panel_ratio_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.right_preview_ratio_resize_drag else {
            return;
        };
        let content_height = self.right_preview_panel_content_height();
        self.right_preview_preview_ratio = config::normalize_right_preview_preview_ratio(
            drag.ratio_start + (position.y - drag.cursor_start_y) / content_height,
        );
    }

    /// 宽度与分隔条拖拽共用的收尾:任一拖拽进行过才写盘一次。
    pub(super) fn finish_right_preview_panel_drag_commands(&mut self) -> Task<Message> {
        let width_drag_active = self.right_preview_panel_resize_drag.take().is_some();
        let ratio_drag_active = self.right_preview_ratio_resize_drag.take().is_some();
        if !width_drag_active && !ratio_drag_active {
            return Task::none();
        }
        self.user_config.right_preview_panel_width = self.right_preview_panel_width;
        self.user_config.right_preview_preview_ratio = self.right_preview_preview_ratio;
        self.persist_user_preferences_command()
    }

    /// 面板内容视口的估算尺寸:宽度精确(决定媒体适配与分页文档页宽),
    /// 高度允许 ±数 px 误差(只影响垂直居中留白与滚动高度估算)。
    pub(crate) fn right_preview_panel_viewport(&self) -> PreviewSize {
        let width = (self.right_preview_panel_effective_width()
            - PANEL_RESIZE_HANDLE_WIDTH
            - PANEL_CONTENT_PADDING * 2.0)
            .max(MIN_PANEL_CONTENT_WIDTH);
        PreviewSize {
            width,
            height: self.right_preview_panel_content_height()
                * self.right_preview_panel_effective_preview_ratio(),
        }
    }

    /// 面板内容总高(信息区 + 分隔条 + 预览区),不含内容留白。
    /// 全窗工具栏顶栏横贯后,面板从顶栏下方开始,两种 chrome 布局
    /// 都要让位顶栏;独立标题栏布局再额外让位应用内标题条。
    pub(crate) fn right_preview_panel_content_height(&self) -> f32 {
        let title_bar_height = match self.user_config.window_controls.layout() {
            crate::model::WindowChromeLayout::SeparateTitleBar => {
                crate::model::WINDOW_TOP_BAR_HEIGHT
            }
            crate::model::WindowChromeLayout::IntegratedNavigation => 0.0,
        };
        // 终端面板展开时占据 height();收起时横贯底部的窄条同样让位。
        let terminal_height =
            if self.terminal_panel.height() >= crate::terminal_panel::view::CELL_HEIGHT {
                self.terminal_panel.height()
            } else {
                crate::terminal_panel::view::BOTTOM_BAR_HEIGHT
            };
        (self.main_window_height
            - title_bar_height
            - crate::model::MAIN_TOOLBAR_ROW_HEIGHT
            - terminal_height
            - PANEL_CONTENT_PADDING * 2.0)
            .max(MIN_PANEL_CONTENT_HEIGHT)
    }

    /// 存储比例经当前窗高的信息区保底夹取后的实际生效值。夹取依赖运行期
    /// 窗高,存储层只挡非法值(非有限/越界),两层职责不混。
    pub(crate) fn right_preview_panel_effective_preview_ratio(&self) -> f32 {
        let content_height = self.right_preview_panel_content_height();
        let max_ratio = ((content_height - MIN_INFO_AREA_HEIGHT - PANEL_RATIO_DIVIDER_HEIGHT)
            / content_height)
            .clamp(0.0, 1.0);
        self.right_preview_preview_ratio.min(max_ratio)
    }

    /// 预览区在总 portion(1000)里的份额;视图用 FillPortion 分割,
    /// 分隔线位置由布局决定,不受高度估算误差影响。
    pub(crate) fn right_preview_panel_preview_portion(&self) -> u16 {
        (self.right_preview_panel_effective_preview_ratio()
            * f32::from(crate::model::SPLIT_PORTION_TOTAL)) as u16
    }

    /// 面板内容:无预览状态时显示空态;有则复用独立窗口的渲染管线,
    /// 仅把视口尺寸换成面板自己的估算值。媒体播放控件固定全显:
    /// 控件透明度归属预览窗口的指针跟踪状态,面板会话永远等不到
    /// 悬停显隐事件,强制复用会让面板视频/动图没有可操作的控件。
    pub(crate) fn right_preview_panel_content(&self) -> Element<'_, Message> {
        if self.preview.is_none() {
            return container_centered(
                crate::typography::localized_text("Select an item to preview").size(14),
            );
        }
        crate::view::view_preview_window(
            self.preview.as_ref(),
            self.text_preview_document.as_ref(),
            self.sqlite_preview.as_ref(),
            self.right_preview_panel_viewport(),
            &self.preview_image_viewport,
            self.audio_preview.as_ref(),
            self.video_preview.as_ref(),
            1.0,
            self.operation_progress_animation_frame,
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewDirectory),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewDirectory),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewArchive),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewArchive),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewDocument),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewDocument),
            self.scrollbar_visibility_for(&ScrollbarRegion::TextPreview),
            self.scrollbar_viewport_for(&ScrollbarRegion::TextPreview),
            self.text_preview_content_height,
            self.scrollbar_visibility_for(&ScrollbarRegion::MarkdownPreview),
            self.scrollbar_viewport_for(&ScrollbarRegion::MarkdownPreview),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewSqliteTables),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewSqliteTables),
            self.scrollbar_visibility_for(&ScrollbarRegion::PreviewSqliteData),
            self.scrollbar_viewport_for(&ScrollbarRegion::PreviewSqliteData),
        )
    }

    /// 加载管线中的窗口呈现步骤:独立窗口会话照常确保/聚焦窗口,
    /// 面板会话原样跳过,窗口保持关闭、不缩放、不抢焦点。
    pub(super) fn preview_window_presentation_command(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> Task<Message> {
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.ensure_preview_window(profile),
            PreviewLoadSurface::RightDockedPanel => Task::none(),
        }
    }

    /// 与 [`Self::preview_window_presentation_command`] 同一边界的异步回流
    /// 形态:窗口缺失时补开,仅对独立窗口会话生效。
    pub(super) fn ensure_preview_window_for_standalone_load(
        &mut self,
        profile: PreviewWindowProfile,
    ) -> Task<Message> {
        if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow
            && self.preview_window.is_none()
        {
            return self.ensure_preview_window(profile);
        }
        Task::none()
    }

    /// 当前预览表面的媒体区视口尺寸:独立窗口会话沿用窗口尺寸,
    /// 面板会话用面板估算值。预览内容的排版与缩放钳制共用这一个来源,
    /// 保证视图渲染基准与交互消息的处理基准一致。
    pub(super) fn preview_surface_viewport(&self) -> PreviewSize {
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.preview_size,
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
    use std::sync::Arc;

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
        browser.right_preview_panel_open = true;
        browser
    }

    #[test]
    fn sync_loads_selected_target_only_while_panel_is_open() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![test_file_entry(&path)]);
        browser.selected = Some(path.clone());

        drop(browser.sync_right_preview_panel());

        assert_eq!(browser.preview_shown_path, Some(path.clone()));
        assert!(matches!(browser.preview, Some(PreviewState::Loading(_))));
        assert_eq!(
            browser.preview_load_surface,
            PreviewLoadSurface::RightDockedPanel
        );

        // 面板关闭后同一收敛点必须原样空操作,预览不得被改动。
        let mut closed_browser = browser;
        closed_browser.right_preview_panel_open = false;
        drop(closed_browser.sync_right_preview_panel());
        assert_eq!(closed_browser.preview_shown_path, Some(path));
    }

    #[test]
    fn sync_with_no_target_clears_stale_preview_content() {
        let mut browser = panel_open_browser();
        browser.preview = Some(PreviewState::Error("stale".to_owned()));
        browser.preview_shown_path = Some(PathBuf::from("/workspace/report.txt"));

        drop(browser.sync_right_preview_panel());

        assert!(browser.preview.is_none());
    }

    #[test]
    fn sync_reloads_target_after_content_was_cleared_at_same_path() {
        // 回归:clear_preview 不重置 preview_shown_path;“选中 A → 点空白
        // 清空 → 再选中 A”时若只比较路径会误判“已一致”,面板停留在空态。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![test_file_entry(&path)]);
        browser.preview_shown_path = Some(path.clone());
        browser.preview = None;
        browser.selected = Some(path.clone());

        drop(browser.sync_right_preview_panel());

        assert!(matches!(browser.preview, Some(PreviewState::Loading(_))));
    }

    #[test]
    fn directory_target_skips_preview_and_stays_converged() {
        // 目录不进面板预览(文件夹树与侧栏视觉重复):预览区回空态,
        // 信息区仍显示目录条目;目标必须登记,否则每轮 update 重复清空。
        let folder = PathBuf::from("/workspace/Projects");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![DirectoryEntry::new(
            folder.clone(),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        )]);
        browser.selected = Some(folder.clone());
        browser.preview = Some(PreviewState::Error("previous".to_owned()));

        drop(browser.sync_right_preview_panel());

        assert_eq!(browser.preview_shown_path, Some(folder.clone()));
        assert!(browser.preview.is_none());

        // 再次收敛必须原样空操作(不重载、不清守卫外的状态)。
        drop(browser.sync_right_preview_panel());
        assert!(browser.preview.is_none());
        assert_eq!(browser.preview_shown_path, Some(folder));
    }

    #[test]
    fn file_target_after_directory_target_loads_again() {
        let folder = PathBuf::from("/workspace/Projects");
        let file = PathBuf::from("/workspace/Projects/main.rs");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![
            DirectoryEntry::new(
                folder.clone(),
                FileKind::Directory,
                EntryMetadata::default(),
                false,
                false,
                false,
            ),
            test_file_entry(&file),
        ]);
        browser.selected = Some(folder);
        drop(browser.sync_right_preview_panel());
        assert!(browser.preview.is_none());

        browser.selected = Some(file.clone());
        drop(browser.sync_right_preview_panel());
        assert!(matches!(browser.preview, Some(PreviewState::Loading(_))));
    }

    #[test]
    fn sync_keeps_ready_preview_when_target_already_matches() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.preview_shown_path = Some(path.clone());
        browser.preview = Some(PreviewState::Ready(PreviewContent::Directory {
            entries: Vec::new(),
        }));
        browser.selected = Some(path);

        drop(browser.sync_right_preview_panel());

        assert!(matches!(
            browser.preview,
            Some(PreviewState::Ready(PreviewContent::Directory { .. }))
        ));
    }

    #[test]
    fn resize_drag_clamps_width_to_configured_bounds() {
        let mut browser = panel_open_browser();
        browser.right_preview_panel_width = 320.0;
        browser.cursor_position = Point::new(600.0, 0.0);
        drop(browser.start_right_preview_panel_resize_drag());
        // 指针右移 400px:宽度应收窄并夹在下限。
        browser.update_right_preview_panel_resize_drag(Point::new(1000.0, 0.0));
        assert_eq!(
            browser.right_preview_panel_width,
            config::MIN_RIGHT_PREVIEW_PANEL_WIDTH
        );
        // 指针大幅左移:宽度顶到上限。
        browser.update_right_preview_panel_resize_drag(Point::new(-600.0, 0.0));
        assert_eq!(
            browser.right_preview_panel_width,
            config::MAX_RIGHT_PREVIEW_PANEL_WIDTH
        );
    }

    #[test]
    fn finished_resize_drag_persists_width_into_user_config() {
        let mut browser = panel_open_browser();
        browser.right_preview_panel_width = 500.0;
        drop(browser.start_right_preview_panel_resize_drag());

        drop(browser.finish_right_preview_panel_drag_commands());

        assert_eq!(browser.user_config.right_preview_panel_width, 500.0);
        assert!(browser.right_preview_panel_resize_drag.is_none());
    }

    #[test]
    fn ratio_drag_clamps_to_configured_bounds() {
        let mut browser = panel_open_browser();
        browser.right_preview_preview_ratio = 0.7;
        browser.cursor_position = Point::new(0.0, 400.0);
        drop(browser.start_right_preview_panel_ratio_resize_drag());
        // 指针大幅上移:比例压到存储下限。
        browser.update_right_preview_panel_ratio_resize_drag(Point::new(0.0, -100.0));
        assert_eq!(
            browser.right_preview_preview_ratio,
            config::MIN_RIGHT_PREVIEW_PREVIEW_RATIO
        );
        // 指针大幅下移:比例顶到存储上限。
        browser.update_right_preview_panel_ratio_resize_drag(Point::new(0.0, 1_000.0));
        assert_eq!(
            browser.right_preview_preview_ratio,
            config::MAX_RIGHT_PREVIEW_PREVIEW_RATIO
        );
    }

    #[test]
    fn effective_ratio_keeps_info_area_minimum_height() {
        // 存储比例 1.0 时,小窗高度下信息区保底 120px + 分隔条必须留出。
        let mut browser = panel_open_browser();
        browser.right_preview_preview_ratio = 1.0;
        browser.main_window_height = 400.0;
        let ratio = browser.right_preview_panel_effective_preview_ratio();
        let content_height = browser.right_preview_panel_content_height();
        assert!(content_height * ratio <= content_height - 120.0);
    }

    #[test]
    fn finished_ratio_drag_persists_ratio_into_user_config() {
        let mut browser = panel_open_browser();
        browser.right_preview_preview_ratio = 0.55;
        drop(browser.start_right_preview_panel_ratio_resize_drag());

        drop(browser.finish_right_preview_panel_drag_commands());

        assert_eq!(browser.user_config.right_preview_preview_ratio, 0.55);
        assert!(browser.right_preview_ratio_resize_drag.is_none());
    }

    #[test]
    fn finish_without_any_drag_skips_preference_write() {
        let mut browser = panel_open_browser();
        drop(browser.finish_right_preview_panel_drag_commands());
        // 无拖拽时不得把运行值误固化进配置(拖拽外的运行期夹取不落盘)。
        assert_eq!(
            browser.user_config.right_preview_preview_ratio,
            config::DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO
        );
    }

    #[test]
    fn selected_multi_click_focus_is_the_convergence_target() {
        // “最后点选的那个”由 selection 层的 selected 单焦点语义保证;
        // 这里只锁住收敛点读取的就是 selected 而非多选集合。
        let first = PathBuf::from("/workspace/a.txt");
        let second = PathBuf::from("/workspace/b.txt");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![test_file_entry(&first), test_file_entry(&second)]);
        browser.selected_paths = HashSet::from([first.clone(), second.clone()]);
        browser.selected = Some(second.clone());

        drop(browser.sync_right_preview_panel());

        assert_eq!(browser.preview_shown_path, Some(second));
    }

    #[test]
    fn space_opens_standalone_window_while_panel_session_is_active() {
        // 回归:面板加载也置 preview=Some,Space 的 toggle 判定若借用
        // preview.is_some() 会误判“已显示”而永远打不开独立窗口。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![test_file_entry(&path)]);
        browser.selected = Some(path.clone());
        browser.hovered_entry = Some(path.clone());
        drop(browser.sync_right_preview_panel());
        assert!(browser.preview.is_some());
        assert!(browser.preview_window.is_none());

        drop(browser.request_preview());

        assert_eq!(
            browser.preview_load_surface,
            PreviewLoadSurface::StandaloneWindow
        );
        assert!(browser.preview_window.is_some());
        assert_eq!(browser.preview_shown_path, Some(path));
    }

    #[test]
    fn space_closes_active_standalone_preview_window() {
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.selected = Some(path.clone());
        browser.hovered_entry = Some(path.clone());
        browser.preview_shown_path = Some(path.clone());
        browser.preview = Some(PreviewState::Loading(path.clone()));
        browser.preview_window = Some(iced::window::Id::unique());

        drop(browser.request_preview());

        assert!(browser.preview_window.is_none());
        assert!(browser.preview.is_none());
    }

    #[test]
    fn space_without_hovered_entry_opens_selected_preview() {
        // 键盘选中即够:纯键盘操作没有 hovered_entry,Space 照样按选中项
        // 开独立窗口;面板会话(无独立窗口)不被清空,而是切到独立窗口。
        let path = PathBuf::from("/workspace/report.txt");
        let mut browser = panel_open_browser();
        browser.entries = Arc::new(vec![test_file_entry(&path)]);
        browser.selected = Some(path.clone());
        browser.preview_shown_path = Some(path.clone());
        browser.preview = Some(PreviewState::Loading(path.clone()));

        drop(browser.request_preview());

        assert_eq!(
            browser.preview_load_surface,
            PreviewLoadSurface::StandaloneWindow
        );
        assert!(browser.preview_window.is_some());
        assert_eq!(browser.preview_shown_path, Some(path));
    }
}
