//! 重读窗格内容。入口按为什么重读分：用户要求（F5、切标签、恢复会话）、列表规则变了、
//! 文件操作做完。磁盘监视报来的变化不是用户操作，直接用 `reload_pane`（见 `reload_observed_directory`）。
//! 文件夹项数要不要重数各入口自己定：按操作精确失效的调用方先失效再调。

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::{load_directory_command, load_expanded_directory_command};
use crate::model::{
    BrowserLocation, BrowserPaneId, DirectoryCollectionPhase, LocationKey, Message,
};

impl FileBrowser {
    /// F5、恢复会话：重读所有看得见的窗格。
    pub(super) fn reload_visible_panes(&mut self) -> Task<Message> {
        self.clear_transient_interaction_state();
        self.reload_panes_on_request(self.pane_layout.visible_pane_ids())
    }

    /// 切标签、在旁边窗格打开文件夹、从桌面打开：重读活动窗格。
    pub(super) fn reload_current(&mut self) -> Task<Message> {
        self.clear_transient_interaction_state();
        self.reload_panes_on_request(vec![self.active_pane_id()])
    }

    /// 拖着文件停在标签上切了过去：拖拽还没完，不清交互状态。
    pub(super) fn reload_current_for_file_drop(&mut self) -> Task<Message> {
        self.reload_panes_on_request(vec![self.active_pane_id()])
    }

    /// 显不显示隐藏文件、怎么排序是所有窗格共用的列表规则：变了以后可见窗格都按新规则重读
    /// （后台标签切过去时本来就重读）。回收站快照按旧规则拉，正在拉的作废重拉。目录摘要
    /// 隐藏的也数、和顺序无关，留着。
    pub(super) fn relist_visible_panes(&mut self) -> Task<Message> {
        self.invalidate_trash_snapshot_refresh();
        self.clear_transient_interaction_state();
        self.reload_panes(self.pane_layout.visible_pane_ids())
    }

    /// 文件操作做完：回收站快照另外刷一次（后台标签显示的也算），看得见的目录重扫。不打断
    /// 用户正在做的事。
    pub(super) fn reload_visible_panes_after_file_operation(&mut self) -> Task<Message> {
        let trash = self.refresh_trash_snapshot_after_change();
        // 搜索结果在操作完成时已就地更新（改名跟着走、删掉的拿掉），不整次重搜。
        let rescanned_panes = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter(|pane_id| {
                self.pane_by_id(*pane_id)
                    .is_some_and(|pane| pane.tab().location.search().is_none())
            })
            .collect();
        Task::batch([trash, self.reload_panes(rescanned_panes)])
    }

    /// 用户要求的重读：文件夹项数重数；显示着回收站时，正在读的那次可能早于用户看到的变化，
    /// 读完还得再读一次。
    fn reload_panes_on_request(&mut self, pane_ids: Vec<BrowserPaneId>) -> Task<Message> {
        let mut shows_trash = false;
        for &pane_id in &pane_ids {
            self.invalidate_list_directory_summaries_for_pane(pane_id);
            shows_trash |= self
                .pane_by_id(pane_id)
                .is_some_and(|pane| pane.tab().location == BrowserLocation::Trash);
        }
        let trash = if shows_trash {
            self.refresh_trash_snapshot_after_change()
        } else {
            Task::none()
        };
        Task::batch([trash, self.reload_panes(pane_ids)])
    }

    fn reload_panes(&mut self, pane_ids: Vec<BrowserPaneId>) -> Task<Message> {
        Task::batch(
            pane_ids
                .into_iter()
                .map(|pane_id| self.reload_pane(pane_id))
                .collect::<Vec<_>>(),
        )
    }

    /// 重新加载窗格活动标签的内容：回收站视图重拉快照，搜索结果按同样的意图重搜，
    /// 其余重扫目录并刷新已展开的子目录。图标展开只属于活动窗格的交互上下文。
    pub(super) fn reload_pane(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        let is_active = pane_id == self.active_pane_id();
        let has_trash_snapshot = self.trash_refresh.snapshot().is_some();
        let location = self
            .pane_by_id(pane_id)
            .map(|pane| pane.tab().location.key())
            .expect("a reload targets an existing pane");
        let current_dir = match location {
            LocationKey::Search(session_id) => return self.restart_search(session_id),
            LocationKey::Trash => {
                if is_active {
                    self.clear_icon_grid_expansion();
                }
                let pane = self
                    .pane_by_id_mut(pane_id)
                    .expect("a reload targets an existing pane");
                pane.directory_loading_placeholder = None;
                let tab = pane.tab_mut();
                tab.directory_collection_phase = if has_trash_snapshot {
                    DirectoryCollectionPhase::Ready
                } else {
                    DirectoryCollectionPhase::Discovering
                };
                tab.close_expanded_directories();
                pane.cancel_directory_load();
                self.retract_global_error(GlobalErrorSource::DirectoryLoad);
                self.prune_list_directory_summaries_to_live_roots();
                return self.request_trash_snapshot_refresh();
            }
            LocationKey::Directory(directory) => directory,
        };
        let pane = self
            .pane_by_id_mut(pane_id)
            .expect("a reload targets an existing pane");
        pane.directory_loading_placeholder = None;
        let tab = pane.tab_mut();
        // 已显示完整列表时是刷新：旧列表一直留着，结果落地整体替换。
        tab.directory_collection_phase = match tab.directory_collection_phase {
            DirectoryCollectionPhase::Discovering => DirectoryCollectionPhase::Discovering,
            DirectoryCollectionPhase::Ready | DirectoryCollectionPhase::Refreshing => {
                DirectoryCollectionPhase::Refreshing
            }
        };
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);

        let mut commands = self.refresh_expanded_directory_commands(pane_id);
        if is_active {
            commands.extend(self.refresh_icon_grid_expansion_commands());
        }
        let (request, cancellation) = self.begin_directory_load(pane_id, current_dir);
        commands.push(load_directory_command(
            request,
            self.listing_options(pane_id),
            cancellation,
        ));
        Task::batch(commands)
    }

    /// 把窗格活动标签里已展开的目录全部置为加载中并发起重新加载。
    fn refresh_expanded_directory_commands(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Vec<Task<Message>> {
        let expanded_directories = &mut self
            .pane_by_id_mut(pane_id)
            .expect("a reload targets an existing pane")
            .tab_mut()
            .expanded_directories;
        let paths = expanded_directories.keys().cloned().collect::<Vec<_>>();
        for expanded in expanded_directories.values_mut() {
            expanded.status = expanded.status.reloading();
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = 1.0;
        }

        paths
            .into_iter()
            .filter_map(|path| {
                let load_context = self.expanded_directory_load_context(pane_id, &path);
                let expanded = self
                    .pane_by_id_mut(pane_id)?
                    .tab_mut()
                    .expanded_directories
                    .get_mut(&path)?;
                let (request, cancellation) =
                    Self::next_expanded_directory_load_request(load_context, path, expanded);
                Some(load_expanded_directory_command(
                    request,
                    self.listing_options(pane_id),
                    cancellation,
                ))
            })
            .collect()
    }
}
