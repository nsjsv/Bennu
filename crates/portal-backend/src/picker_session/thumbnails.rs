//! 会话级缩略图调度：可见区间裁剪、并发限流、失败 backoff、LRU 就绪表。
//! 参考主软件 `app-ui/src/thumbnail_cache.rs` 的队列思路做会话架构精简版：
//! 请求在会话内积攒（`queued`），main 层在每条会话消息处理后 drain 并
//! `Task::perform` 发起加载，结果经 `SessionMessage::ThumbnailReady`
//! 回信——滚动类消息绕过 `SessionEffect` 路径，drain 机制对滚动路径与
//! update 路径统一生效。本模块不 import iced 类型，可脱离窗口单测。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_core::entry::FileKind;
use file_core::DirectoryEntry;
// 本模块名与 crate 同名，mod.rs 侧的 `use thumbnails::…` 解析到本模块；
// 这两类需要出现在 SessionMessage 定义里，经本模块再导出（与 scrollbar
// 模块再导出 ScrollbarViewport 同一模式）。
use thumbnails::ThumbnailKey;
pub(crate) use thumbnails::{CachedThumbnail, ThumbnailRequest};

use super::scrollbar::SessionScrollRegion;
use super::view_mode::LIST_THUMBNAIL_EDGE;
use super::{PickerRow, PickerSession, PickerViewMode};

#[cfg(test)]
use super::expansion::LIST_ROW_STRIDE;
#[cfg(test)]
use super::scrollbar::ScrollbarViewport;

/// 行内缩略图生成档位已由 view_mode 子模块按视图模式给出（列表 128 /
/// 大图 thumbnail_edge(96)）；本常量仅剩测试夹具使用。
#[cfg(test)]
const THUMBNAIL_MAX_EDGE: u32 = 128;
/// 同时在途的生成请求数：portal 是轻量弹窗，固定 4 而不跟随核数。
const MAX_IN_FLIGHT: usize = 4;
/// 就绪表 LRU 上限：CachedThumbnail 只持路径不驻留像素，2048 项成本
/// 可忽略；跨目录保留（返回原目录即时显示）。
const READY_LIMIT: usize = 2048;
/// 失败 backoff：损坏图回落图标后 60s 内不重试，防止重试风暴。
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);
/// 可见区间前后各多取的行数：滚动惯性期间提前请求即将进入的行。
/// 数值由 view_mode 子模块的可见窗口换算消费，保留常量单源在本模块。
pub(super) const VISIBLE_MARGIN_ROWS: isize = 8;

/// portal 缩略图磁盘缓存目录：与主软件默认配置同目录，主软件已生成
/// 的缩略图直接命中，不重复生成。
pub(crate) fn default_thumbnail_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|dir| dir.join("thumbnails"))
        .unwrap_or_default()
}

/// 回信失败标记。`ThumbnailError` 不实现 Clone，而 SessionMessage 必须
/// 整体 Clone（iced widget 的 Element 转换要求 Message: Clone）；错误
/// 细节由 main 层在边界用 tracing 记录一次，会话层只需要"失败"事实
/// 来记 backoff。
#[derive(Debug, Clone)]
pub(crate) struct ThumbnailLoadFailed;

/// 缩略图调度状态：就绪表（LRU）、在途集合、排队队列、失败 backoff。
#[derive(Debug)]
pub(crate) struct SessionThumbnailState {
    cache_dir: PathBuf,
    /// 按源路径存最新就绪图，附带其请求档位：列表 128 档就绪后切到大图
    /// 必须能判出“档位不足”并补请求 192 档，否则大图永远显示小图。
    /// 不用像素尺寸判定：小原图的缩略图永远达不到档位，会反复重请求。
    ready: HashMap<PathBuf, (u32, CachedThumbnail)>,
    /// ready 的插入顺序（LRU 淘汰依据；同源重复就绪先移除旧序防重复）。
    ready_order: VecDeque<PathBuf>,
    in_flight: HashSet<ThumbnailKey>,
    queued: VecDeque<ThumbnailRequest>,
    /// queued 的 key 镜像：视口每次滚动都会重算可见区间，靠它去重。
    queued_keys: HashSet<ThumbnailKey>,
    failed_until: HashMap<ThumbnailKey, Instant>,
}

