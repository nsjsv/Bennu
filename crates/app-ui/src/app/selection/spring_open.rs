use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::Task;

use super::super::FileBrowser;
use crate::model::{
    BrowserPaneId, BrowserViewMode, FileDragDropIntent, FileDragSpringHover, FileDragSpringSource,
    FileDropSessionPhase, Message, NavigationMode,
};

/// 悬停多久才自动打开;固定值,不做设置项。对齐 macOS Finder 的
/// springing delay 默认值 0.5s(Windows 导航窗格自动展开同样在半秒内)。
pub(crate) const FILE_DRAG_SPRING_OPEN_DELAY: Duration = Duration::from_millis(500);
/// 候选存在期间的收敛 tick:同时驱动进度环重绘与到点触发。
pub(crate) const FILE_DRAG_SPRING_OPEN_TICK_INTERVAL: Duration = Duration::from_millis(50);

impl FileBrowser {
    /// spring 只在拖拽进行中成立:内部拖拽实际进入 Dragging,或落点
    /// 会话处于悬停(内部原生交接 / 外部拖入)。
    fn file_drag_spring_is_armed(&self) -> bool {
        self.file_drag
            .as_ref()
            .is_some_and(|drag| drag.is_dragging())
            || self
                .file_drop_session
                .as_ref()
                .is_some_and(|session| session.phase == FileDropSessionPhase::Hovering)
    }

    /// 移动意图下落点是拖拽源自身或其子树会造成循环移动(落地被整批
    /// 拒绝),spring 必须抑制;复制/链接无此限制,Ctrl 拖进自身子树合法。
    fn file_drag_spring_open_allowed(&self, directory: &Path) -> bool {
        let Some(drag) = &self.file_drag else {
            return true;
        };
        let cycle_risk = drag
            .sources
            .iter()
            .any(|source| source == directory || directory.starts_with(source));
        !cycle_risk
            || self.file_drag_drop_intent(&drag.sources, directory) != FileDragDropIntent::Move
    }

    /// 面包屑 spring 的资格:悬停 pane 的当前目录段落导航是空操作不
    /// 触发;回收站 pane 在 note 的拖放资格 gate 处已被排除。多栏视图
    /// 允许——导航回祖先即重置栏链(拖拽栏链快照由导航路径清空,渲染
    /// 回到活链)。按候选 pane 判定:跨 pane 悬停时"当前目录"属于被
    /// 悬停的 pane,不是活动 pane。
    fn file_drag_spring_breadcrumb_allowed(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
    ) -> bool {
        self.pane_view(pane_id)
            .is_some_and(|pane| directory != pane.current_dir)
    }

    /// 所有悬停状态变化后的统一收敛点。`hover` 是 (pane, 目录路径, 来源):
    /// iced hover 路径由 `handle_entry_hovered` / `handle_file_drag_entry_hovered_in_pane`
    /// 传入目录条目,原生 dnd 路径由布局快照命中结果传入条目或面包屑
    /// 段落,面包屑消息由 `handle_breadcrumb_drop_target_hovered` 传入。
    /// 传 None(移开/非目录条目/空白/侧边栏/标签页/清理)即清除候选。
    ///
    /// 不变量:候选存在 ⟺ 拖拽中且被悬停 pane(任意接受拖放的 pane,
    /// 含跨 pane 悬停)悬停着 spring 资格目录;同目录同 pane 同来源的
    /// 重复收敛保持计时起点,其余重置。
    pub(super) fn note_file_drag_spring_hover(
        &mut self,
        hover: Option<(BrowserPaneId, PathBuf, FileDragSpringSource)>,
    ) {
        let candidate = hover
            .filter(|(pane_id, _, _)| self.pane_accepts_file_drag(*pane_id))
            .filter(|_| self.file_drag_spring_is_armed())
            .filter(|(pane_id, directory, source)| match source {
                FileDragSpringSource::Entry => self.file_drag_spring_open_allowed(directory),
                FileDragSpringSource::Breadcrumb => {
                    self.file_drag_spring_open_allowed(directory)
                        && self.file_drag_spring_breadcrumb_allowed(*pane_id, directory)
                }
            })
            .map(|(pane_id, directory, source)| FileDragSpringHover {
                directory,
                pane_id,
                source,
                since: Instant::now(),
                fired: false,
            });
        let unchanged = match (&self.file_drag_spring_hover, &candidate) {
            (Some(current), Some(next)) => {
                current.directory == next.directory
                    && current.pane_id == next.pane_id
                    && current.source == next.source
            }
            _ => false,
        };
        if !unchanged {
            self.file_drag_spring_hover = candidate;
        }
    }

