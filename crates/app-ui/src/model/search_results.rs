//! 搜索位置上的结果：命中转成标签的普通条目（列表、网格、选择、拖动、重命名都按普通
//! 文件处理），条目装不下的摘要与高亮记在搜索状态的详情里，两边在这里一起变。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use file_core::{DirectoryEntry, DirectoryMetadataAvailability, EntryMetadata, FileKind};
use file_operations::{path_after_completed_migrations, CompletedPathMigration};
use file_search::{SearchFileKind, SearchHit, SearchQuery, SearchResultBatch};
use tokio_util::sync::CancellationToken;

use super::browser_panes::BrowserTab;
use super::search::{
    DirectoryFallbackOutcome, IndexedSearchRequest, SearchHitsPlacement, SearchRunId,
    SEARCH_RESULT_TOTAL_LIMIT,
};
use super::DirectoryCollectionPhase;

impl BrowserTab {
    /// 发起一次新查询：旧结果和选择清掉，等第一页。
    pub(crate) fn begin_search_query(
        &mut self,
        query: SearchQuery,
    ) -> Option<(IndexedSearchRequest, CancellationToken)> {
        let started = self.location.search_mut()?.begin_indexed_query(query);
        self.clear_search_entries();
        Some(started)
    }

    /// 还有下一页且没在等时发起下一页。
    pub(crate) fn begin_next_search_page(
        &mut self,
    ) -> Option<(IndexedSearchRequest, SearchQuery, CancellationToken)> {
        let loaded = self.entries.len();
        let search = self.location.search_mut()?;
        if !search.indexed_next_page_is_available(loaded) {
            return None;
        }
        search.begin_next_indexed_page()
    }

    /// 收下索引的一页；不是这个标签在等的回信时返回 false。
    pub(crate) fn accept_search_batch(
        &mut self,
        request: IndexedSearchRequest,
        batch: SearchResultBatch,
    ) -> bool {
        let Some(search) = self
            .location
            .search_mut()
            .filter(|search| search.accepts_indexed_outcome(request))
        else {
            return false;
        };
        let (placement, hits) = search.accept_indexed_batch(request, batch);
        self.place_search_hits(placement, hits);
        true
    }

    /// 第一页失败：之前的结果不再算数。
    pub(crate) fn accept_search_failure(&mut self, request: IndexedSearchRequest, message: String) {
        let Some(search) = self.location.search_mut() else {
            return;
        };
        if search.apply_indexed_failure(request, message) {
            self.clear_search_entries();
        }
    }

    /// 本地回退扫描的一批结果，接在后面。
    pub(crate) fn accept_fallback_hits(&mut self, run: SearchRunId, hits: Vec<SearchHit>) {
        let Some(search) = self
            .location
            .search_mut()
            .filter(|search| search.accepts_directory_fallback(run))
        else {
            return;
        };
        search.accept_directory_batch();
        self.place_search_hits(SearchHitsPlacement::Append, hits);
    }

    pub(crate) fn finish_fallback_search(
        &mut self,
        run: SearchRunId,
        outcome: DirectoryFallbackOutcome,
    ) {
        if let Some(search) = self
            .location
            .search_mut()
            .filter(|search| search.accepts_directory_fallback(run))
        {
            search.finish_directory_fallback(outcome);
        }
    }

    /// 清空输入或筛选失效：结果清空，留在搜索里。
    pub(crate) fn clear_search_query(&mut self) {
        if let Some(search) = self.location.search_mut() {
            search.clear_query();
            self.clear_search_entries();
        }
    }

    pub(crate) fn reject_search_query(&mut self, message: String) {
        if let Some(search) = self.location.search_mut() {
            search.reject_query(message);
            self.clear_search_entries();
        }
    }

    /// 这些路径（及它们下面的结果）已被删除：从结果里拿掉，详情和选择一起收掉。
    /// 有结果被拿掉时返回 true。
    pub(crate) fn remove_search_results(&mut self, removed: &[PathBuf]) -> bool {
        let Some(search) = self.location.search_mut() else {
            return false;
        };
        let is_removed =
            |path: &std::path::Path| removed.iter().any(|removed| path.starts_with(removed));
        if !self.entries.iter().any(|entry| is_removed(&entry.path)) {
            return false;
        }
        search.details.retain(|path, _| !is_removed(path));
        Arc::make_mut(&mut self.entries).retain(|entry| !is_removed(&entry.path));
        self.retain_direct_entry_selection();
        true
    }

