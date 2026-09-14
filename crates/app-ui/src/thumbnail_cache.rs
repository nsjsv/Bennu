use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_core::{DirectoryEntry, FileKind, TransferConflictMetadata};
use iced::widget::image;
use thumbnails::{CachedThumbnail, ThumbnailKey, ThumbnailRequest, ThumbnailSourceMetadata};

pub(crate) const LIST_THUMBNAIL_EDGE: u32 = 96;
pub(crate) const LIST_THUMBNAIL_SIZE: f32 = 42.0;
pub(crate) const COLUMN_THUMBNAIL_EDGE: u32 = 48;
pub(crate) const COLUMN_THUMBNAIL_SIZE: f32 = 18.0;
pub(crate) const TRANSFER_CONFLICT_THUMBNAIL_EDGE: u32 = 96;
pub(crate) const PREVIEW_THUMBNAIL_MAX_EDGE: u32 = 2048;

/// 内存就绪缩略图上限：Handle 仅持路径不驻留像素，2048 项内存成本可忽略。
const MAX_READY_THUMBNAILS: usize = 2048;
/// 解码并发跟随可用核数：IO+CPU 混合负载，限制在 2..=8 防止小核机器过载。
fn max_in_flight() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .clamp(2, 8)
}

const PREVIEW_IN_FLIGHT_EXTRA: usize = 1;