impl SessionThumbnailState {
    pub(crate) fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            ready: HashMap::new(),
            ready_order: VecDeque::new(),
            in_flight: HashSet::new(),
            queued: VecDeque::new(),
            queued_keys: HashSet::new(),
            failed_until: HashMap::new(),
        }
    }

    /// 换目录时清空在途与排队：迟到回信按 key 找不到在途记录即丢弃，
    /// 天然防止旧目录结果回填到新列表；ready/failed 保留（返回原目录
    /// 即时显示、失败不必立刻重试）。
    pub(crate) fn clear_pending(&mut self) {
        self.in_flight.clear();
        self.queued.clear();
        self.queued_keys.clear();
    }

    /// 对可见条目索引区间内的图片行入队缺失请求。条目集由调用方给
    /// 出（列表 = 扁平行，多栏 = 栏内过滤条目），区间换算随视图模式
    /// 几何不同，由 view_mode/columns 子模块计算后传入；edge 是请求
    /// 档位（大图请求大边长）。
    fn enqueue_visible_entries(
        &mut self,
        rows: &[PickerRow],
        first: usize,
        last: usize,
        edge: u32,
        now: Instant,
    ) {
        self.enqueue_visible_range(
            rows.len(),
            |index| rows.get(index).map(|row| &row.entry),
            first,
            last,
            edge,
            now,
        );
    }

    /// 多栏变体：直接对栏内过滤条目入队（避免逐帧克隆整栏）。
    pub(super) fn enqueue_visible_lane_entries(
        &mut self,
        entries: &[DirectoryEntry],
        first: usize,
        last: usize,
        edge: u32,
        now: Instant,
    ) {
        self.enqueue_visible_range(
            entries.len(),
            |index| entries.get(index),
            first,
            last,
            edge,
            now,
        );
    }

    /// 可见区间入队的共享实现：条目经闭包按索引取（两种行模型共用
    /// 同一套缓存目录源拒绝/档位去重/backoff 规则）。
    fn enqueue_visible_range<'a>(
        &'a mut self,
        count: usize,
        entry_at: impl Fn(usize) -> Option<&'a DirectoryEntry>,
        first: usize,
        last: usize,
        edge: u32,
        now: Instant,
    ) {
        // 顺手清理过期 backoff：到期即可重试，不让表无界增长。
        self.failed_until.retain(|_, until| *until > now);
        let last = last.min(count.saturating_sub(1));
        for index in first..=last {
            let Some(entry) = entry_at(index) else {
                continue;
            };
            // 仅图片：视频依赖外部 ffmpegthumbnailer，生成慢不做（prd 约定）。
            if entry.kind != FileKind::File || !file_core::is_supported_image_path(&entry.path) {
                continue;
            }
            // spec 合同：缓存目录内的源文件拒绝入队，防止"缓存的缓存"。
            if thumbnails::path_is_in_thumbnail_cache(&self.cache_dir, &entry.path) {
                continue;
            }
            let request = ThumbnailRequest::new(
                &entry.path,
                thumbnails::ThumbnailSourceMetadata::from(&entry.metadata),
                edge,
            );
            let key = request.key();
            let ready_covers_edge = self
                .ready
                .get(&entry.path)
                .is_some_and(|(ready_edge, _)| *ready_edge >= edge);
            if ready_covers_edge || self.in_flight.contains(&key) || self.queued_keys.contains(&key)
            {
                continue;
            }
            if self.failed_until.contains_key(&key) {
                continue;
            }
            self.queued_keys.insert(key);
            self.queued.push_back(request);
        }
    }

    /// 出队补满并发额度：main 层在每条会话消息处理后调用；回信
    /// （ThumbnailReady）本身也走会话消息路径，处理完再 drain 即自然
    /// 补位，无需定时器。
    fn drain_due_requests(&mut self) -> Vec<ThumbnailRequest> {
        let mut drained = Vec::new();
        while self.in_flight.len() < MAX_IN_FLIGHT {
            let Some(request) = self.queued.pop_front() else {
                break;
            };
            self.queued_keys.remove(&request.key());
            self.in_flight.insert(request.key());
            drained.push(request);
        }
        drained
    }

    /// 环境异常（解析不出缓存目录）下放弃排队请求：只清排队，不动
    /// 在途——与 drain 不同，这些请求从未发起，不能占在途额度。
    fn clear_pending_requests(&mut self) {
        for request in self.queued.drain(..) {
            self.queued_keys.remove(&request.key());
        }
    }

    /// 接受回信。key 已不在 in_flight（换目录清空后的迟到回信）则整体
    /// 丢弃，防串目录回填；返回是否产生了新的就绪缩略图。
    fn accept_thumbnail_outcome(
        &mut self,
        request: ThumbnailRequest,
        outcome: Result<CachedThumbnail, ThumbnailLoadFailed>,
        now: Instant,
    ) -> bool {
        let key = request.key();
        if !self.in_flight.remove(&key) {
            return false;
        }
        match outcome {
            Ok(cached) => {
                let edge = request.max_edge;
                let source = request.source;
                // 不降档：大档就绪后迟到的小档回信（切换视图前已在途）不覆盖。
                if self
                    .ready
                    .get(&source)
                    .is_some_and(|(ready_edge, _)| *ready_edge > edge)
                {
                    return false;
                }
                if let Some(position) = self.ready_order.iter().position(|path| path == &source) {
                    self.ready_order.remove(position);
                }
                self.ready_order.push_back(source.clone());
                self.ready.insert(source, (edge, cached));
                while self.ready.len() > READY_LIMIT {
                    let Some(evicted) = self.ready_order.pop_front() else {
                        break;
                    };
                    self.ready.remove(&evicted);
                }
                true
            }
            Err(_) => {
                self.failed_until.insert(key, now + FAILURE_BACKOFF);
                false
            }
        }
    }

    fn ready_for(&self, source: &Path) -> Option<&CachedThumbnail> {
        self.ready.get(source).map(|(_, cached)| cached)
    }

    /// 预览档位缩略图入队（预览宿主专用，主软件 thumbnail_cache
    /// `enqueue_request(Purpose::Preview)` 的对齐物）：绕过行内入队的
    /// 可见区间与 128 档位约束；缓存目录源拒绝、key 去重、在途合并、
    /// 失败 backoff 与行内路径同规则。返回是否处于等待（在途/已排队/
    /// 新入队）——宿主以此决定 `pending_preview_thumbnail_display` 标记；
    /// backoff 或缓存目录源返回 false（不等缩略图，原图解码兜底）。
    fn enqueue_preview_request(&mut self, request: ThumbnailRequest) -> bool {
        if thumbnails::path_is_in_thumbnail_cache(&self.cache_dir, &request.source) {
            return false;
        }
        let key = request.key();
        if self.in_flight.contains(&key) || self.queued_keys.contains(&key) {
            return true;
        }
        if self.failed_until.contains_key(&key) {
            return false;
        }
        self.queued_keys.insert(key);
        self.queued.push_back(request);
        true
    }
}