    /// 到点触发:列表/大图直接进目录(与双击同路径,历史入栈),多栏
    /// 在下一栏打开(与双击/按住打开同路径)。跨 pane 候选先激活被悬停
    /// pane——导航状态(current_dir/历史/多栏链)属于该 pane,激活与
    /// 点击进入同路。打开属于拖拽中途的目录变化,落点布局过期由 stale
    /// 标记置位、后续 tick 消化重测(导航加载完成再置一次),否则松手
    /// 会按旧快照解析落点。
    ///
    /// 触发只置 fired 不清候选:悬停未离开时布局重测会原样重建同一
    /// 目录的候选,清掉会导致每过一个阈值重复触发;同一次悬停仅触发
    /// 一次由 fired 状态表达,移开(note 收到 None)才允许重新计时。
    pub(in crate::app) fn handle_file_drag_spring_open_tick(&mut self) -> Task<Message> {
        let Some(current) = self.file_drag_spring_hover.clone() else {
            return Task::none();
        };
        if !self.file_drag_spring_is_armed() {
            // 拖拽会话已终结:候选是悬停态的衍生,滞留即清(下个 tick 兜底)。
            self.file_drag_spring_hover = None;
            return Task::none();
        }
        // 条目内容变化(spring 导航加载完成/watcher/元数据落地)后布局
        // 过期:先重测,等新布局 Ready 后 refresh 恢复悬停高亮与链式候选。
        if self.file_drop_layout_stale {
            self.file_drop_layout_stale = false;
            return self.remeasure_active_file_drop_layout();
        }
        // 候选 pane 在拖拽中途消失(窗格合并等):候选随 pane 失效。
        if !self.pane_accepts_file_drag(current.pane_id) {
            self.file_drag_spring_hover = None;
            return Task::none();
        }
        if current.fired || current.since.elapsed() < FILE_DRAG_SPRING_OPEN_DELAY {
            return Task::none();
        }
        let mut fired = current.clone();
        fired.fired = true;
        self.file_drag_spring_hover = Some(fired);
        if current.pane_id != self.active_pane_id() {
            self.activate_pane(current.pane_id);
        }
        let columns_mode = self
            .pane_view(current.pane_id)
            .is_some_and(|pane| pane.view_mode == BrowserViewMode::Columns);
        match current.source {
            // 面包屑:直接按记录历史导航回该级,继续拖(面包屑目录不在
            // 条目集里,不能走 activate_path)。
            FileDragSpringSource::Breadcrumb => {
                self.navigate_to(current.directory, NavigationMode::RecordHistory)
            }
            FileDragSpringSource::Entry if columns_mode => {
                // 先延伸 deepest 并写回链快照:open_column_for_directory 内部
                // 的 focus_latest_column 数的是拖拽中的冻结快照,顺序反了会
                // 滚不到新栏。
                self.set_deepest_open_column_directory(Some(current.directory.clone()));
                self.refresh_file_drag_column_chain_snapshot(current.pane_id);
                self.open_column_for_directory(current.directory)
            }
            FileDragSpringSource::Entry => self.activate_path(current.directory),
        }
    }

