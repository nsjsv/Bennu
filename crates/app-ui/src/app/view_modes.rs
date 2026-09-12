use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use file_core::FileKind;
use iced::Task;

use super::FileBrowser;
use crate::app::smooth_scroll::smooth_scroll_id;
use crate::commands::load_expanded_directory_command;
use crate::config::normalize_visible_column_count;
use crate::list_view::LIST_HEADER_HEIGHT;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, ListExpansionFollowSessionId, Message,
    ScrollbarRegion,
};
use crate::thumbnail_cache::ColumnViewport;
use crate::virtual_range::vertical_scroll_delta_to_reveal;

const LIST_DIRECTORY_ANIMATION_STEP: f32 = 0.18;

#[derive(Debug, Clone)]
pub(super) struct ListExpansionFollowPlan {
    pane_id: BrowserPaneId,
    tab_id: usize,
    current_dir: PathBuf,
    session_id: ListExpansionFollowSessionId,
    remaining_directories: VecDeque<PathBuf>,
    waiting_for: PathBuf,
    target_selection: PathBuf,
}

struct DirectoryExpansionTransfer {
    chain: Vec<PathBuf>,
    target_selection: PathBuf,
}

impl FileBrowser {
    pub(crate) fn select_visible_column_count(&mut self, count: usize) -> Task<Message> {
        let count = normalize_visible_column_count(count);
        if self.user_config.visible_column_count == count {
            return Task::none();
        }
        self.user_config.visible_column_count = count;
        self.persist_user_preferences_command()
    }

    pub(super) fn select_browser_view_mode(
        &mut self,
        pane_id: BrowserPaneId,
        view_mode: BrowserViewMode,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        if self.view_mode == view_mode {
            return Task::none();
        }

        let previous_mode = self.view_mode;
        let list_to_icons = (previous_mode == BrowserViewMode::List
            && view_mode == BrowserViewMode::Icons)
            .then(|| self.list_expansion_transfer())
            .flatten();
        let icons_to_list = (previous_mode == BrowserViewMode::Icons
            && view_mode == BrowserViewMode::List)
            .then(|| self.icon_grid_expansion_transfer())
            .flatten();

        self.clear_icon_grid_expansion_for_context_change();
        if previous_mode == BrowserViewMode::Icons || view_mode == BrowserViewMode::Icons {
            self.retain_direct_entry_selection();
        }
        let mut transition_command = match (previous_mode, view_mode) {
            (BrowserViewMode::Columns, BrowserViewMode::List) => {
                self.sync_expanded_directories_to_open_columns();
                Task::none()
            }
            (BrowserViewMode::List, BrowserViewMode::Columns) => {
                self.sync_open_column_directory_to_list_selection()
            }
            (BrowserViewMode::Icons, BrowserViewMode::Columns)
                if self.selected_is_direct_entry() =>
            {
                self.sync_open_column_directory_to_list_selection()
            }
            _ => Task::none(),
        };
        self.view_mode = view_mode;
        if let Some(transfer) = list_to_icons {
            transition_command =
                self.start_icon_grid_expansion_follow(transfer.chain, transfer.target_selection);
        } else if let Some(transfer) = icons_to_list {
            transition_command =
                self.start_list_expansion_follow(transfer.chain, transfer.target_selection);
        } else if previous_mode == BrowserViewMode::Icons && view_mode == BrowserViewMode::List {
            self.clear_list_expansion_for_follow();
        }
        self.hovered_entry = None;
        self.clear_list_header_hover_in_pane(pane_id);
        self.cursor_paste_directory = None;
        self.clear_column_interaction_context();
        self.selection_marquee = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        self.pending_keyboard_column_focus = None;
        self.column_resize_drag = None;
        self.file_entry_bounds.clear();
        self.user_config.browser_view_mode = view_mode;
        self.sync_active_tab_state();
        let list_directory_summary_command = if view_mode == BrowserViewMode::List {
            self.schedule_visible_list_directory_summaries_for_pane(pane_id)
        } else {
            Task::none()
        };
        let view_switch_reveal = self.reveal_selected_after_view_switch();
        Task::batch([
            transition_command,
            list_directory_summary_command,
            view_switch_reveal,
            self.persist_user_preferences_command(),
            self.schedule_thumbnail_refresh(),
            self.request_browser_session_save(),
        ])
    }