impl PickerSession {
    /// 重算可见区间并入队缺失请求。无视口缓存（首帧探针未回）或
    /// 视口退化时不发请求，等滚动条布局回信触发。可见区间与请求档
    /// 位按视图模式计算：列表/大图用 List 视口；多栏逐栏用各自栏
    /// 视口（行内小缩略图同列表档位）。
    pub(crate) fn schedule_visible_thumbnails(&mut self) {
        if self.view_mode() == PickerViewMode::Columns {
            for lane in 0..self.columns_chain().len() {
                let Some(viewport) =
                    self.scrollbar_viewport_for(&SessionScrollRegion::ColumnsLane(lane))
                else {
                    continue;
                };
                let Some((first, last)) = self.columns_visible_window(lane, &viewport) else {
                    continue;
                };
                let entries: Vec<DirectoryEntry> =
                    self.column_entries_iter(lane).cloned().collect();
                self.thumbnails.enqueue_visible_lane_entries(
                    &entries,
                    first,
                    last,
                    LIST_THUMBNAIL_EDGE,
                    Instant::now(),
                );
            }
            return;
        }
        let Some(viewport) = self.scrollbar_viewport_for(&SessionScrollRegion::List) else {
            return;
        };
        let Some((first, last, edge)) = self.visible_thumbnail_window(&viewport) else {
            return;
        };
        let rows: &[PickerRow] = &self.rows;
        self.thumbnails
            .enqueue_visible_entries(rows, first, last, edge, Instant::now());
    }

    /// main 层入口：拿走已到并发额度的请求，逐个 Task::perform 发起。
    pub(crate) fn drain_pending_thumbnail_requests(&mut self) -> Vec<ThumbnailRequest> {
        self.thumbnails.drain_due_requests()
    }

    /// main 层入口：解析不出缓存目录等环境异常时，放弃排队请求且不占
    /// 在途额度（这些请求从未发起）。
    pub(crate) fn clear_pending_thumbnail_requests(&mut self) {
        self.thumbnails.clear_pending_requests();
    }

