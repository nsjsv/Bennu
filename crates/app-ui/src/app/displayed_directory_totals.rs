//! 状态栏的当前目录统计（可见文件总大小、隐藏条目数）。要当前目录全部文件的大小：后台把它们的
//! 文件信息补齐，写进条目本身（大小列、按大小排序、选中统计、缩略图都直接用上），再由条目求和。
//! 「当前目录」与地址栏同源：多栏取最深打开的那一栏。

use file_core::{DirectoryDiscovery, DirectoryMetadataRequirement, FileKind};
use iced::Task;

use super::FileBrowser;
use crate::app::panes::BrowserPaneView;
use crate::model::{BrowserPaneId, DirectoryEntrySnapshot, DirectoryMetadataLoadContext, Message};
use crate::selection_summary::{directory_file_totals, DirectoryFileTotals, DirectoryTotalsKey};

/// 状态栏那个目录显示着的条目和它的扫描结果。
struct DisplayedListing<'a> {
    context: DirectoryMetadataLoadContext,
    entries: &'a DirectoryEntrySnapshot,
    discovery: &'a DirectoryDiscovery,
    cancellation: tokio_util::sync::CancellationToken,
    /// 按大小、修改时间排序，还在等全部条目的文件信息才能定顺序。
    order_waits_for_metadata: bool,
}

impl FileBrowser {
    fn displayed_listing<'a>(&'a self, pane: BrowserPaneView<'a>) -> Option<DisplayedListing<'a>> {
        let directory = pane.address_bar_directory()?;
        if pane.current_directory() == Some(directory) {
            return Some(DisplayedListing {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: pane.id,
                    path: directory.to_path_buf(),
                    collection_generation: self.pane_by_id(pane.id)?.directory_load_generation,
                },
                entries: pane.entries,
                discovery: pane.directory_discovery?,
                cancellation: self.directory_load_cancel_for_pane(pane.id),
                order_waits_for_metadata: !self
                    .pane_by_id(pane.id)?
                    .tab()
                    .directory_order_phase
                    .is_ready(),
            });
        }
        let expanded = pane
            .expanded_directories
            .get(directory)
            .filter(|expanded| expanded.status.has_listing())?;
        Some(DisplayedListing {
            context: DirectoryMetadataLoadContext::Expanded {
                pane_id: pane.id,
                path: directory.to_path_buf(),
                load_generation: expanded.load_generation,
            },
            entries: &expanded.entries,
            discovery: expanded.directory_discovery.as_ref()?,
            cancellation: expanded.load_cancel.clone().unwrap_or_default(),
            order_waits_for_metadata: !expanded.directory_order_phase.is_ready(),
        })
    }

    /// 把状态栏那个目录要的文件信息补齐（已补上、正在补的不再要）：全部文件的大小（文件夹不计入
    /// 总大小）；按大小、修改时间排序还没定顺序时连文件夹一起要——原来只有列表视图要，图标、分栏
    /// 视图的顺序一直停在等文件信息。每次滚动都会走到这里，都知道了就直接返回，不再挨个看条目。
    pub(super) fn schedule_displayed_directory_metadata(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        let totals_known = self
            .displayed_directory_totals(pane_id)
            .is_none_or(|totals| totals.visible_files_bytes.is_some());
        let Some((context, discovery, cancellation, every_entry)) =
            self.pane_view(pane_id).and_then(|pane| {
                let listing = self.displayed_listing(pane)?;
                Some((
                    listing.context,
                    listing.discovery.clone(),
                    listing.cancellation,
                    listing.order_waits_for_metadata,
                ))
            })
        else {
            return Task::none();
        };
        if totals_known && !every_entry {
            return Task::none();
        }
        let wanted = discovery
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| every_entry || entry.kind() != FileKind::Directory)
            .map(|(index, _)| index);
        self.schedule_directory_metadata_requirement(
            context,
            &discovery,
            cancellation,
            DirectoryMetadataRequirement::Filesystem,
            wanted,
        )
    }

    /// 状态栏的当前目录统计；目录还没扫完（或不是目录，如回收站、搜索）时没有。
    pub(super) fn displayed_directory_totals(
        &self,
        pane_id: BrowserPaneId,
    ) -> Option<DirectoryFileTotals> {
        let pane = self.pane_by_id(pane_id)?;
        let listing = self.displayed_listing(self.pane_view(pane_id)?)?;
        Some(directory_file_totals(
            &pane.directory_totals,
            DirectoryTotalsKey {
                entries_revision: listing.entries.revision(),
                show_hidden: self.user_config.show_hidden_files,
                hidden_entries_seen: listing.discovery.hidden_entries,
                entry_metadata_revision: self.entry_metadata_revision,
            },
            listing.entries.iter(),
        ))
    }
}

#[cfg(test)]
#[path = "displayed_directory_totals_tests.rs"]
mod tests;