    /// 视图切换后让主选中项在新视图中可见；切到多栏时同时横向滚到选中列。
    fn reveal_selected_after_view_switch(&mut self) -> Task<Message> {
        self.pending_view_switch_reveal = None;
        let Some(path) = self.selected.clone() else {
            return Task::none();
        };
        let pane_id = self.active_pane_id();
        let vertical = match self.view_switch_reveal_scroll(&path) {
            Some(task) => task,
            // 选中项所在目录仍在加载：挂起等待加载完成后补聚焦
            None => {
                self.pending_view_switch_reveal = Some((pane_id, path.clone()));
                Task::none()
            }
        };
        let horizontal = if self.view_mode == BrowserViewMode::Columns {
            self.focus_column_containing_path(&path)
        } else {
            Task::none()
        };
        Task::batch([vertical, horizontal])
    }

    /// 视口越界自愈:条目集骤减(切换"显示隐藏文件"、文件操作、增量更新
    /// 等)后,记录的滚动偏移可能超过新内容可支撑的最大偏移,虚拟列表按
    /// 旧偏移算出的可见区间落在内容之外,渲染出整屏空白("列表内容消失")。
    /// 出口统一把三种视图的越界视口重钉到内容尾部,并同步 iced 的真实
    /// 滚动位置。
    pub(super) fn clamp_viewports_to_content(&mut self) -> Task<Message> {
        #[derive(Debug)]
        enum Reclamp {
            Shared {
                pane_id: BrowserPaneId,
                region: ScrollbarRegion,
                directory: PathBuf,
                viewport: ColumnViewport,
            },
            Icons {
                pane_id: BrowserPaneId,
                directory: PathBuf,
                viewport: crate::model::IconGridViewport,
            },
        }

        let pane_ids: Vec<_> = self.panes.iter().map(|pane| pane.id).collect();
        let mut pending = Vec::new();
        for pane_id in pane_ids {
            let Some(pane) = self.pane_view(pane_id) else {
                continue;
            };
            match pane.view_mode {
                BrowserViewMode::List => {
                    let Some(viewport) = pane.column_viewports.get(pane.current_dir).copied()
                    else {
                        continue;
                    };
                    let geometry = crate::list_view::ListGeometry::for_level(
                        self.user_config.list_view_density,
                    );
                    // 粗估守卫:行数乘积是内容高度下界,偏移在此之内必然合法,
                    // 免去常规帧的精确遍历;疑似越界才做含展开状态行的精确计算。
                    let flat_lower_bound =
                        LIST_HEADER_HEIGHT + pane.entries.len() as f32 * geometry.row_height;
                    if viewport.offset_y <= (flat_lower_bound - viewport.height).max(0.0) {
                        continue;
                    }
                    let content_height = LIST_HEADER_HEIGHT
                        + crate::visible_entries::list_rows_content_height(
                            pane.entries,
                            pane.expanded_directories,
                            geometry.row_height,
                        );
                    let max_offset = (content_height - viewport.height).max(0.0);
                    if viewport.offset_y > max_offset {
                        pending.push(Reclamp::Shared {
                            pane_id,
                            region: ScrollbarRegion::PaneList(pane_id),
                            directory: pane.current_dir.clone(),
                            viewport: ColumnViewport {
                                offset_y: max_offset,
                                height: viewport.height,
                            },
                        });
                    }
                }
                BrowserViewMode::Columns => {
                    let geometry = crate::three_column_view::ColumnGeometry::for_level(
                        self.user_config.columns_view_density,
                    );
                    for directory in crate::three_column_view::column_directories_for_pane(pane) {
                        let Some(viewport) = pane.column_viewports.get(&directory).copied() else {
                            continue;
                        };
                        // 栏内容高按行数乘积保守估计(不含面板外留白),
                        // clamp 稍紧只是不在最底部,不会二次越界。
                        let entries: &[file_core::DirectoryEntry] = if directory
                            == *pane.current_dir
                        {
                            pane.entries
                        } else {
                            let Some(expanded) = pane.expanded_directories.get(&directory) else {
                                continue;
                            };
                            &expanded.entries
                        };
                        let content_height = geometry.entries_top_padding
                            + entries.len() as f32 * geometry.entry_scroll_height;
                        let max_offset = (content_height - viewport.height).max(0.0);
                        if viewport.offset_y > max_offset {
                            pending.push(Reclamp::Shared {
                                pane_id,
                                region: ScrollbarRegion::Column {
                                    pane_id,
                                    directory: directory.clone(),
                                },
                                directory,
                                viewport: ColumnViewport {
                                    offset_y: max_offset,
                                    height: viewport.height,
                                },
                            });
                        }
                    }
                }
                BrowserViewMode::Icons => {
                    let layout = self.icon_grid_layout_for_pane(pane);
                    let viewport = pane.icon_grid_viewport;
                    let max_offset = (layout.total_height() - viewport.height).max(0.0);
                    if viewport.offset_y > max_offset {
                        pending.push(Reclamp::Icons {
                            pane_id,
                            directory: pane.current_dir.clone(),
                            viewport: crate::model::IconGridViewport {
                                offset_y: max_offset,
                                ..viewport
                            },
                        });
                    }
                }
            }
        }

        let mut commands = Vec::new();
        for reclamp in pending {
            match reclamp {
                Reclamp::Shared {
                    pane_id,
                    region,
                    directory,
                    viewport,
                } => {
                    if pane_id == self.active_pane_id() {
                        self.column_viewports.insert(directory, viewport);
                    } else if let Some(pane_snapshot) = self.pane_by_id_mut(pane_id) {
                        pane_snapshot.column_viewports.insert(directory, viewport);
                    }
                    commands.push(iced::widget::operation::scroll_to(
                        smooth_scroll_id(&region),
                        iced::widget::scrollable::AbsoluteOffset {
                            x: None,
                            y: Some(viewport.offset_y),
                        },
                    ));
                    // 重钉让内容高度骤变,滚动条缓存的溢出数据随之过期;
                    // 借布局探针把当帧布局写回,thumb 不再照旧数据显示。
                    commands.push(self.verify_scrollbar_layout(region));
                }
                Reclamp::Icons {
                    pane_id,
                    directory,
                    viewport,
                } => {
                    self.icon_grid_viewports.insert(
                        pane_id,
                        super::PaneIconGridViewport {
                            directory,
                            viewport,
                        },
                    );
                    commands.push(iced::widget::operation::scroll_to(
                        smooth_scroll_id(&ScrollbarRegion::PaneIcons(pane_id)),
                        iced::widget::scrollable::AbsoluteOffset {
                            x: None,
                            y: Some(viewport.offset_y),
                        },
                    ));
                    commands
                        .push(self.verify_scrollbar_layout(ScrollbarRegion::PaneIcons(pane_id)));
                }
            }
        }
        if commands.is_empty() {
            Task::none()
        } else {
            Task::batch(commands)
        }
    }