    /// ThumbnailReady 回信处理：迟到回信由状态层按 in_flight 丢弃。
    pub(crate) fn accept_thumbnail_ready(
        &mut self,
        request: ThumbnailRequest,
        outcome: Result<CachedThumbnail, ThumbnailLoadFailed>,
    ) {
        self.thumbnails
            .accept_thumbnail_outcome(request, outcome, Instant::now());
    }

    /// 视图查询：行内缩略图就绪则返回缓存 PNG 信息，否则回落类型图标。
    pub(crate) fn thumbnail_ready(&self, source: &Path) -> Option<&CachedThumbnail> {
        self.thumbnails.ready_for(source)
    }

    /// 预览档位入队入口（预览宿主调用；main 层随后的 drain 照常发起）。
    pub(crate) fn enqueue_preview_thumbnail_request(&mut self, request: ThumbnailRequest) -> bool {
        self.thumbnails.enqueue_preview_request(request)
    }

    /// 按路径查目录条目元数据（预览宿主构造缩略图请求用；行序扫描与
    /// 主软件 entry_for_path 同为线性查找）。
    pub(crate) fn directory_entry_for_path(&self, path: &Path) -> Option<&DirectoryEntry> {
        self.rows
            .iter()
            .find(|row| row.entry.path == path)
            .map(|row| &row.entry)
    }