    /// 拖拽期间多栏链由 `source_column_directories` 快照冻结(拖拽预览
    /// 与落点稳定的不变量);spring 开栏是唯一的拖拽中途扩链,且只在
    /// 链尾追加(deepest 延伸),已有栏位置与预览偏移不受影响——把重算
    /// 后的链写回快照,新栏才会出现在拖拽中的视图里。
    fn refresh_file_drag_column_chain_snapshot(&mut self, pane_id: BrowserPaneId) {
        let snapshot_applies = self.pane_view(pane_id).is_some_and(|pane| {
            self.file_drag.as_ref().is_some_and(|drag| {
                drag.source_pane_id == pane_id
                    && drag.source_tab_id == pane.active_tab_id
                    && !drag.column_directories_snapshot.is_empty()
            })
        });
        if !snapshot_applies {
            return;
        }
        // 不能用 column_directories(self):拖拽中它返回的正是冻结快照,
        // 写回等于没变。按同一展开规则取 deepest 更新后的活链。
        let mut chain = vec![self.current_dir.clone()];
        if let Some(open_directory) = &self.deepest_open_column_directory {
            crate::three_column_view::append_column_directory_chain(
                &mut chain,
                &self.current_dir,
                open_directory,
            );
        }
        if let Some(drag) = &mut self.file_drag {
            drag.column_directories_snapshot = chain;
        }
    }