    /// 主选中项在当前视图中的纵向定位任务；目录内容尚未就绪时返回 None。
    pub(super) fn view_switch_reveal_scroll(&self, path: &Path) -> Option<Task<Message>> {
        let pane_id = self.active_pane_id();
        let (region, target_y) = match self.view_mode {
            BrowserViewMode::Icons => {
                let pane = self.pane_view(pane_id)?;
                let viewport = pane.icon_grid_viewport;
                let delta = self
                    .icon_grid_layout_for_pane(pane)
                    .scroll_delta_to_reveal(viewport, path);
                (
                    ScrollbarRegion::PaneIcons(pane_id),
                    viewport.offset_y + delta,
                )
            }
            BrowserViewMode::List => {
                let geometry =
                    crate::list_view::ListGeometry::for_level(self.user_config.list_view_density);
                let (item_offset, item_height) =
                    crate::visible_entries::list_entry_vertical_bounds(
                        &self.entries,
                        &self.expanded_directories,
                        path,
                        geometry.row_height,
                        LIST_HEADER_HEIGHT,
                    )?;
                let viewport = self.column_viewports.get(&self.current_dir);
                (
                    ScrollbarRegion::PaneList(pane_id),
                    reveal_target_y(viewport, item_offset, item_height),
                )
            }
            BrowserViewMode::Columns => {
                let directory = self.entry_parent_directory(path);
                let entries = if directory == self.current_dir {
                    self.entries.as_ref()
                } else {
                    self.expanded_directories
                        .get(&directory)?
                        .entries
                        .as_slice()
                };
                let row_index = entries.iter().position(|entry| entry.path == path)?;
                let geometry = crate::three_column_view::ColumnGeometry::for_level(
                    self.user_config.columns_view_density,
                );
                let item_offset =
                    geometry.entries_top_padding + row_index as f32 * geometry.entry_scroll_height;
                let viewport = self.column_viewports.get(&directory);
                (
                    ScrollbarRegion::Column { pane_id, directory },
                    reveal_target_y(viewport, item_offset, geometry.entry_height),
                )
            }
        };
        Some(iced::widget::operation::scroll_to(
            smooth_scroll_id(&region),
            iced::widget::scrollable::AbsoluteOffset {
                x: None,
                y: Some(target_y.max(0.0)),
            },
        ))
    }

