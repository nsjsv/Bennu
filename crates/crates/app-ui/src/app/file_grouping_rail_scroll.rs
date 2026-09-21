//! 分组索引栏的滚动定位:组序 → 组头顶点偏移 → 滚动目标偏移,并把
//! 目标主动回写视口状态。iced 编程 `scroll_to` 不回发 on_scroll——只发
//! 命令的话,滚动条缓存与"当前组高亮"都停在原地;因此滚动命令必须与
//! `ListScrolled`/`IconGridScrolled` 同款的状态写入成对出现。定位数据
//! 与渲染同源:列表取合并行流、网格取根面板 flow,不重算分组。

use iced::widget::scrollable;
use iced::Task;

use super::smooth_scroll::smooth_scroll_id;
use super::FileBrowser;
use crate::model::{
    file_group_scroll_target_offset, BrowserPaneId, BrowserViewMode, Message, ScrollbarRegion,
};

impl FileBrowser {
    /// 光标进入分组索引栏:栏是事件屏障,栏上无条目可指,条目 hover
    /// 必须显式清掉(滚动/换组时下层行的 exit 永远不会发出);同时把
    /// "光标在栏上"入账,滚动补偿重算据此跳过该窗格。
    pub(super) fn enter_file_grouping_rail(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        self.hovered_grouping_rail_pane = Some(pane_id);
        if pane_id != self.active_pane_id() {
            return Task::none();
        }
        // 走事件通道同一个清理出口:条目 hover 与其衍生的 spring 悬停
        // 候选一并回收,不另写一套 path-free 的清理逻辑。
        match self.hovered_entry.clone() {
            Some(hovered) => self.handle_entry_hover_cleared(hovered),
            None => Task::none(),
        }
    }

    /// 光标离开索引栏:屏障解除,滚动补偿恢复对该窗格生效。
    pub(super) fn exit_file_grouping_rail(&mut self, pane_id: BrowserPaneId) {
        if self.hovered_grouping_rail_pane == Some(pane_id) {
            self.hovered_grouping_rail_pane = None;
        }
    }

    pub(super) fn scroll_to_file_group_target(
        &mut self,
        pane_id: BrowserPaneId,
        group_index: usize,
    ) -> Task<Message> {
        // 平滑滚动惯性会与编程跳转互相拉扯,跳转前先停掉在途动画。
        self.smooth_scroll.stop();
        let Some(pane) = self.pane_view(pane_id) else {
            return Task::none();
        };
        match pane.view_mode {
            BrowserViewMode::List => {
                let geometry =
                    crate::list_view::ListGeometry::for_level(self.user_config().list_view_density);
                // 与列表渲染同一条合并行流,组头顶点偏移和内容高都从它
                // 累计,落点才与实际渲染一致。
                let rows = crate::transfer_placeholder_view::build_list_transfer_rows(
                    self,
                    pane,
                    geometry.row_height,
                );
                let entries = crate::transfer_placeholder_view::list_file_group_rail_entries(
                    &rows,
                    geometry.row_height,
                );
                let Some(entry) = entries.get(group_index) else {
                    return Task::none();
                };
                let viewport_height = pane
                    .column_viewports
                    .get(pane.current_dir)
                    .map(|viewport| viewport.height.max(1.0))
                    .unwrap_or(self.main_window_height);
                let content_height = crate::list_view::LIST_HEADER_HEIGHT
                    + crate::transfer_placeholder_view::list_transfer_rows_content_height(
                        &rows,
                        geometry.row_height,
                    );
                // 表头占据内容流顶部,组头顶点换算滚动偏移要加回表头
                // 留量,与 reveal/虚拟范围的口径一致。
                let target = file_group_scroll_target_offset(
                    entry.top_offset,
                    crate::list_view::LIST_HEADER_HEIGHT,
                    content_height,
                    viewport_height,
                );
                let region = ScrollbarRegion::PaneList(pane_id);
                Task::batch([
                    iced::widget::operation::scroll_to(
                        smooth_scroll_id(&region),
                        scrollable::AbsoluteOffset {
                            x: None,
                            y: Some(target),
                        },
                    ),
                    self.show_scrollbars_temporarily(region),
                    self.handle_list_scrolled(pane_id, target, viewport_height),
                ])
            }
            BrowserViewMode::Icons => {
                let layout = self.icon_grid_layout_for_pane(pane);
                let entries = layout.root().file_group_rail_entries();
                let Some(entry) = entries.get(group_index) else {
                    return Task::none();
                };
                let viewport = pane.icon_grid_viewport;
                let viewport_height = if viewport.height > f32::EPSILON {
                    viewport.height
                } else {
                    self.main_window_height
                };
                // 网格 flow 段的 top 已含内容内边距,组头顶点即内容空间
                // 偏移,无需表头留量。
                let target = file_group_scroll_target_offset(
                    entry.top_offset,
                    0.0,
                    layout.total_height(),
                    viewport_height,
                );
                let region = ScrollbarRegion::PaneIcons(pane_id);
                Task::batch([
                    iced::widget::operation::scroll_to(
                        smooth_scroll_id(&region),
                        scrollable::AbsoluteOffset {
                            x: None,
                            y: Some(target),
                        },
                    ),
                    self.show_scrollbars_temporarily(region),
                    self.handle_icon_grid_scrolled(
                        pane_id,
                        target,
                        viewport.width.max(1.0),
                        viewport_height,
                    ),
                ])
            }
            // 多栏视图不参与分组,索引栏不会出现在该模式下。
            BrowserViewMode::Columns => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::super::FileBrowser;
    use crate::config;
    use crate::icon_grid_geometry::{row_height, ICON_GRID_CONTENT_PADDING};
    use crate::icon_grid_layout::ICON_GRID_GROUP_HEADER_HEIGHT;
    use crate::list_view::{LIST_GROUP_HEADER_HEIGHT, LIST_HEADER_HEIGHT, LIST_ROW_HEIGHT};
    use crate::model::{BrowserPaneId, BrowserViewMode, FileGroupingMode, Message};
    use crate::thumbnail_cache::ColumnViewport;

    fn file_entry(path: &str) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from(path),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )
    }