const FAILURE_BACKOFF: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThumbnailPurpose {
    List,
    Preview,
    TransferConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThumbnailLoadPolicy {
    CacheOnly,
    LoadOrGenerate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThumbnailPriority {
    Background,
    Visible,
    Focused,
    Preview,
}

#[derive(Debug, Clone)]
pub(crate) struct ThumbnailWork {
    pub(crate) request: ThumbnailRequest,
    pub(crate) purpose: ThumbnailPurpose,
    pub(crate) priority: ThumbnailPriority,
    pub(crate) load_policy: ThumbnailLoadPolicy,
    pub(crate) scope: Option<ThumbnailScope>,
}

impl ThumbnailWork {
    pub(crate) fn key(&self) -> ThumbnailKey {
        self.request.key()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ThumbnailLoadOutcome {
    pub(crate) work: ThumbnailWork,
    pub(crate) result: ThumbnailLoadResult,
}

#[derive(Debug, Clone)]
pub(crate) enum ThumbnailLoadResult {
    Ready(CachedThumbnail),
    CacheMiss,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ThumbnailHandleEntry {
    pub(crate) handle: image::Handle,
    /// 请求源路径：就绪表只按 ThumbnailKey(哈希) 索引,预览会话需要
    /// 按源路径回查"当前已有的最大尺寸缩略图",必须冗余记录。
    pub(crate) source: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) max_edge: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ColumnViewport {
    pub(crate) offset_y: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ThumbnailScope {
    PaneDirectory { pane_id: u64, directory: PathBuf },
}

#[derive(Debug)]
pub(crate) struct ThumbnailCache {
    cache_dir: PathBuf,
    ready: HashMap<ThumbnailKey, ThumbnailHandleEntry>,
    ready_order: VecDeque<ThumbnailKey>,
    queued: HashMap<ThumbnailKey, ThumbnailWork>,
    queue_order: VecDeque<ThumbnailKey>,
    inflight: HashMap<ThumbnailKey, ThumbnailPurpose>,
    failures: HashMap<ThumbnailKey, Instant>,
}

impl ThumbnailCache {
    pub(crate) fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            ready: HashMap::new(),
            ready_order: VecDeque::new(),
            queued: HashMap::new(),
            queue_order: VecDeque::new(),
            inflight: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    pub(crate) fn set_cache_dir(&mut self, cache_dir: PathBuf) {
        if self.cache_dir == cache_dir {
            return;
        }
        self.cache_dir = cache_dir;
        self.ready.clear();
        self.ready_order.clear();
        self.queued.clear();
        self.queue_order.clear();
        self.inflight.clear();
        self.failures.clear();
    }

    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    pub(crate) fn enqueue_request(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
    ) -> bool {
        self.enqueue_request_with_scope(
            request,
            purpose,
            priority,
            ThumbnailLoadPolicy::LoadOrGenerate,
            None,
        )
    }

    /// 与 `enqueue_request` 同语义的 CacheOnly 变体：只读磁盘缓存不生成,
    /// 返回是否需要等待结果事件。
    pub(crate) fn enqueue_cached_request(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
    ) -> bool {
        self.enqueue_request_with_scope(
            request,
            purpose,
            priority,
            ThumbnailLoadPolicy::CacheOnly,
            None,
        )
    }

    pub(crate) fn enqueue_request_for_scope(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
        scope: ThumbnailScope,
    ) {
        self.enqueue_request_with_scope(
            request,
            purpose,
            priority,
            ThumbnailLoadPolicy::LoadOrGenerate,
            Some(scope),
        );
    }

    pub(crate) fn enqueue_cached_request_for_scope(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
        scope: ThumbnailScope,
    ) {
        self.enqueue_request_with_scope(
            request,
            purpose,
            priority,
            ThumbnailLoadPolicy::CacheOnly,
            Some(scope),
        );
    }

    pub(crate) fn prune_scope_except(&mut self, scope: &ThumbnailScope, keep: &[ThumbnailKey]) {
        self.queued.retain(|key, work| {
            work.scope.as_ref() != Some(scope) || keep.iter().any(|keep_key| keep_key == key)
        });
        self.queue_order.retain(|key| self.queued.contains_key(key));
    }

    pub(crate) fn retain_queued_work(&mut self, mut keep_work: impl FnMut(&ThumbnailWork) -> bool) {
        self.queued.retain(|_, work| keep_work(work));
        self.queue_order.retain(|key| self.queued.contains_key(key));
    }

    fn enqueue_request_with_scope(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
        load_policy: ThumbnailLoadPolicy,
        scope: Option<ThumbnailScope>,
    ) -> bool {
        if self.request_is_inside_cache_dir(&request) {
            return false;
        }
        let key = request.key();
        if !self.thumbnail_can_be_queued(&key, load_policy) {
            return self.inflight.get(&key) == Some(&ThumbnailPurpose::Preview);
        }

        let work = ThumbnailWork {
            request,
            purpose,
            priority,
            load_policy,
            scope,
        };
        if let Some(existing) = self.queued.get_mut(&key) {
            if existing.priority < priority || existing.load_policy < load_policy {
                *existing = work;
            } else if existing.scope.is_none() {
                existing.scope = work.scope;
            }
            return true;
        }

        self.queued.insert(key.clone(), work);
        self.queue_order.push_back(key);
        true
    }

    pub(crate) fn take_next_batch(&mut self) -> Vec<ThumbnailWork> {
        if self.queued.is_empty() {
            return Vec::new();
        }

        let mut works = Vec::new();
        let available = max_in_flight().saturating_sub(self.inflight.len());
        for _ in 0..available {
            let Some(work) = self.take_highest_priority_work() else {
                break;
            };
            works.push(work);
        }
        if self.preview_extra_slot_is_available() {
            if let Some(work) = self.take_highest_priority_preview_work() {
                works.push(work);
            }
        }
        works
    }

    pub(crate) fn finish(&mut self, key: &ThumbnailKey) {
        self.inflight.remove(key);
    }

    pub(crate) fn insert_ready(
        &mut self,
        thumbnail: CachedThumbnail,
        max_edge: u32,
    ) -> ThumbnailHandleEntry {
        let key = thumbnail.key.clone();
        let entry = ThumbnailHandleEntry {
            handle: image::Handle::from_path(thumbnail.output.clone()),
            source: thumbnail.source.clone(),
            width: thumbnail.width,
            height: thumbnail.height,
            max_edge,
        };

        self.ready.insert(key.clone(), entry.clone());
        self.ready_order.retain(|ready_key| ready_key != &key);
        self.ready_order.push_back(key);
        self.trim_ready_cache();
        entry
    }

    pub(crate) fn ready_for_entry(
        &self,
        entry: &DirectoryEntry,
        max_edge: u32,
    ) -> Option<&ThumbnailHandleEntry> {
        let request = request_for_entry(entry, max_edge)?;
        if self.request_is_inside_cache_dir(&request) {
            return None;
        }
        self.ready.get(&request.key())
    }

    pub(crate) fn ready_for_request(
        &self,
        request: &ThumbnailRequest,
    ) -> Option<&ThumbnailHandleEntry> {
        self.ready.get(&request.key())
    }

    /// 同一源路径下已就绪的最大尺寸缩略图（不限请求档位）：
    /// 预览会话用它把列表阶段已生成的缩略图直接抬上屏。
    pub(crate) fn largest_ready_for_source(
        &self,
        source: &Path,
    ) -> Option<&ThumbnailHandleEntry> {
        self.ready
            .values()
            .filter(|entry| entry.source == source)
            .max_by_key(|entry| entry.max_edge)
    }

    pub(crate) fn mark_failure(&mut self, key: ThumbnailKey) {
        self.failures.insert(key, Instant::now());
    }

    fn failure_is_active(&mut self, key: &ThumbnailKey) -> bool {
        let Some(failed_at) = self.failures.get(key).copied() else {
            return false;
        };
        if failed_at.elapsed() <= FAILURE_BACKOFF {
            return true;
        }
        self.failures.remove(key);
        false
    }

    fn thumbnail_can_be_queued(
        &mut self,
        key: &ThumbnailKey,
        load_policy: ThumbnailLoadPolicy,
    ) -> bool {
        !self.ready.contains_key(key)
            && !self.inflight.contains_key(key)
            && (load_policy == ThumbnailLoadPolicy::CacheOnly || !self.failure_is_active(key))
    }

    fn request_is_inside_cache_dir(&self, request: &ThumbnailRequest) -> bool {
        thumbnails::path_is_in_thumbnail_cache(&self.cache_dir, &request.source)
    }

    fn take_highest_priority_work(&mut self) -> Option<ThumbnailWork> {
        self.take_highest_priority_work_matching(|_| true)
    }

    fn take_highest_priority_preview_work(&mut self) -> Option<ThumbnailWork> {
        self.take_highest_priority_work_matching(|work| work.purpose == ThumbnailPurpose::Preview)
    }

    fn take_highest_priority_work_matching(
        &mut self,
        accepts_work: impl FnMut(&ThumbnailWork) -> bool,
    ) -> Option<ThumbnailWork> {
        let key = self.pop_highest_priority_key_matching(accepts_work)?;
        self.start_thumbnail_work(key)
    }

    fn start_thumbnail_work(&mut self, key: ThumbnailKey) -> Option<ThumbnailWork> {
        let work = self.queued.remove(&key)?;
        self.inflight.insert(key, work.purpose);
        Some(work)
    }

    fn pop_highest_priority_key_matching(
        &mut self,
        mut accepts_work: impl FnMut(&ThumbnailWork) -> bool,
    ) -> Option<ThumbnailKey> {
        let mut best_index = None;
        let mut best_priority = ThumbnailPriority::Background;

        for (index, key) in self.queue_order.iter().enumerate() {
            let Some(work) = self.queued.get(key) else {
                continue;
            };
            if !accepts_work(work) {
                continue;
            }
            if best_index.is_none() || work.priority > best_priority {
                best_index = Some(index);
                best_priority = work.priority;
            }
        }

        let index = best_index?;
        self.queue_order.remove(index)
    }

    fn preview_extra_slot_is_available(&self) -> bool {
        self.inflight.len() >= max_in_flight()
            && self
                .inflight
                .values()
                .filter(|purpose| **purpose == ThumbnailPurpose::Preview)
                .count()
                < PREVIEW_IN_FLIGHT_EXTRA
    }

    fn trim_ready_cache(&mut self) {
        while self.ready.len() > MAX_READY_THUMBNAILS {
            let Some(key) = self.ready_order.pop_front() else {
                break;
            };
            self.ready.remove(&key);
        }
        // 失败表过期即清扫：上界恒等于最近一个退避周期内的失败数，
        // 避免视图早已移走的失败键永久滞留内存。
        self.sweep_expired_failures(Instant::now());
    }

    fn sweep_expired_failures(&mut self, now: Instant) {
        self.failures
            .retain(|_, failed_at| now.duration_since(*failed_at) <= FAILURE_BACKOFF);
    }
}

pub(crate) fn request_for_entry(entry: &DirectoryEntry, max_edge: u32) -> Option<ThumbnailRequest> {
    if entry.kind != FileKind::File || !thumbnails::is_supported_thumbnail_path(&entry.path) {
        return None;
    }

    Some(ThumbnailRequest::new(
        &entry.path,
        ThumbnailSourceMetadata::from(&entry.metadata),
        max_edge,
    ))
}

pub(crate) fn request_for_transfer_conflict_path(
    path: &Path,
    metadata: &TransferConflictMetadata,
    max_edge: u32,
) -> Option<ThumbnailRequest> {
    if metadata.is_directory || !thumbnails::is_supported_thumbnail_path(path) {
        return None;
    }

    Some(ThumbnailRequest::new(
        path,
        ThumbnailSourceMetadata {
            len: metadata.len,
            modified: metadata.modified,
        },
        max_edge,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_only_work_dispatches_with_its_policy_intact() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        let request = thumbnail_request("photo.png", 1);

        let waits = cache.enqueue_cached_request(
            request.clone(),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );
        assert!(waits);

        // 派发链路不得改写 CacheOnly:该策略保证只读磁盘缓存,零解码。
        let batch = cache.take_next_batch();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].load_policy, ThumbnailLoadPolicy::CacheOnly);
        assert_eq!(batch[0].request, request);
    }

    fn thumbnail_request(file_name: &str, len: u64) -> ThumbnailRequest {
        ThumbnailRequest::new(
            PathBuf::from(file_name),
            ThumbnailSourceMetadata {
                len,
                modified: None,
            },
            LIST_THUMBNAIL_EDGE,
        )
    }

    #[test]
    fn preview_work_uses_extra_slot_when_list_work_fills_normal_slots() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        for index in 0..max_in_flight() {
            cache.enqueue_request(
                thumbnail_request(&format!("list-{index}.png"), index as u64),
                ThumbnailPurpose::List,
                ThumbnailPriority::Focused,
            );
        }

        let first_batch = cache.take_next_batch();

        assert_eq!(first_batch.len(), max_in_flight());
        assert!(first_batch
            .iter()
            .all(|work| work.purpose == ThumbnailPurpose::List));

        let preview_request = thumbnail_request("preview.png", 3);
        cache.enqueue_request(
            preview_request.clone(),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );

        let preview_batch = cache.take_next_batch();

        assert_eq!(preview_batch.len(), PREVIEW_IN_FLIGHT_EXTRA);
        assert_eq!(preview_batch[0].purpose, ThumbnailPurpose::Preview);
        assert_eq!(preview_batch[0].request, preview_request);
    }

    #[test]
    fn preview_extra_slot_allows_only_one_preview_work() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        for index in 0..max_in_flight() {
            cache.enqueue_request(
                thumbnail_request(&format!("list-{index}.png"), index as u64),
                ThumbnailPurpose::List,
                ThumbnailPriority::Focused,
            );
        }
        assert_eq!(cache.take_next_batch().len(), max_in_flight());

        cache.enqueue_request(
            thumbnail_request("preview-a.png", 3),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );
        assert_eq!(cache.take_next_batch().len(), PREVIEW_IN_FLIGHT_EXTRA);

        cache.enqueue_request(
            thumbnail_request("preview-b.png", 4),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );

        assert!(cache.take_next_batch().is_empty());
    }

    #[test]
    fn pruning_scope_removes_only_stale_queued_work() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        let scope = ThumbnailScope::PaneDirectory {
            pane_id: 1,
            directory: PathBuf::from("/workspace"),
        };
        let kept_request = thumbnail_request("kept.png", 1);
        let stale_request = thumbnail_request("stale.png", 2);
        let preview_request = thumbnail_request("preview.png", 3);

        cache.enqueue_request_for_scope(
            kept_request.clone(),
            ThumbnailPurpose::List,
            ThumbnailPriority::Visible,
            scope.clone(),
        );
        cache.enqueue_request_for_scope(
            stale_request,
            ThumbnailPurpose::List,
            ThumbnailPriority::Visible,
            scope.clone(),
        );
        cache.enqueue_request(
            preview_request.clone(),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );

        cache.prune_scope_except(&scope, &[kept_request.key()]);

        let batch = cache.take_next_batch();
        let sources = batch
            .into_iter()
            .map(|work| work.request.source)
            .collect::<Vec<_>>();
        assert!(sources.contains(&kept_request.source));
        assert!(sources.contains(&preview_request.source));
        assert!(!sources.contains(&PathBuf::from("stale.png")));
    }

    #[test]
    fn queue_rejects_sources_inside_thumbnail_cache_dir() {
        let mut cache = ThumbnailCache::new(PathBuf::from("/cache/thumbnails"));
        cache.enqueue_request(
            thumbnail_request("/cache/thumbnails/generated.png", 1),
            ThumbnailPurpose::List,
            ThumbnailPriority::Visible,
        );
        cache.enqueue_request(
            thumbnail_request("/cache/thumbnails/nested/generated.png", 2),
            ThumbnailPurpose::List,
            ThumbnailPriority::Visible,
        );
        let sibling_request = thumbnail_request("/cache/thumbnails-extra/photo.png", 3);
        cache.enqueue_request(
            sibling_request.clone(),
            ThumbnailPurpose::List,
            ThumbnailPriority::Visible,
        );

        let batch = cache.take_next_batch();

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].request, sibling_request);
    }

    #[test]
    fn ready_cache_trims_oldest_entries() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        let first_request = thumbnail_request("image-0.png", 0);
        let second_request = thumbnail_request("image-1.png", 1);
        let newest_request = thumbnail_request("image-newest.png", 9999);

        for index in 0..MAX_READY_THUMBNAILS {
            let request = thumbnail_request(&format!("image-{index}.png"), index as u64);
            cache.insert_ready(cached_thumbnail_for_request(&request), LIST_THUMBNAIL_EDGE);
        }
        cache.insert_ready(
            cached_thumbnail_for_request(&newest_request),
            LIST_THUMBNAIL_EDGE,
        );

        assert_eq!(cache.ready.len(), MAX_READY_THUMBNAILS);
        assert!(cache.ready_for_request(&first_request).is_none());
        assert!(cache.ready_for_request(&second_request).is_some());
        assert!(cache.ready_for_request(&newest_request).is_some());
    }

    #[test]
    fn failure_entries_expire_after_backoff_window() {
        let mut cache = ThumbnailCache::new(PathBuf::from("cache"));
        let key = thumbnail_request("broken.png", 1).key();
        cache.mark_failure(key.clone());

        // 退避期内保留；退避期过后必须被清扫，失败表不允许无限累积。
        cache.sweep_expired_failures(Instant::now());
        assert!(cache.failures.contains_key(&key));

        cache.sweep_expired_failures(Instant::now() + FAILURE_BACKOFF);
        assert!(!cache.failures.contains_key(&key));
    }

    fn cached_thumbnail_for_request(request: &ThumbnailRequest) -> CachedThumbnail {
        CachedThumbnail {
            key: request.key(),
            source: request.source.clone(),
            output: PathBuf::from(format!("{}.thumb.png", request.key().as_str())),
            width: LIST_THUMBNAIL_EDGE,
            height: LIST_THUMBNAIL_EDGE,
            cache_hit: true,
        }
    }
}