    pub(super) fn retain_direct_entry_selection(&mut self) {
        crate::model::retain_direct_entry_selection(
            &self.entries,
            &mut self.selected,
            &mut self.selected_paths,
            &mut self.selection_anchor,
        );
    }

    pub(super) fn cancel_expansion_follow_plans(&mut self) {
        self.cancel_list_expansion_follow();
        if let Some(state) = self.icon_grid_expansion.as_mut() {
            state.cancel_follow_plan();
        }
    }

    pub(super) fn cancel_list_expansion_follow(&mut self) {
        let Some(plan) = self.list_expansion_follow.take() else {
            return;
        };
        if let Some(mut expanded) = self.expanded_directories.remove(&plan.waiting_for) {
            Self::cancel_expanded_directory_load(&mut expanded);
        }
    }

    fn list_expansion_transfer(&self) -> Option<DirectoryExpansionTransfer> {
        let target_selection = self.selected.clone()?;
        if target_selection == self.current_dir || !target_selection.starts_with(&self.current_dir)
        {
            return None;
        }

        let selected_is_expanded_directory = self
            .expanded_directories
            .get(&target_selection)
            .is_some_and(list_directory_is_followable);
        let mut current = if selected_is_expanded_directory {
            target_selection.clone()
        } else {
            target_selection.parent()?.to_path_buf()
        };
        let mut chain = Vec::new();
        while current != self.current_dir {
            chain.push(current.clone());
            current = current.parent()?.to_path_buf();
        }
        chain.reverse();
        if chain.is_empty() {
            return None;
        }

        let mut parent = self.current_dir.as_path();
        for directory_path in &chain {
            let entries = if parent == self.current_dir {
                self.entries.as_slice()
            } else {
                self.expanded_directories
                    .get(parent)
                    .filter(|expanded| list_directory_is_followable(expanded))?
                    .entries
                    .as_slice()
            };
            if !entries.iter().any(|entry| {
                entry.path == *directory_path
                    && entry.kind == FileKind::Directory
                    && entry.path.parent() == Some(parent)
            }) || !self
                .expanded_directories
                .get(directory_path)
                .is_some_and(list_directory_is_followable)
            {
                return None;
            }
            parent = directory_path;
        }

        let target_parent = target_selection.parent()?;
        let target_entries = if target_parent == self.current_dir {
            self.entries.as_slice()
        } else {
            self.expanded_directories
                .get(target_parent)
                .filter(|expanded| list_directory_is_followable(expanded))?
                .entries
                .as_slice()
        };
        if !target_entries.iter().any(|entry| {
            entry.path == target_selection && entry.path.parent() == Some(target_parent)
        }) {
            return None;
        }

        Some(DirectoryExpansionTransfer {
            chain,
            target_selection,
        })
    }

    fn icon_grid_expansion_transfer(&self) -> Option<DirectoryExpansionTransfer> {
        let target_selection = self.selected.clone()?;
        let chain = self
            .icon_grid_expansion
            .as_ref()?
            .interactive_expansion_chain_for_selection(&target_selection)?;
        (!chain.is_empty()).then_some(DirectoryExpansionTransfer {
            chain,
            target_selection,
        })
    }