    fn grouped_list_browser() -> (FileBrowser, PathBuf, BrowserPaneId) {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = PathBuf::from("/workspace");
        let pane_id = BrowserPaneId::PRIMARY;
        browser.view_mode = BrowserViewMode::List;
        browser.current_dir = root.clone();
        browser.entries = Arc::new(vec![
            file_entry("/workspace/a.txt"),
            file_entry("/workspace/z.txt"),
        ]);
        browser.user_config.file_grouping = FileGroupingMode::NameInitial;
        (browser, root, pane_id)
    }

    #[test]
    fn rail_target_message_scrolls_list_and_writes_back_viewport_state() {
        let (mut browser, root, pane_id) = grouped_list_browser();
        // 行流:[组头A(32), a(46), 组头Z(32), z(46)],内容 156 + 表头 32;
        // 视口高 100 时可支撑的最大偏移 = 188 - 100 = 88。
        browser.column_viewports.insert(
            root.clone(),
            ColumnViewport {
                offset_y: 0.0,
                height: 100.0,
            },
        );

        drop(browser.update(Message::FileGroupingRailTargetSelected {
            pane: pane_id,
            group_index: 0,
        }));
        // 组A顶点 0 + 表头留量 32 = 32;ListScrolled 同款状态写入必须发生
        // (scroll_to 不回发 on_scroll,高亮与滚动条靠这次回写活过来)。
        assert_eq!(
            browser.column_viewports.get(&root).unwrap().offset_y,
            LIST_HEADER_HEIGHT
        );

        // 组Z顶点 78,目标 78 + 32 = 110,夹到内容尾部 88。
        drop(browser.update(Message::FileGroupingRailTargetSelected {
            pane: pane_id,
            group_index: 1,
        }));
        let max_offset =
            LIST_HEADER_HEIGHT + (LIST_GROUP_HEADER_HEIGHT + LIST_ROW_HEIGHT) * 2.0 - 100.0;
        assert_eq!(
            browser.column_viewports.get(&root).unwrap().offset_y,
            max_offset
        );
    }

    #[test]
    fn rail_target_message_scrolls_icon_grid_and_writes_back_viewport_state() {
        let (mut browser, root, pane_id) = grouped_list_browser();
        browser.view_mode = BrowserViewMode::Icons;
        // 预置网格视口(高度 120):IconGridScrolled 的状态写入出口。
        drop(browser.handle_icon_grid_scrolled(pane_id, 0.0, 600.0, 120.0));

        // flow:组头A(顶点=内边距) + 组A行段 + 组头Z + 组Z行段,各 1 行;
        // 组Z顶点即滚动目标(网格无表头留量),受内容尾夹取。
        let edge = browser.user_config.icons_icon_edge();
        let header_z_top =
            ICON_GRID_CONTENT_PADDING + row_height(edge) + ICON_GRID_GROUP_HEADER_HEIGHT;
        let content_height = ICON_GRID_CONTENT_PADDING * 2.0
            + ICON_GRID_GROUP_HEADER_HEIGHT * 2.0
            + row_height(edge) * 2.0;
        let expected = header_z_top.min((content_height - 120.0).max(0.0));

        drop(browser.update(Message::FileGroupingRailTargetSelected {
            pane: pane_id,
            group_index: 1,
        }));
        let state = browser
            .icon_grid_viewports
            .get(&pane_id)
            .expect("icon grid viewport recorded");
        assert_eq!(state.directory, root);
        assert_eq!(state.viewport.offset_y, expected);
    }
}