    /// 改名或移动后结果跟着走：条目路径、详情、选择、起点文件夹都换成新路径，名字高亮按
    /// 新名字重算。
    pub(super) fn migrate_search_results(&mut self, migrations: &[CompletedPathMigration]) {
        let Some(search) = self.location.search_mut() else {
            return;
        };
        search.root.migrate_origin(migrations);
        let mut moved = Vec::new();
        for entry in Arc::make_mut(&mut self.entries) {
            let migrated = path_after_completed_migrations(&entry.path, migrations);
            if migrated == entry.path {
                continue;
            }
            let previous = std::mem::replace(&mut entry.path, migrated);
            entry.name = entry
                .path
                .file_name()
                .unwrap_or_else(|| entry.path.as_os_str())
                .to_os_string();
            moved.push((
                previous,
                entry.path.clone(),
                entry.name.to_string_lossy().into_owned(),
            ));
        }
        search.migrate_details(moved);
        for path in self
            .selected
            .iter_mut()
            .chain(self.selection_anchor.iter_mut())
        {
            *path = path_after_completed_migrations(path, migrations);
        }
        self.selected_paths = self
            .selected_paths
            .drain()
            .map(|path| path_after_completed_migrations(&path, migrations))
            .collect();
    }

    fn clear_search_entries(&mut self) {
        Arc::make_mut(&mut self.entries).clear();
        self.clear_selection();
        self.directory_collection_phase = DirectoryCollectionPhase::Ready;
    }

    fn place_search_hits(&mut self, placement: SearchHitsPlacement, hits: Vec<SearchHit>) {
        let Some(search) = self.location.search_mut() else {
            return;
        };
        let entries = hits.iter().map(entry_from_hit).collect::<Vec<_>>();
        let display_names = entries
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if placement == SearchHitsPlacement::Replace {
            search.details.clear();
        }
        search.remember_details(&hits, &display_names);
        let listing = Arc::make_mut(&mut self.entries);
        if placement == SearchHitsPlacement::Replace {
            listing.clear();
        }
        listing.extend(entries);
        let excess = listing.len().saturating_sub(SEARCH_RESULT_TOTAL_LIMIT);
        if excess > 0 {
            for dropped in listing.drain(..excess) {
                search.details.remove(&dropped.path);
            }
            search.note_truncated();
        }
        self.directory_collection_phase = DirectoryCollectionPhase::Ready;
        self.retain_direct_entry_selection();
    }
}

/// 一条命中当作普通条目：大小与时间来自索引；属主、权限索引里没有，标为拿不到。
fn entry_from_hit(hit: &SearchHit) -> DirectoryEntry {
    let kind = match hit.kind {
        SearchFileKind::Directory => FileKind::Directory,
        SearchFileKind::File => FileKind::File,
        SearchFileKind::Symlink => FileKind::Symlink,
        SearchFileKind::Other => FileKind::Other,
    };
    let is_hidden = hit
        .path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'));
    DirectoryEntry::new(
        hit.path.clone(),
        kind,
        EntryMetadata {
            filesystem_availability: DirectoryMetadataAvailability::Complete,
            identity_names_availability: DirectoryMetadataAvailability::Unavailable,
            len: hit.size,
            modified: hit.modified_ms.map(system_time_from_ms),
            accessed: hit.accessed_ms.map(system_time_from_ms),
            created: hit.created_ms.map(system_time_from_ms),
            ..EntryMetadata::default()
        },
        is_hidden,
        kind == FileKind::Symlink,
        false,
    )
}

fn system_time_from_ms(milliseconds: i64) -> SystemTime {
    match u64::try_from(milliseconds) {
        Ok(after) => UNIX_EPOCH + Duration::from_millis(after),
        Err(_) => UNIX_EPOCH - Duration::from_millis(milliseconds.unsigned_abs()),
    }
}

#[cfg(test)]
#[path = "search_results_tests.rs"]
mod tests;