    fn start_list_expansion_follow(
        &mut self,
        mut chain: Vec<PathBuf>,
        target_selection: PathBuf,
    ) -> Task<Message> {
        let Some(root_path) = chain.first().cloned() else {
            return Task::none();
        };
        self.clear_list_expansion_for_follow();
        if !self.entries.iter().any(|entry| {
            entry.path == root_path
                && entry.kind == FileKind::Directory
                && entry.path.parent() == Some(self.current_dir.as_path())
        }) {
            return Task::none();
        }

        let mut expanded = loading_list_directory();
        let session_id = self.next_list_expansion_follow_session_id();
        let pane_id = self.active_pane_id();
        let tab_id = self.active_tab_id;
        let current_dir = self.current_dir.clone();
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            DirectoryExpansionLoadContext::ListFollow {
                pane_id,
                tab_id,
                current_dir: current_dir.clone(),
                session_id,
            },
            root_path.clone(),
            &mut expanded,
        );
        self.expanded_directories
            .insert(root_path.clone(), expanded);
        chain.remove(0);
        self.list_expansion_follow = Some(ListExpansionFollowPlan {
            pane_id,
            tab_id,
            current_dir,
            session_id,
            remaining_directories: chain.into(),
            waiting_for: root_path,
            target_selection,
        });
        load_expanded_directory_command(request, self.options.clone(), cancellation)
    }

    fn clear_list_expansion_for_follow(&mut self) {
        self.list_expansion_follow = None;
        for expanded in self.expanded_directories.values_mut() {
            Self::cancel_expanded_directory_load(expanded);
        }
        self.expanded_directories.clear();
    }

    pub(super) fn expanded_directory_load_context(
        &self,
        pane_id: BrowserPaneId,
        path: &Path,
    ) -> DirectoryExpansionLoadContext {
        let Some(plan) = self.list_expansion_follow.as_ref() else {
            return DirectoryExpansionLoadContext::BrowserTree { pane_id };
        };
        if self.view_mode == BrowserViewMode::List
            && pane_id == plan.pane_id
            && self.active_pane_id() == plan.pane_id
            && self.active_tab_id == plan.tab_id
            && self.current_dir == plan.current_dir
            && path == plan.waiting_for
        {
            DirectoryExpansionLoadContext::ListFollow {
                pane_id: plan.pane_id,
                tab_id: plan.tab_id,
                current_dir: plan.current_dir.clone(),
                session_id: plan.session_id,
            }
        } else {
            DirectoryExpansionLoadContext::BrowserTree { pane_id }
        }
    }

    pub(super) fn list_expansion_follow_request_is_current(
        &self,
        request: &ExpandedDirectoryLoadRequest,
    ) -> bool {
        let Some(plan) = self.list_expansion_follow.as_ref() else {
            return false;
        };
        matches!(
            &request.context,
            DirectoryExpansionLoadContext::ListFollow {
                pane_id,
                tab_id,
                current_dir,
                session_id,
            } if *pane_id == plan.pane_id
                && *tab_id == plan.tab_id
                && current_dir == &plan.current_dir
                && *session_id == plan.session_id
        ) && self.view_mode == BrowserViewMode::List
            && self.active_pane_id() == plan.pane_id
            && self.active_tab_id == plan.tab_id
            && self.current_dir == plan.current_dir
            && request.path == plan.waiting_for
    }

    #[cfg(test)]
    pub(in crate::app) fn pending_list_expansion_follow_request(
        &self,
    ) -> Option<ExpandedDirectoryLoadRequest> {
        let plan = self.list_expansion_follow.as_ref()?;
        let expanded = self.expanded_directories.get(&plan.waiting_for)?;
        Some(ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::ListFollow {
                pane_id: plan.pane_id,
                tab_id: plan.tab_id,
                current_dir: plan.current_dir.clone(),
                session_id: plan.session_id,
            },
            path: plan.waiting_for.clone(),
            generation: expanded.load_generation,
        })
    }

    pub(super) fn advance_list_expansion_follow(
        &mut self,
        loaded_successfully: bool,
    ) -> Task<Message> {
        let Some(plan) = self.list_expansion_follow.as_ref() else {
            return Task::none();
        };
        if !loaded_successfully {
            self.list_expansion_follow = None;
            return Task::none();
        }

        if let Some(next_path) = plan.remaining_directories.front().cloned() {
            let waiting_for = plan.waiting_for.clone();
            let next_is_direct_directory = self
                .expanded_directories
                .get(&waiting_for)
                .filter(|expanded| list_directory_is_followable(expanded))
                .is_some_and(|expanded| {
                    expanded.entries.iter().any(|entry| {
                        entry.path == next_path
                            && entry.kind == FileKind::Directory
                            && entry.path.parent() == Some(waiting_for.as_path())
                    })
                });
            if !next_is_direct_directory {
                self.list_expansion_follow = None;
                return Task::none();
            }

            let context = DirectoryExpansionLoadContext::ListFollow {
                pane_id: plan.pane_id,
                tab_id: plan.tab_id,
                current_dir: plan.current_dir.clone(),
                session_id: plan.session_id,
            };
            let mut expanded = loading_list_directory();
            let (next_request, cancellation) = Self::next_expanded_directory_load_request(
                context,
                next_path.clone(),
                &mut expanded,
            );
            self.expanded_directories
                .insert(next_path.clone(), expanded);
            let plan = self
                .list_expansion_follow
                .as_mut()
                .expect("list follow plan remains active");
            plan.remaining_directories.pop_front();
            plan.waiting_for = next_path;
            return load_expanded_directory_command(
                next_request,
                self.options.clone(),
                cancellation,
            );
        }

        let target_selection = plan.target_selection.clone();
        self.list_expansion_follow = None;
        if crate::visible_entries::entry_is_visible(
            &target_selection,
            &self.entries,
            &self.expanded_directories,
        ) {
            self.select_path(target_selection.clone());
            match self.view_switch_reveal_scroll(&target_selection) {
                Some(task) => return task,
                None => {
                    self.pending_view_switch_reveal =
                        Some((self.active_pane_id(), target_selection));
                }
            }
        }
        Task::none()
    }

    fn next_list_expansion_follow_session_id(&mut self) -> ListExpansionFollowSessionId {
        let session_id =
            ListExpansionFollowSessionId::new(self.next_list_expansion_follow_session_id);
        self.next_list_expansion_follow_session_id =
            self.next_list_expansion_follow_session_id.wrapping_add(1);
        session_id
    }

    fn selected_is_direct_entry(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|selected| self.entries.iter().any(|entry| entry.path == *selected))
    }

    pub(super) fn list_directory_animation_is_active(&self) -> bool {
        self.expanded_directories
            .values()
            .any(expanded_directory_is_animating)
            || self.panes.iter().any(|pane| {
                pane.expanded_directories
                    .values()
                    .any(expanded_directory_is_animating)
            })
    }

    pub(super) fn advance_list_directory_animations(&mut self) -> Task<Message> {
        let active_changed = advance_expanded_directories(&mut self.expanded_directories);
        for pane in &mut self.panes {
            let pane_changed = advance_expanded_directories(&mut pane.expanded_directories);
            if pane_changed {
                pane.sync_active_tab_state();
            }
        }
        if active_changed {
            self.sync_active_tab_state();
        }
        Task::none()
    }

    pub(super) fn toggle_list_directory(
        &mut self,
        pane_id: BrowserPaneId,
        path: PathBuf,
    ) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        self.activate_pane(pane_id);
        self.toggle_list_directory_for_path(path)
    }

    pub(super) fn expand_selected_list_directory(&mut self) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let Some(selected) = self.selected.clone() else {
            return Task::none();
        };
        if self.entry_kind(&selected) != Some(FileKind::Directory) {
            return Task::none();
        }

        if self
            .expanded_directories
            .get(&selected)
            .is_some_and(|expanded| expanded.is_expanded && !expanded.is_collapsing)
        {
            if let Some(child) = self.first_visible_child_path(&selected) {
                return self.select_path_from_keyboard(child);
            }
            return Task::none();
        }

        self.open_list_directory(selected)
    }

    pub(super) fn collapse_selected_list_directory_or_select_parent(&mut self) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let Some(selected) = self.selected.clone() else {
            return Task::none();
        };

        if self
            .expanded_directories
            .get(&selected)
            .is_some_and(|expanded| expanded.is_expanded && !expanded.is_collapsing)
        {
            return self.collapse_list_directory(selected);
        }

        let Some(parent) = selected.parent().map(Path::to_path_buf) else {
            return Task::none();
        };
        if parent == self.current_dir {
            return Task::none();
        }
        if !crate::visible_entries::entry_is_visible(
            &parent,
            &self.entries,
            &self.expanded_directories,
        ) {
            return Task::none();
        }

        let scroll_task = self.select_path_from_keyboard(parent);
        self.sync_active_tab_state();
        Task::batch([scroll_task, self.schedule_thumbnail_refresh()])
    }

    fn toggle_list_directory_for_path(&mut self, path: PathBuf) -> Task<Message> {
        if self.is_trash_view || self.entry_kind(&path) != Some(FileKind::Directory) {
            return Task::none();
        }

        if self
            .expanded_directories
            .get(&path)
            .is_some_and(|expanded| expanded.is_expanded && !expanded.is_collapsing)
        {
            return self.collapse_list_directory(path);
        }

        self.open_list_directory(path)
    }

    fn collapse_list_directory(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(expanded) = self.expanded_directories.get_mut(&path) {
            Self::cancel_expanded_directory_load(expanded);
            expanded.is_collapsing = true;
            expanded.animation_progress = expanded.animation_progress.clamp(0.0, 1.0);
        }
        self.sync_active_tab_state();
        Task::batch([
            self.schedule_thumbnail_refresh(),
            self.request_browser_session_save(),
        ])
    }

    fn open_list_directory(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(expanded) = self.expanded_directories.get_mut(&path) {
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = expanded.animation_progress.clamp(0.0, 1.0);
            self.sync_active_tab_state();
            return Task::batch([
                self.schedule_thumbnail_refresh(),
                self.request_browser_session_save(),
            ]);
        }

        let mut expanded = ExpandedDirectory {
            entries: Vec::new(),
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loading,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 0.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        };
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            DirectoryExpansionLoadContext::BrowserTree {
                pane_id: self.active_pane_id(),
            },
            path.clone(),
            &mut expanded,
        );
        self.expanded_directories.insert(path, expanded);
        self.sync_active_tab_state();
        Task::batch([
            load_expanded_directory_command(request, self.options.clone(), cancellation),
            self.schedule_thumbnail_refresh(),
            self.request_browser_session_save(),
        ])
    }

    fn first_visible_child_path(&self, directory: &Path) -> Option<PathBuf> {
        crate::visible_entries::visible_child_paths(
            directory,
            &self.current_dir,
            &self.entries,
            &self.expanded_directories,
        )
        .into_iter()
        .next()
    }

    fn sync_expanded_directories_to_open_columns(&mut self) {
        let retained_paths = crate::three_column_view::column_directories(self)
            .into_iter()
            .filter(|path| path != &self.current_dir)
            .collect::<HashSet<_>>();
        self.expanded_directories.retain(|path, expanded| {
            if retained_paths.contains(path) {
                expanded.is_expanded = true;
                expanded.is_collapsing = false;
                expanded.animation_progress = 1.0;
                return true;
            }
            Self::cancel_expanded_directory_load(expanded);
            false
        });
    }

    fn sync_open_column_directory_to_list_selection(&mut self) -> Task<Message> {
        let selected = self.selected.clone();
        let selected_kind = selected.as_deref().and_then(|path| self.entry_kind(path));
        match (selected, selected_kind) {
            (Some(path), Some(FileKind::Directory)) => {
                let command = self.open_column_for_directory(path);
                self.sync_expanded_directories_to_open_columns();
                command
            }
            (Some(path), Some(_)) => {
                self.set_deepest_open_column_directory(path.parent().map(Path::to_path_buf));
                self.sync_expanded_directories_to_open_columns();
                Task::none()
            }
            _ => {
                self.set_deepest_open_column_directory(None);
                self.sync_expanded_directories_to_open_columns();
                Task::none()
            }
        }
    }

    /// 目录加载完成后补做视图切换聚焦：匹配挂起的选中项时纵向定位一次。
    pub(super) fn complete_pending_view_switch_reveal(
        &mut self,
        loaded_path: &Path,
    ) -> Task<Message> {
        let Some((pane_id, path)) = self.pending_view_switch_reveal.clone() else {
            return Task::none();
        };
        if pane_id != self.active_pane_id() || path.parent() != Some(loaded_path) {
            return Task::none();
        }
        self.pending_view_switch_reveal = None;
        if self.selected.as_deref() != Some(path.as_path()) {
            return Task::none();
        }
        match self.view_switch_reveal_scroll(&path) {
            Some(task) => task,
            // 链式展开仍在继续：保留等待最后一环加载
            None => {
                self.pending_view_switch_reveal = Some((pane_id, path));
                Task::none()
            }
        }
    }
}