    /// 缩略图磁盘缓存目录（main 层发起加载时使用；与主软件共享）。
    pub(crate) fn thumbnail_cache_dir(&self) -> &Path {
        &self.thumbnails.cache_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus_file_chooser::PickerResolution;
    use crate::picker_request::{PickerKind, PickerRequestSpec};
    use crate::picker_session::scan::{DirectoryScanOutcome, DirectoryScanResult};
    use crate::picker_session::SessionMessage;
    use file_core::entry::{DirectoryEntry, EntryMetadata};
    use thumbnails::ThumbnailSourceMetadata;

    fn image_session(files: &[&str]) -> PickerSession {
        let (reply, _receiver) = tokio::sync::oneshot::channel::<PickerResolution>();
        let base = tempfile::tempdir().unwrap();
        let mut session = PickerSession::new(
            &PickerRequestSpec {
                kind: PickerKind::OpenFile {
                    multiple: false,
                    directory: false,
                },
                accept_label: None,
                title: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
                choices: Vec::new(),
            },
            "/req/thumb".to_string(),
            base.keep(),
            crate::picker_session::PickerViewMode::List,
            reply,
        );
        let entries = files
            .iter()
            .map(|&name| {
                DirectoryEntry::new(
                    session.directory.join(name),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )
            })
            .collect();
        session.apply_scan(DirectoryScanResult {
            directory: session.directory.clone(),
            outcome: Ok(DirectoryScanOutcome { entries }),
        });
        session
    }

    fn viewport_at(offset_y: f32, height: f32) -> ScrollbarViewport {
        ScrollbarViewport {
            offset_x: 0.0,
            offset_y,
            viewport_width: 600.0,
            viewport_height: height,
            content_width: 600.0,
            content_height: 100.0 * LIST_ROW_STRIDE,
        }
    }

    fn install_viewport(session: &mut PickerSession, viewport: ScrollbarViewport) {
        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport,
        });
    }

    fn cached_png(request: &ThumbnailRequest) -> CachedThumbnail {
        CachedThumbnail {
            key: request.key(),
            source: request.source.clone(),
            output: PathBuf::from("/tmp/fake-thumbnail.png"),
            width: THUMBNAIL_MAX_EDGE,
            height: THUMBNAIL_MAX_EDGE,
            cache_hit: false,
        }
    }

    fn failure() -> ThumbnailLoadFailed {
        ThumbnailLoadFailed
    }

    /// 排空队列并全部标记成功，返回按出队顺序的请求列表。
    fn drain_and_complete(session: &mut PickerSession) -> Vec<ThumbnailRequest> {
        let mut all = Vec::new();
        loop {
            let drained = session.drain_pending_thumbnail_requests();
            if drained.is_empty() {
                break;
            }
            for request in drained {
                session.accept_thumbnail_ready(request.clone(), Ok(cached_png(&request)));
                all.push(request);
            }
        }
        all
    }

    fn queued_sources(session: &PickerSession) -> Vec<PathBuf> {
        session
            .thumbnails
            .queued
            .iter()
            .map(|request| request.source.clone())
            .collect()
    }

    #[test]
    fn drain_limits_concurrency_to_four_and_pumps_on_completion() {
        let files: Vec<String> = (0..10).map(|index| format!("img{index}.png")).collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let mut session = image_session(&names);
        install_viewport(&mut session, viewport_at(0.0, 30.0 * 10.0));

        session.schedule_visible_thumbnails();
        // 并发上限 4：一次 drain 最多给出 4 个请求。
        let drained = session.drain_pending_thumbnail_requests();
        assert_eq!(drained.len(), 4);
        assert_eq!(session.thumbnails.in_flight.len(), 4);
        assert!(session.drain_pending_thumbnail_requests().is_empty());

        // 就绪回信释放额度后，下一次 drain 自然补位（无需定时器）。
        let completed = drained[0].clone();
        session.accept_thumbnail_ready(completed.clone(), Ok(cached_png(&completed)));
        assert_eq!(session.thumbnails.in_flight.len(), 3);
        assert_eq!(session.drain_pending_thumbnail_requests().len(), 1);
        assert_eq!(session.thumbnails.in_flight.len(), 4);
    }

    #[test]
    fn failure_backoff_blocks_rescheduling_until_expiry() {
        let mut session = image_session(&["broken.png"]);
        install_viewport(&mut session, viewport_at(0.0, 300.0));

        session.schedule_visible_thumbnails();
        let request = session.drain_pending_thumbnail_requests().pop().unwrap();
        session.accept_thumbnail_ready(request.clone(), Err(failure()));

        // backoff 期内不再对该图发请求。
        session.schedule_visible_thumbnails();
        assert!(session.drain_pending_thumbnail_requests().is_empty());

        // 到期（人为回拨时间戳）后允许重试。
        let expired = Instant::now() - FAILURE_BACKOFF;
        session
            .thumbnails
            .failed_until
            .insert(request.key(), expired);
        session.schedule_visible_thumbnails();
        assert_eq!(session.drain_pending_thumbnail_requests().len(), 1);
    }

    #[test]
    fn late_reply_after_directory_change_is_discarded() {
        let mut session = image_session(&["old.png"]);
        install_viewport(&mut session, viewport_at(0.0, 300.0));
        session.schedule_visible_thumbnails();
        let request = session.drain_pending_thumbnail_requests().pop().unwrap();

        // 换目录清空在途/排队：旧目录的迟到回信必须整体丢弃。
        session.enter_directory(PathBuf::from("/other/directory"));
        session.accept_thumbnail_ready(request.clone(), Ok(cached_png(&request)));
        assert!(session.thumbnail_ready(&request.source).is_none());
    }

    #[test]
    fn ready_lru_is_capped_and_keeps_newest_entries() {
        let mut session = image_session(&["seed.png"]);
        let base = session.directory().to_path_buf();
        let now = Instant::now();
        for index in 0..=READY_LIMIT {
            let source = base.join(format!("img{index}.png"));
            let request = ThumbnailRequest::new(
                &source,
                ThumbnailSourceMetadata {
                    len: 0,
                    modified: None,
                },
                THUMBNAIL_MAX_EDGE,
            );
            // 直接走状态层：先补一个在途占位再回信，模拟完整请求周期。
            session.thumbnails.in_flight.insert(request.key());
            session.thumbnails.accept_thumbnail_outcome(
                request.clone(),
                Ok(cached_png(&request)),
                now,
            );
        }

        assert_eq!(session.thumbnails.ready.len(), READY_LIMIT);
        assert!(session.thumbnail_ready(&base.join("img0.png")).is_none());
        assert!(session
            .thumbnail_ready(&base.join(format!("img{READY_LIMIT}.png")))
            .is_some());
        assert_eq!(session.thumbnails.ready_order.len(), READY_LIMIT);
    }

    #[test]
    fn visible_interval_requests_only_rows_near_viewport() {
        let files: Vec<String> = (0..100).map(|index| format!("img{index:03}.png")).collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let mut session = image_session(&names);
        let base = session.directory().to_path_buf();

        // 无视口缓存（首帧探针未回）：不发任何请求。
        session.schedule_visible_thumbnails();
        assert!(queued_sources(&session).is_empty());

        // 视口盖住第 40-49 行（offset 1200、高 300）：±8 行余量 → 32..=57。
        install_viewport(&mut session, viewport_at(30.0 * 40.0, 30.0 * 10.0));
        session.schedule_visible_thumbnails();
        let sources = queued_sources(&session);
        assert_eq!(sources.len(), 26);
        assert_eq!(sources.first(), Some(&base.join("img032.png")));
        assert_eq!(sources.last(), Some(&base.join("img057.png")));
        // 列表模式请求 128 档（大图 192 档的断言在 view_mode 测试）。
        assert!(session
            .thumbnails
            .queued
            .iter()
            .all(|request| request.max_edge == THUMBNAIL_MAX_EDGE));
    }

    #[test]
    fn non_image_rows_and_cache_dir_sources_are_not_enqueued() {
        let mut session = image_session(&[]);
        session.apply_scan(DirectoryScanResult {
            directory: session.directory().to_path_buf(),
            outcome: Ok(DirectoryScanOutcome {
                entries: vec![
                    DirectoryEntry::new(
                        session.directory.join("photo.jpg"),
                        FileKind::File,
                        EntryMetadata::default(),
                        false,
                        false,
                        false,
                    ),
                    DirectoryEntry::new(
                        session.directory.join("notes.txt"),
                        FileKind::File,
                        EntryMetadata::default(),
                        false,
                        false,
                        false,
                    ),
                    DirectoryEntry::new(
                        session.directory.join("subdir"),
                        FileKind::Directory,
                        EntryMetadata::default(),
                        false,
                        false,
                        false,
                    ),
                ],
            }),
        });
        // 把缓存目录指到当前目录：其中的"图片"必须被拒（spec 合同），
        // 非图片与目录行同样不入队。
        session.thumbnails.cache_dir = session.directory().to_path_buf();
        install_viewport(&mut session, viewport_at(0.0, 300.0));

        session.schedule_visible_thumbnails();
        assert!(queued_sources(&session).is_empty());
    }

    #[test]
    fn scrolling_into_new_range_requests_newly_visible_rows() {
        let files: Vec<String> = (0..100).map(|index| format!("img{index:03}.png")).collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let mut session = image_session(&names);

        // 首屏（第 0-1 行可见 ±8）全部完成后就绪。
        install_viewport(&mut session, viewport_at(0.0, 30.0 * 2.0));
        session.schedule_visible_thumbnails();
        let first_batch = drain_and_complete(&mut session);
        assert!(!first_batch.is_empty());
        assert!(first_batch.iter().all(|request| request
            .source
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("img0")));

        // 滚动到第 50 行附近：视口回传走滚动消息路径写缓存并触发调度，
        // 新进入区间的行（42..=59）必须发起请求。
        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: viewport_at(30.0 * 50.0, 30.0 * 2.0),
        });
        let second_batch = drain_and_complete(&mut session);
        assert!(!second_batch.is_empty());
        assert!(second_batch.iter().all(|request| {
            let name = request.source.file_name().unwrap().to_string_lossy();
            let index: u32 = name
                .trim_start_matches("img")
                .trim_end_matches(".png")
                .parse()
                .unwrap();
            (42..=59).contains(&index)
        }));
    }

    #[test]
    fn icon_view_upgrades_small_ready_thumbnail_and_late_small_reply_never_downgrades() {
        let mut session = image_session(&["photo.png"]);
        let source = session.directory().join("photo.png");
        install_viewport(&mut session, viewport_at(0.0, 300.0));

        // 列表档就绪后切大图：就绪档位不足，必须补请求大档。
        session.schedule_visible_thumbnails();
        let small = session.drain_pending_thumbnail_requests().pop().unwrap();
        assert_eq!(small.max_edge, THUMBNAIL_MAX_EDGE);
        session.accept_thumbnail_ready(small.clone(), Ok(cached_png(&small)));
        session.update(SessionMessage::ViewModeSelected {
            mode: crate::picker_session::PickerViewMode::Icons,
        });
        session.schedule_visible_thumbnails();
        let large = session.drain_pending_thumbnail_requests().pop().unwrap();
        assert_eq!(large.max_edge, 192);
        session.accept_thumbnail_ready(large.clone(), Ok(cached_png(&large)));
        assert_eq!(session.thumbnails.ready[&source].0, 192);

        // 切回列表：大档覆盖小档需求，不再请求。
        session.update(SessionMessage::ViewModeSelected {
            mode: crate::picker_session::PickerViewMode::List,
        });
        session.schedule_visible_thumbnails();
        assert!(session.drain_pending_thumbnail_requests().is_empty());

        // 迟到的小档回信不降档。
        session.thumbnails.in_flight.insert(small.key());
        session.accept_thumbnail_ready(small.clone(), Ok(cached_png(&small)));
        assert_eq!(session.thumbnails.ready[&source].0, 192);
    }
}