    /// 进度环:只有候选目录本身的条目画环,进度 = 悬停时长 / 阈值。
    /// 已触发(fired)、拖拽已结束、面包屑候选(无图标挂点,靠落点
    /// 高亮与短延迟反馈)不画环。
    pub(crate) fn file_drag_spring_open_progress(&self, path: &Path) -> Option<f32> {
        if !self.file_drag_spring_is_armed() {
            return None;
        }
        let current = self.file_drag_spring_hover.as_ref()?;
        if current.fired
            || current.source != FileDragSpringSource::Entry
            || current.directory != path
        {
            return None;
        }
        let fraction =
            current.since.elapsed().as_secs_f32() / FILE_DRAG_SPRING_OPEN_DELAY.as_secs_f32();
        Some(fraction.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};
    use iced::Point;

    use super::super::super::FileBrowser;
    use super::FILE_DRAG_SPRING_OPEN_DELAY;
    use crate::config;
    use crate::model::{
        BrowserPaneId, BrowserPaneLayout, BrowserViewMode, FileDragSpringSource,
        FileDragStationaryAction, SplitAxis,
    };

    fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            kind,
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

    /// 单 pane 列表视图:/workspace 下有 dir_a、dir_b 两个目录和 file.txt。
    fn spring_browser() -> (FileBrowser, PathBuf, PathBuf, PathBuf, PathBuf) {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let workspace = PathBuf::from("/workspace");
        let dir_a = workspace.join("dir_a");
        let dir_b = workspace.join("dir_b");
        let file = workspace.join("file.txt");
        browser.current_dir = workspace.clone();
        browser.view_mode = BrowserViewMode::List;
        browser.entries = vec![
            test_entry(dir_a.clone(), FileKind::Directory),
            test_entry(dir_b.clone(), FileKind::Directory),
            test_entry(file.clone(), FileKind::File),
        ]
        .into();
        (browser, dir_a, dir_b, file, workspace)
    }

    /// 从按下 dir_a 进入真实拖拽(跨过移动阈值,建立 iced 落点会话)。
    fn start_drag_on(browser: &mut FileBrowser, pressed: &PathBuf) {
        browser.selected = Some(pressed.clone());
        browser.cursor_position = Point::new(10.0, 10.0);
        drop(browser.start_file_drag(
            pressed.clone(),
            FileDragStationaryAction::SelectionOnly,
            Vec::new(),
        ));
        drop(browser.update_file_drag(Point::new(40.0, 10.0)));
    }

    #[test]
    fn hovering_directory_entry_establishes_candidate_and_opens_after_delay() {
        let (mut browser, dir_a, _dir_b, file, workspace) = spring_browser();
        start_drag_on(&mut browser, &file);

        drop(browser.handle_entry_hovered(dir_a.clone()));
        let candidate = browser.file_drag_spring_hover.clone().unwrap();
        assert_eq!(candidate.directory, dir_a);
        assert_eq!(candidate.pane_id, browser.active_pane_id());

        // 未到阈值:不动。
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, workspace);
        assert!(browser.file_drag_spring_hover.is_some());

        // 回拨计时起点后 tick:进入目录,与双击同路径(历史入栈)。
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, dir_a);
        assert_eq!(browser.back_stack, vec![workspace]);
        // 触发后候选保留(fired):悬停未离开不重复触发,环消失。
        assert!(browser.file_drag_spring_hover.as_ref().unwrap().fired);
        assert_eq!(browser.file_drag_spring_open_progress(&dir_a), None);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, dir_a);
    }

    #[test]
    fn hovering_file_entry_or_blank_clears_candidate() {
        let (mut browser, dir_a, _dir_b, file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &file);

        drop(browser.handle_entry_hovered(dir_a.clone()));
        assert!(browser.file_drag_spring_hover.is_some());

        // 悬停文件条目:落点是其父目录而非条目自身,不构成 spring。
        drop(browser.handle_entry_hovered(file.clone()));
        assert!(browser.file_drag_spring_hover.is_none());

        // 悬停空白处(当前目录落点):不是条目悬停,保持清除。
        drop(browser.handle_drop_target_hovered(browser.current_dir.clone()));
        drop(browser.handle_entry_hovered(dir_a.clone()));
        assert!(browser.file_drag_spring_hover.is_some());
        drop(browser.handle_drop_target_hovered(browser.current_dir.clone()));
        assert!(browser.file_drag_spring_hover.is_none());
    }

    #[test]
    fn same_directory_hover_keeps_timer_start_other_directory_resets() {
        let (mut browser, dir_a, dir_b, file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &file);

        drop(browser.handle_entry_hovered(dir_a.clone()));
        let since = browser.file_drag_spring_hover.as_ref().unwrap().since;
        // 同目录重复 hover 事件(移动热路径反复触发):起点不变。
        drop(browser.handle_entry_hovered(dir_a.clone()));
        assert_eq!(
            browser.file_drag_spring_hover.as_ref().unwrap().since,
            since
        );

        // 换目录:重新计时。
        drop(browser.handle_entry_hovered(dir_b.clone()));
        let moved = browser.file_drag_spring_hover.as_ref().unwrap();
        assert_eq!(moved.directory, dir_b);
        assert!(moved.since > since);
    }

    #[test]
    fn dragging_directory_over_itself_does_not_spring() {
        let (mut browser, dir_a, _dir_b, _file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &dir_a);

        // 拖拽源就是 dir_a,悬停它自己:移动意图下循环落点,不 spring。
        drop(browser.handle_entry_hovered(dir_a.clone()));
        assert!(browser.file_drag_spring_hover.is_none());
    }

    #[test]
    fn candidate_dies_with_drag_session() {
        let (mut browser, dir_a, _dir_b, file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &file);
        drop(browser.handle_entry_hovered(dir_a.clone()));
        assert!(browser.file_drag_spring_hover.is_some());

        // 松手结束拖拽:候选是悬停态的衍生,滞留由 tick 的会话兜底清除。
        browser.cancel_file_drag_interaction();
        drop(browser.handle_file_drag_spring_open_tick());
        assert!(browser.file_drag_spring_hover.is_none());
    }

    /// 分栏浏览器:pane A(PRIMARY,活动,列表视图)浏览 /workspace,
    /// pane B(BrowserPaneId(1),非活动,列表视图)浏览 /second 且含
    /// folder 目录条目。返回 (browser, folder 路径)。
    fn split_browser_with_inactive_folder_pane() -> (FileBrowser, PathBuf) {
        let (mut browser, _dir_a, _dir_b, _file, _workspace) = spring_browser();
        let folder = PathBuf::from("/second/folder");
        let mut second_pane = browser.capture_active_pane_snapshot();
        second_pane.id = BrowserPaneId(1);
        second_pane.current_dir = PathBuf::from("/second");
        second_pane.entries = vec![test_entry(folder.clone(), FileKind::Directory)].into();
        browser.panes.push(second_pane);
        browser.pane_layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 500,
        };
        (browser, folder)
    }

    /// 跨 pane 悬停(iced 路径):B 里的目录条目同样构成 spring 候选并
    /// 画进度环;到点激活 B 并进入目录,A 的状态完整快照回 panes。
    #[test]
    fn cross_pane_hover_springs_into_inactive_pane_directory() {
        let (mut browser, folder) = split_browser_with_inactive_folder_pane();
        let file = PathBuf::from("/workspace/file.txt");
        start_drag_on(&mut browser, &file);

        drop(browser.handle_file_drag_entry_hovered_in_pane(BrowserPaneId(1), folder.clone()));
        let candidate = browser.file_drag_spring_hover.as_ref().unwrap();
        assert_eq!(candidate.directory, folder);
        assert_eq!(candidate.pane_id, BrowserPaneId(1));
        assert!(browser.file_drag_spring_open_progress(&folder).is_some());

        // 未到阈值:不动,B 也不被激活。
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.active_pane_id(), BrowserPaneId::PRIMARY);
        assert_eq!(browser.current_dir, PathBuf::from("/workspace"));

        // 到点:激活 B 并进入目录;拖拽存活,A 快照完整。
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.active_pane_id(), BrowserPaneId(1));
        assert_eq!(browser.current_dir, folder);
        assert!(browser
            .file_drag
            .as_ref()
            .is_some_and(|drag| drag.is_dragging()));
        assert_eq!(
            browser
                .pane_by_id(BrowserPaneId::PRIMARY)
                .unwrap()
                .current_dir,
            PathBuf::from("/workspace")
        );
    }

    /// 跨 pane 悬停(原生 dnd 路径):布局快照命中 pane B 的目录条目,
    /// 候选成立;到点同样激活 B 并进入。
    #[test]
    fn native_layout_cross_pane_hover_opens_directory_in_target_pane() {
        use iced::{Rectangle, Size};

        use crate::model::{ColumnEntryBounds, FileDragHitTestBounds, FileDropLayoutState};

        let (mut browser, folder) = split_browser_with_inactive_folder_pane();
        let file = PathBuf::from("/workspace/file.txt");
        start_drag_on(&mut browser, &file);

        // 激活后落点会话建立(测试无 Wayland 运行时,走应用内回退)。
        let identity = browser.file_drop_session.as_ref().unwrap().identity.clone();
        let request = match &browser.file_drop_session.as_ref().unwrap().layout {
            FileDropLayoutState::Pending(request) => request.clone(),
            other => panic!("expected pending drop layout, got {other:?}"),
        };

        // 喂布局:pane B 的 folder 行命中光标,按原生 Moved 事件收敛悬停。
        let mut measured = FileDragHitTestBounds::default();
        measured.entries = vec![ColumnEntryBounds {
            pane_id: BrowserPaneId(1),
            path: folder.clone(),
            bounds: Rectangle::new(Point::new(0.0, 0.0), Size::new(200.0, 200.0)),
        }];
        drop(browser.accept_drop_layout(request, measured));
        drop(browser.move_native_file_drop_session(identity, Point::new(50.0, 50.0)));
        assert_eq!(
            browser
                .file_drag_spring_hover
                .as_ref()
                .map(|candidate| (candidate.directory.clone(), candidate.pane_id)),
            Some((folder.clone(), BrowserPaneId(1)))
        );

        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.active_pane_id(), BrowserPaneId(1));
        assert_eq!(browser.current_dir, folder);
    }

    /// 候选 pane 在拖拽中途消失(窗格合并等):候选随 pane 失效,tick
    /// 清除且不导航。
    #[test]
    fn cross_pane_candidate_dies_when_candidate_pane_disappears() {
        let (mut browser, folder) = split_browser_with_inactive_folder_pane();
        let file = PathBuf::from("/workspace/file.txt");
        start_drag_on(&mut browser, &file);

        drop(browser.handle_file_drag_entry_hovered_in_pane(BrowserPaneId(1), folder.clone()));
        assert!(browser.file_drag_spring_hover.is_some());
        browser.panes.retain(|pane| pane.id != BrowserPaneId(1));

        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert!(browser.file_drag_spring_hover.is_none());
        assert_eq!(browser.active_pane_id(), BrowserPaneId::PRIMARY);
        assert_ne!(browser.current_dir, folder);
    }

    /// 回收站视图的 pane 不接受拖放,其目录条目悬停不构成候选。
    #[test]
    fn trash_pane_hover_never_springs() {
        let (mut browser, _folder) = split_browser_with_inactive_folder_pane();
        let trashed_folder = PathBuf::from("/trash-root/folder");
        let mut trash_pane = browser.capture_active_pane_snapshot();
        trash_pane.id = BrowserPaneId(1);
        trash_pane.current_dir = PathBuf::from("/trash-root");
        trash_pane.is_trash_view = true;
        trash_pane.entries = vec![test_entry(trashed_folder.clone(), FileKind::Directory)].into();
        browser.panes.pop();
        browser.panes.push(trash_pane);

        let file = PathBuf::from("/workspace/file.txt");
        start_drag_on(&mut browser, &file);
        drop(
            browser
                .handle_file_drag_entry_hovered_in_pane(BrowserPaneId(1), trashed_folder.clone()),
        );
        assert!(browser.file_drag_spring_hover.is_none());
    }

    #[test]
    fn native_layout_hover_opens_column_and_extends_frozen_chain_snapshot() {
        use iced::{Rectangle, Size};

        use crate::model::{ColumnEntryBounds, FileDragHitTestBounds, FileDropLayoutState};

        let (mut browser, dir_a, _dir_b, file, workspace) = spring_browser();
        browser.view_mode = BrowserViewMode::Columns;
        // 多栏真实拖拽:单击栏条目起拖,栏链快照被捕获(拖拽期间渲染冻结链)。
        browser.selected = Some(file.clone());
        browser.cursor_position = Point::new(100.0, 100.0);
        drop(browser.handle_column_entry_clicked(file.clone()));
        drop(browser.update_file_drag(Point::new(130.0, 100.0)));
        assert_eq!(
            browser
                .file_drag
                .as_ref()
                .unwrap()
                .column_directories_snapshot,
            vec![workspace.clone()]
        );

        // 激活后落点会话建立(测试无 Wayland 运行时,走应用内回退)。
        let identity = browser.file_drop_session.as_ref().unwrap().identity.clone();
        let request = match &browser.file_drop_session.as_ref().unwrap().layout {
            FileDropLayoutState::Pending(request) => request.clone(),
            other => panic!("expected pending drop layout, got {other:?}"),
        };

        // 喂布局:dir_a 行命中光标;按原生 Moved 事件同款入口收敛悬停。
        let mut measured = FileDragHitTestBounds::default();
        measured.entries = vec![ColumnEntryBounds {
            pane_id: browser.active_pane_id(),
            path: dir_a.clone(),
            bounds: Rectangle::new(Point::new(0.0, 0.0), Size::new(200.0, 200.0)),
        }];
        drop(browser.accept_drop_layout(request, measured));
        drop(browser.move_native_file_drop_session(identity, Point::new(50.0, 50.0)));
        assert_eq!(
            browser
                .file_drag_spring_hover
                .as_ref()
                .map(|candidate| candidate.directory.clone()),
            Some(dir_a.clone())
        );

        // 到点:开栏,并把新链写回拖拽快照——新栏在拖拽中的视图可见。
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert!(browser.expanded_directories.contains_key(&dir_a));
        assert_eq!(
            browser
                .file_drag
                .as_ref()
                .unwrap()
                .column_directories_snapshot,
            vec![workspace.clone(), dir_a.clone()]
        );
        assert_eq!(browser.current_dir, workspace);
        // 同一次悬停不重复触发。
        assert!(browser.file_drag_spring_hover.as_ref().unwrap().fired);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, workspace);
    }

    #[test]
    fn columns_view_opens_next_column_instead_of_navigating() {
        let (mut browser, dir_a, _dir_b, file, workspace) = spring_browser();
        browser.view_mode = BrowserViewMode::Columns;
        start_drag_on(&mut browser, &file);

        drop(browser.handle_entry_hovered(dir_a.clone()));
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());

        // 多栏:当前目录不变,目标目录在下一栏打开;同一次悬停不重复触发。
        assert_eq!(browser.current_dir, workspace);
        assert!(browser.expanded_directories.contains_key(&dir_a));
        assert!(browser.file_drag_spring_hover.as_ref().unwrap().fired);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, workspace);
    }

    #[test]
    fn navigation_marks_drop_layout_stale_and_tick_remeasures() {
        use crate::model::FileDropLayoutState;

        let (mut browser, dir_a, _dir_b, file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &file);
        assert!(!browser.file_drop_layout_stale);

        // spring 导航:条目集被整体替换(导航先清空)→ 布局内容级过期。
        drop(browser.handle_entry_hovered(dir_a.clone()));
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert!(browser.file_drop_layout_stale);

        // tick 消化:布局进入 Pending 重测,标记复位。
        drop(browser.handle_file_drag_spring_open_tick());
        assert!(!browser.file_drop_layout_stale);
        assert!(matches!(
            browser.file_drop_session.as_ref().unwrap().layout,
            FileDropLayoutState::Pending(_)
        ));

        // 无拖拽会话时条目变化不置位(避免空转重测)。
        browser.cancel_file_drag_interaction();
        drop(browser.handle_file_drag_spring_open_tick());
        browser.set_entries(browser.entries.clone());
        assert!(!browser.file_drop_layout_stale);
    }

    #[test]
    fn chain_drilling_works_after_navigation_refreshes_layout() {
        use iced::{Rectangle, Size};

        use crate::model::{ColumnEntryBounds, FileDragHitTestBounds, FileDropLayoutState};

        let (mut browser, dir_a, _dir_b, file, _workspace) = spring_browser();
        start_drag_on(&mut browser, &file);

        // 第一跳(iced hover 路径):进入 dir_a。
        drop(browser.handle_entry_hovered(dir_a.clone()));
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, dir_a);

        // 导航加载完成:新条目落定,过期布局经 tick 重测进入 Pending。
        let sub = dir_a.join("sub");
        browser.entries = vec![test_entry(sub.clone(), FileKind::Directory)].into();
        assert!(browser.file_drop_layout_stale);
        drop(browser.handle_file_drag_spring_open_tick());
        let identity = browser.file_drop_session.as_ref().unwrap().identity.clone();
        let request = match &browser.file_drop_session.as_ref().unwrap().layout {
            FileDropLayoutState::Pending(request) => request.clone(),
            other => panic!("expected pending drop layout, got {other:?}"),
        };

        // 喂新布局(sub 行命中光标),按原生 Moved 入口收敛悬停:链式候选恢复。
        let mut measured = FileDragHitTestBounds::default();
        measured.entries = vec![ColumnEntryBounds {
            pane_id: browser.active_pane_id(),
            path: sub.clone(),
            bounds: Rectangle::new(Point::new(0.0, 0.0), Size::new(200.0, 600.0)),
        }];
        drop(browser.accept_drop_layout(request, measured));
        drop(browser.move_native_file_drop_session(identity, Point::new(50.0, 50.0)));
        assert_eq!(
            browser
                .file_drag_spring_hover
                .as_ref()
                .map(|candidate| candidate.directory.clone()),
            Some(sub.clone())
        );

        // 第二跳:到点进入子目录(历史逐跳入栈)。
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, sub);
        assert_eq!(
            browser.back_stack,
            vec![dir_a.parent().unwrap().clone(), &dir_a]
        );
    }

    #[test]
    fn breadcrumb_hover_navigates_back_after_delay() {
        let (mut browser, dir_a, _dir_b, file, workspace) = spring_browser();
        // 模拟 spring 已钻入 dir_a:当前目录与其内容,拖着其中的文件。
        browser.current_dir = dir_a.clone();
        browser.entries = vec![test_entry(file.clone(), FileKind::File)].into();
        start_drag_on(&mut browser, &file);

        // 悬浮面包屑的 workspace 段落:候选来源为面包屑,无进度环。
        browser.handle_breadcrumb_drop_target_hovered(workspace.clone());
        let candidate = browser.file_drag_spring_hover.as_ref().unwrap();
        assert_eq!(candidate.directory, workspace);
        assert_eq!(candidate.source, FileDragSpringSource::Breadcrumb);
        assert_eq!(browser.file_drag_spring_open_progress(&workspace), None);

        // 悬停离开该段落:候选清除。
        browser.handle_breadcrumb_drop_target_hover_cleared(workspace.clone());
        assert!(browser.file_drag_spring_hover.is_none());

        // 重新悬浮,到点导航回 workspace(历史入栈)。
        browser.handle_breadcrumb_drop_target_hovered(workspace.clone());
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, workspace);
        assert_eq!(browser.back_stack, vec![dir_a]);
    }

    #[test]
    fn breadcrumb_spring_skips_current_dir_segment() {
        let (mut browser, _dir_a, _dir_b, file, workspace) = spring_browser();
        start_drag_on(&mut browser, &file);

        // 悬停 current_dir 段落:导航是空操作,不触发。
        browser.handle_breadcrumb_drop_target_hovered(workspace.clone());
        assert!(browser.file_drag_spring_hover.is_none());
    }

    #[test]
    fn breadcrumb_hover_in_columns_view_resets_chain_and_keeps_drag() {
        let (mut browser, dir_a, _dir_b, _file, workspace) = spring_browser();
        browser.view_mode = BrowserViewMode::Columns;
        // 模拟已在 dir_a 内:当前目录、内容与其展开子栏。
        let file_in_a = dir_a.join("file.txt");
        let sub = dir_a.join("sub");
        browser.current_dir = dir_a.clone();
        browser.entries = vec![
            test_entry(sub.clone(), FileKind::Directory),
            test_entry(file_in_a.clone(), FileKind::File),
        ]
        .into();
        drop(browser.open_column_for_directory(sub.clone()));

        // 按多栏方式起拖(捕获真实栏链快照 [dir_a, dir_a/sub])。
        browser.selected = Some(file_in_a.clone());
        browser.cursor_position = Point::new(100.0, 100.0);
        let snapshot = crate::three_column_view::column_directories(&browser);
        drop(browser.start_file_drag(
            file_in_a.clone(),
            FileDragStationaryAction::ActivateColumnEntry,
            snapshot,
        ));
        drop(browser.update_file_drag(Point::new(130.0, 100.0)));
        assert_eq!(
            browser
                .file_drag
                .as_ref()
                .unwrap()
                .column_directories_snapshot,
            vec![dir_a.clone(), sub.clone()]
        );

        // 悬停面包屑 workspace 段落(dir_a 的父级),到点导航:栏链重置
        // 回活链,拖拽存活。
        browser.handle_breadcrumb_drop_target_hovered(workspace.clone());
        assert_eq!(
            browser
                .file_drag_spring_hover
                .as_ref()
                .map(|candidate| (candidate.directory.clone(), candidate.source)),
            Some((workspace.clone(), FileDragSpringSource::Breadcrumb))
        );
        browser.file_drag_spring_hover.as_mut().unwrap().since =
            Instant::now() - FILE_DRAG_SPRING_OPEN_DELAY - Duration::from_millis(1);
        drop(browser.handle_file_drag_spring_open_tick());
        assert_eq!(browser.current_dir, workspace);
        assert!(browser.expanded_directories.is_empty());
        let drag = browser.file_drag.as_ref().unwrap();
        assert!(drag.column_directories_snapshot.is_empty());
        assert!(drag.is_dragging());
    }
}