/// 视口已有记录时做最小滚动揭示；从未记录过视口时把选中项滚到顶部可见。
fn reveal_target_y(viewport: Option<&ColumnViewport>, item_offset: f32, item_height: f32) -> f32 {
    match viewport {
        Some(viewport) => {
            viewport.offset_y
                + vertical_scroll_delta_to_reveal(
                    viewport.offset_y,
                    viewport.height,
                    item_offset,
                    item_height,
                )
        }
        None => item_offset,
    }
}

fn list_directory_is_followable(expanded: &ExpandedDirectory) -> bool {
    expanded.is_expanded
        && !expanded.is_collapsing
        && matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
}

fn loading_list_directory() -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loading,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 0.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn expanded_directory_is_animating(expanded: &ExpandedDirectory) -> bool {
    (expanded.is_expanded && !expanded.is_collapsing && expanded.animation_progress < 1.0)
        || expanded.is_collapsing
}

fn advance_expanded_directories(
    expanded_directories: &mut std::collections::HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    let mut changed = false;
    for expanded in expanded_directories.values_mut() {
        if expanded.is_collapsing {
            let next_progress =
                (expanded.animation_progress - LIST_DIRECTORY_ANIMATION_STEP).max(0.0);
            if (next_progress - expanded.animation_progress).abs() > f32::EPSILON {
                expanded.animation_progress = next_progress;
                changed = true;
            }
            if expanded.animation_progress <= f32::EPSILON {
                expanded.is_expanded = false;
                expanded.is_collapsing = false;
                changed = true;
            }
        } else if expanded.is_expanded && expanded.animation_progress < 1.0 {
            let next_progress =
                (expanded.animation_progress + LIST_DIRECTORY_ANIMATION_STEP).min(1.0);
            if (next_progress - expanded.animation_progress).abs() > f32::EPSILON {
                expanded.animation_progress = next_progress;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    use file_core::{DirectoryEntry, EntryMetadata};

    fn test_entry(path: PathBuf) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )
    }

    #[test]
    fn visible_column_count_selection_normalizes_to_configured_range() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

        drop(browser.select_visible_column_count(2));
        assert_eq!(browser.user_config.visible_column_count, 3);

        drop(browser.select_visible_column_count(99));
        assert_eq!(browser.user_config.visible_column_count, 5);

        drop(browser.select_visible_column_count(4));
        assert_eq!(browser.user_config.visible_column_count, 4);
    }

    #[test]
    fn list_viewport_offset_is_reclamped_when_entries_shrink_below_it() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let root = PathBuf::from("/workspace");
        browser.current_dir = root.clone();
        browser.view_mode = BrowserViewMode::List;
        browser.entries = (0..500)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();
        browser.column_viewports.insert(
            root.clone(),
            ColumnViewport {
                offset_y: 22000.0,
                height: 800.0,
            },
        );

        // 模拟关闭"显示隐藏文件"后的条目骤减:滚动偏移仍停在旧内容的底部。
        browser.entries = (0..5)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();

        drop(browser.update(crate::model::Message::ThemeModeSelected(
            crate::matugen_theme::ThemeMode::Dark,
        )));

        let clamped = browser
            .column_viewports
            .get(&root)
            .expect("viewport stays recorded");
        // 新内容(5 行 + 表头)不足一屏,合法偏移只有 0;保持 22000 会让
        // 虚拟列表渲染出整屏空白。
        assert_eq!(clamped.offset_y, 0.0);
    }

    #[test]
    fn columns_viewport_offset_is_reclamped_when_entries_shrink_below_it() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let root = PathBuf::from("/workspace");
        browser.current_dir = root.clone();
        browser.view_mode = BrowserViewMode::Columns;
        browser.entries = (0..200)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();
        browser.column_viewports.insert(
            root.clone(),
            ColumnViewport {
                offset_y: 8000.0,
                height: 600.0,
            },
        );

        // 关闭"显示隐藏文件"后栏条目骤减,旧偏移落在内容之外,
        // 对应首栏显示尾部条目、其余栏空白的截图症状。
        browser.entries = (0..5)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();

        drop(browser.update(crate::model::Message::ThemeModeSelected(
            crate::matugen_theme::ThemeMode::Dark,
        )));

        let clamped = browser
            .column_viewports
            .get(&root)
            .expect("viewport stays recorded");
        assert_eq!(clamped.offset_y, 0.0);
    }

    #[test]
    fn icons_viewport_offset_is_reclamped_when_entries_shrink_below_it() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let root = PathBuf::from("/workspace");
        browser.current_dir = root.clone();
        browser.view_mode = BrowserViewMode::Icons;
        browser.entries = (0..5)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();
        browser.icon_grid_viewports.insert(
            browser.active_pane_id(),
            crate::app::PaneIconGridViewport {
                directory: root.clone(),
                viewport: crate::model::IconGridViewport {
                    offset_y: 20000.0,
                    width: 600.0,
                    height: 400.0,
                },
            },
        );

        drop(browser.update(crate::model::Message::ThemeModeSelected(
            crate::matugen_theme::ThemeMode::Dark,
        )));

        let clamped = browser
            .icon_grid_viewports
            .get(&browser.active_pane_id())
            .expect("viewport stays recorded");
        assert_eq!(clamped.viewport.offset_y, 0.0);
    }

    #[test]
    fn list_viewport_within_new_content_is_left_untouched() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let root = PathBuf::from("/workspace");
        browser.view_mode = BrowserViewMode::List;
        browser.entries = (0..500)
            .map(|index| test_entry(root.join(format!("item-{index}"))))
            .collect::<Vec<_>>()
            .into();
        browser.column_viewports.insert(
            root.clone(),
            ColumnViewport {
                offset_y: 4600.0,
                height: 800.0,
            },
        );

        drop(browser.update(crate::model::Message::ThemeModeSelected(
            crate::matugen_theme::ThemeMode::Dark,
        )));

        let viewport = browser
            .column_viewports
            .get(&root)
            .expect("viewport stays recorded");
        // 合法偏移不得被自愈误改(内容 500 行 × 46px 远超 4600+800)。
        assert_eq!(viewport.offset_y, 4600.0);
    }
}
