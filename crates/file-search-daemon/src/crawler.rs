use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_search::mime::{MimeDatabase, NameType};
use tokio_util::sync::CancellationToken;

use crate::config::{SearchExcludeRules, SearchIndexConfig};
use crate::database::{
    DirectorySignature, EntryStageProgress, FileSignature, IndexedEntryStageState, IndexedFile,
    MAX_CLASSIFICATION_BATCH_BYTES, MAX_CLASSIFICATION_BATCH_ENTRIES,
};
use crate::error::{SearchError, SearchResult};
use crate::extractor::{
    execute_extraction_plan_cancelled, plan_content_extraction, DurableContentStageState,
    ExtractionExecutionMode, ExtractionPlan, ExtractionStatus,
};
use crate::filesystem::{display_name, ensure_not_cancelled, file_time_ms, FilesystemEntry};
use crate::model::SearchFileKind;
use crate::writer::IndexWriter;

#[path = "crawler/reconciliation.rs"]
mod reconciliation;

/// How many files to scan between progress callbacks during a rebuild.
const PROGRESS_REPORT_INTERVAL: u64 = 128;
const CLASSIFICATION_FIXED_BYTES_PER_ENTRY: usize = 96;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub scanned: u64,
    pub checked: u64,
    pub changed: u64,
    pub reindexed: u64,
    pub skipped: u64,
    pub directories_enumerated: u64,
    pub database_mutations: u64,
    pub content_reads: u64,
    pub directory_snapshots_changed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexMaintenanceProgress {
    Checking {
        checked_entries: u64,
        changed_entries: u64,
    },
    Crawling {
        scanned_entries: u64,
        current_scope: PathBuf,
    },
    Applying {
        pending_mutations: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stage1VisibleFields {
    path: PathBuf,
    parent_path: PathBuf,
    display_name: String,
    kind: SearchFileKind,
    modified_ms: Option<i64>,
    accessed_ms: Option<i64>,
    created_ms: Option<i64>,
    signature: FileSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangedFilePipelinePlan {
    stage1_visible_fields: Stage1VisibleFields,
    stage2_metadata_shape: Stage2MetadataShape,
    stage3_content_plan: Stage3ContentPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stage2MetadataShape {
    mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stage3ContentPlan {
    extraction_plan: ExtractionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stage3ContentOutcome {
    content: Option<String>,
    extraction_status: ExtractionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChangedFilePipelineOutcome {
    Observable(IndexedFile),
    Inaccessible { scope: PathBuf, file: IndexedFile },
}

pub struct SearchIndexer {
    writer: Arc<IndexWriter>,
    config: SearchIndexConfig,
    rules: SearchExcludeRules,
    root_devices: Vec<(PathBuf, u64)>,
}

impl SearchIndexer {
    /// Builds an indexer that funnels every write through `writer`, the process's
    /// single database writer. The crawler never opens its own writable
    /// connection, so it cannot race the writer for the lock.
    pub fn new(writer: Arc<IndexWriter>, config: SearchIndexConfig) -> Self {
        let rules = SearchExcludeRules::from_index_config(&config)
            .expect("search index configuration must be validated before indexing");
        let root_devices = config
            .available_roots()
            .filter_map(|root| {
                std::fs::symlink_metadata(root)
                    .ok()
                    .filter(|metadata| metadata.is_dir())
                    .map(|metadata| (root.clone(), metadata.dev()))
            })
            .collect();
        Self {
            writer,
            config,
            rules,
            root_devices,
        }
    }

    pub fn writer(&self) -> &Arc<IndexWriter> {
        &self.writer
    }

    pub async fn rebuild(&self) -> SearchResult<RebuildStats> {
        let cancellation = CancellationToken::new();
        self.rebuild_with_progress_cancelled(&cancellation, |_| {})
            .await
    }

    pub async fn rebuild_paths(&self, changed_paths: Vec<PathBuf>) -> SearchResult<RebuildStats> {
        let cancellation = CancellationToken::new();
        self.rebuild_paths_with_progress_cancelled(changed_paths, &cancellation, |_| {})
            .await
    }

    pub async fn rebuild_with_progress_cancelled(
        &self,
        cancellation: &CancellationToken,
        on_progress: impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<RebuildStats> {
        self.recover_dirty_roots_with_progress_cancelled(
            self.config.available_roots().cloned().collect(),
            cancellation,
            on_progress,
        )
        .await
    }

    pub async fn rebuild_paths_with_progress_cancelled(
        &self,
        changed_paths: Vec<PathBuf>,
        cancellation: &CancellationToken,
        mut on_progress: impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<RebuildStats> {
        self.committed(async {
            ensure_not_cancelled(cancellation)?;
            let mut stats = RebuildStats::default();
            self.reconcile_changed_paths(changed_paths, &mut stats, cancellation, &mut on_progress)
                .await?;
            Ok(stats)
        })
        .await
    }

    pub(crate) async fn repair_scopes_with_progress_cancelled(
        &self,
        scopes: Vec<PathBuf>,
        cancellation: &CancellationToken,
        mut on_progress: impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<RebuildStats> {
        self.committed(async {
            ensure_not_cancelled(cancellation)?;
            let mut stats = RebuildStats::default();
            self.reconcile_changed_paths(scopes, &mut stats, cancellation, &mut on_progress)
                .await?;
            Ok(stats)
        })
        .await
    }

    pub(crate) async fn patrol_unwatched_directories_with_progress_cancelled(
        &self,
        directories: Vec<PathBuf>,
        cancellation: &CancellationToken,
        mut on_progress: impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<RebuildStats> {
        self.committed(async {
            ensure_not_cancelled(cancellation)?;
            let mut stats = RebuildStats::default();
            for directory in directories {
                self.reconcile_changed_paths(
                    vec![directory],
                    &mut stats,
                    cancellation,
                    &mut on_progress,
                )
                .await?;
            }
            Ok(stats)
        })
        .await
    }

    pub(crate) async fn recover_dirty_roots_with_progress_cancelled(
        &self,
        scopes: Vec<PathBuf>,
        cancellation: &CancellationToken,
        mut on_progress: impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<RebuildStats> {
        self.committed(async {
            ensure_not_cancelled(cancellation)?;
            let mut stats = RebuildStats::default();
            self.startup_check_scopes(scopes, &mut stats, cancellation, &mut on_progress)
                .await?;
            Ok(stats)
        })
        .await
    }

    /// 一件索引工作返回时，它的写入一定已经提交。写库线程按批提交，批不能跨工作，
    /// 否则这件工作的提交失败会算到下一件头上。所以成功、失败、取消都要提交。
    async fn committed(
        &self,
        work: impl std::future::Future<Output = SearchResult<RebuildStats>>,
    ) -> SearchResult<RebuildStats> {
        let outcome = work.await;
        let committed = self.writer.commit_pending();
        outcome.and_then(|stats| committed.map(|()| stats))
    }

    fn collapse_paths_by_owning_root(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        self.config
            .available_roots()
            .flat_map(|root| {
                collapse_affected_prefixes(
                    paths
                        .iter()
                        .filter(|path| self.owning_root_for_path(path) == Some(root.as_path()))
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    }

    fn owning_root_for_path(&self, path: &Path) -> Option<&Path> {
        self.config.owning_root(path)
    }

    /// 名字定不下类型时要读文件开头，所以计划本身是异步的。
    async fn plan_changed_file_pipeline(
        &self,
        path: &Path,
        metadata: &std::fs::Metadata,
        observed_signature: FileSignature,
        types: &MimeDatabase,
    ) -> ChangedFilePipelinePlan {
        let stage1_visible_fields =
            self.plan_stage1_visible_fields(path, metadata, observed_signature);
        let size = stage1_visible_fields.signature.size;
        let stage2_metadata_shape = Stage2MetadataShape {
            mime_type: content_type_for(path, size, types).await,
        };
        let stage3_content_plan =
            self.plan_stage3_content(stage2_metadata_shape.mime_type.as_deref(), size, types);

        ChangedFilePipelinePlan {
            stage1_visible_fields,
            stage2_metadata_shape,
            stage3_content_plan,
        }
    }

    fn plan_stage1_visible_fields(
        &self,
        path: &Path,
        metadata: &std::fs::Metadata,
        observed_signature: FileSignature,
    ) -> Stage1VisibleFields {
        Stage1VisibleFields {
            path: path.to_path_buf(),
            parent_path: path.parent().unwrap_or(Path::new("")).to_path_buf(),
            display_name: display_name(path),
            kind: SearchFileKind::File,
            modified_ms: file_time_ms(metadata.modified().ok()),
            accessed_ms: file_time_ms(metadata.accessed().ok()),
            created_ms: file_time_ms(metadata.created().ok()),
            signature: observed_signature,
        }
    }

    fn plan_stage3_content(
        &self,
        content_type: Option<&str>,
        len: u64,
        types: &MimeDatabase,
    ) -> Stage3ContentPlan {
        Stage3ContentPlan {
            extraction_plan: plan_content_extraction(
                content_type,
                len,
                self.config.max_extract_bytes,
                self.config.content_indexing_enabled,
                types,
            ),
        }
    }

    async fn execute_changed_file_pipeline(
        &self,
        path: &Path,
        changed_file_pipeline: ChangedFilePipelinePlan,
        cancellation: &CancellationToken,
    ) -> SearchResult<ChangedFilePipelineOutcome> {
        let stage3_content_outcome = match self
            .execute_stage3_content_plan(
                path,
                &changed_file_pipeline.stage3_content_plan,
                cancellation,
            )
            .await
        {
            Ok(stage3_content_outcome) => stage3_content_outcome,
            Err(SearchError::Inaccessible {
                path: scope,
                source,
            }) => {
                return Ok(ChangedFilePipelineOutcome::Inaccessible {
                    scope,
                    file: materialize_inaccessible_file(
                        changed_file_pipeline.stage1_visible_fields,
                        changed_file_pipeline.stage2_metadata_shape,
                        source.to_string(),
                    ),
                })
            }
            Err(error) => return Err(error),
        };

        Ok(ChangedFilePipelineOutcome::Observable(
            materialize_indexed_file(
                changed_file_pipeline.stage1_visible_fields,
                changed_file_pipeline.stage2_metadata_shape,
                stage3_content_outcome,
            ),
        ))
    }

    async fn execute_stage3_content_plan(
        &self,
        path: &Path,
        stage3_content_plan: &Stage3ContentPlan,
        cancellation: &CancellationToken,
    ) -> SearchResult<Stage3ContentOutcome> {
        let extraction = execute_extraction_plan_cancelled(
            path,
            &stage3_content_plan.extraction_plan,
            cancellation,
        )
        .await?;

        Ok(Stage3ContentOutcome {
            content: extraction.text,
            extraction_status: extraction.status,
        })
    }
}

fn content_stage_progress_for_extraction_status(
    extraction_status: &ExtractionStatus,
) -> EntryStageProgress {
    // ponytail: 当前 quarantine 只靠 durable `Skipped` + signature diff 表示“unchanged 继续跳过、changed 再重试”，
    // ceiling 是还没有 retry budget、backoff 和 bad-entry journal；后续 recovery 子任务再把失败恢复策略独立出来。
    match extraction_status.durable_content_stage_state() {
        DurableContentStageState::Complete => EntryStageProgress::Complete,
        DurableContentStageState::Skipped => EntryStageProgress::Skipped,
    }
}

fn materialize_indexed_file(
    stage1_visible_fields: Stage1VisibleFields,
    stage2_metadata_shape: Stage2MetadataShape,
    stage3_content_outcome: Stage3ContentOutcome,
) -> IndexedFile {
    // ponytail: 现在只是把 Stage1/2/3 的内部边界先显式化；由于 durable schema 仍要求
    // `IndexedFile` 一次性 upsert，ceiling 是 Stage2/3 还不能独立补写，后续 child task
    // 异步化 Stage2/3 时再拆成多步 writer command。
    IndexedFile {
        path: stage1_visible_fields.path,
        parent_path: stage1_visible_fields.parent_path,
        display_name: stage1_visible_fields.display_name,
        kind: stage1_visible_fields.kind,
        size: stage1_visible_fields.signature.size,
        modified_ms: stage1_visible_fields.modified_ms,
        accessed_ms: stage1_visible_fields.accessed_ms,
        created_ms: stage1_visible_fields.created_ms,
        mime_type: stage2_metadata_shape.mime_type,
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: content_stage_progress_for_extraction_status(
                &stage3_content_outcome.extraction_status,
            ),
        },
        content: stage3_content_outcome.content,
        extraction_status: stage3_content_outcome.extraction_status,
        device: stage1_visible_fields.signature.device,
        inode: stage1_visible_fields.signature.inode,
        mtime_ns: stage1_visible_fields.signature.mtime_ns,
        ctime_ns: stage1_visible_fields.signature.ctime_ns,
    }
}

fn materialize_inaccessible_file(
    stage1_visible_fields: Stage1VisibleFields,
    stage2_metadata_shape: Stage2MetadataShape,
    message: String,
) -> IndexedFile {
    IndexedFile {
        path: stage1_visible_fields.path,
        parent_path: stage1_visible_fields.parent_path,
        display_name: stage1_visible_fields.display_name,
        kind: stage1_visible_fields.kind,
        size: stage1_visible_fields.signature.size,
        modified_ms: stage1_visible_fields.modified_ms,
        accessed_ms: stage1_visible_fields.accessed_ms,
        created_ms: stage1_visible_fields.created_ms,
        mime_type: stage2_metadata_shape.mime_type,
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Pending,
        },
        content: None,
        extraction_status: ExtractionStatus::ReadFailed { message },
        device: stage1_visible_fields.signature.device,
        inode: stage1_visible_fields.signature.inode,
        mtime_ns: stage1_visible_fields.signature.mtime_ns,
        ctime_ns: stage1_visible_fields.signature.ctime_ns,
    }
}

/// 只看名字的类型（签名跳过判断要和建索引时同一个依据）。
pub(crate) fn name_type_for(path: &Path, types: &MimeDatabase) -> NameType {
    path.file_name()
        .map_or(NameType::Unknown, |name| types.type_from_name(name))
}

/// 文件类型：名字能定就不读文件；否则读开头交给类型库。读不了时按名字猜，正文提取会再
/// 碰到同样的错误并照常记下。
async fn content_type_for(path: &Path, size: u64, types: &MimeDatabase) -> Option<String> {
    let name_type = name_type_for(path, types);
    if let NameType::Known(mime) = name_type {
        return Some(mime);
    }
    let wanted = u64::try_from(types.magic_extent())
        .unwrap_or(u64::MAX)
        .min(size);
    match read_file_head(path, wanted).await {
        Ok(head) => types.type_from_content(&name_type, &head),
        Err(_) => name_type.without_content(),
    }
}

/// 打开、读、关放在同一次阻塞调用里：名字认不出类型的文件可能有成千上万个，每个都分几次
/// 进出阻塞线程池，开销比读那几 KB 还大。
async fn read_file_head(path: &Path, wanted: u64) -> std::io::Result<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path)?;
        let mut head = Vec::with_capacity(usize::try_from(wanted).unwrap_or(0));
        let read = (&file).take(wanted).read_to_end(&mut head);
        // 只为判断类型读了一小段，不该让这些页留在服务的工作集里。
        let _ = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        read.map(|_| head)
    })
    .await
    .unwrap_or_else(|join_error| Err(std::io::Error::other(join_error)))
}

fn observed_file_signature(metadata: &std::fs::Metadata) -> FileSignature {
    FileSignature {
        device: Some(metadata.dev()),
        inode: Some(metadata.ino()),
        mtime_ns: Some(metadata_mtime_ns(metadata)),
        ctime_ns: Some(metadata_ctime_ns(metadata)),
        size: metadata.len(),
    }
}

fn extraction_plan_reads_content(plan: &Stage3ContentPlan) -> bool {
    !matches!(
        plan.extraction_plan.execution_mode,
        ExtractionExecutionMode::SkipNow { .. }
    )
}

/// Combines whole-second and nanosecond mtime into a single nanosecond value used
/// only for change detection. Saturating math keeps far-future timestamps from
/// panicking; the exact value never matters, only whether it changed.
fn metadata_mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .mtime()
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(metadata.mtime_nsec()))
        .unwrap_or(metadata.mtime())
}

fn metadata_ctime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .ctime()
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(metadata.ctime_nsec()))
        .unwrap_or(metadata.ctime())
}

fn collapse_affected_prefixes(mut affected_prefixes: Vec<PathBuf>) -> Vec<PathBuf> {
    affected_prefixes.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });

    let mut collapsed_prefixes: Vec<PathBuf> = Vec::new();
    'candidate: for affected_prefix in affected_prefixes {
        if collapsed_prefixes
            .iter()
            .any(|existing_prefix| affected_prefix.starts_with(existing_prefix))
        {
            continue 'candidate;
        }

        collapsed_prefixes.push(affected_prefix);
    }

    collapsed_prefixes.sort();
    collapsed_prefixes
}

struct PendingClassificationBatch {
    entries: Vec<FilesystemEntry>,
    estimated_bytes: usize,
}

impl PendingClassificationBatch {
    fn new() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_CLASSIFICATION_BATCH_ENTRIES),
            estimated_bytes: 0,
        }
    }

    async fn push(
        &mut self,
        indexer: &SearchIndexer,
        entry: FilesystemEntry,
        current_scope: &Path,
        stats: &mut RebuildStats,
        cancellation: &CancellationToken,
        on_progress: &mut impl FnMut(IndexMaintenanceProgress),
    ) -> SearchResult<()> {
        let estimated_bytes = entry
            .path()
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .saturating_add(CLASSIFICATION_FIXED_BYTES_PER_ENTRY);
        let batch_is_full = self.entries.len() == MAX_CLASSIFICATION_BATCH_ENTRIES
            || self.estimated_bytes.saturating_add(estimated_bytes)
                > MAX_CLASSIFICATION_BATCH_BYTES;
        if batch_is_full {
            indexer
                .index_observed_batch(
                    self.take_entries(),
                    current_scope,
                    stats,
                    cancellation,
                    on_progress,
                )
                .await?;
        }
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        self.entries.push(entry);
        Ok(())
    }

    fn take_entries(&mut self) -> Vec<FilesystemEntry> {
        self.estimated_bytes = 0;
        std::mem::replace(
            &mut self.entries,
            Vec::with_capacity(MAX_CLASSIFICATION_BATCH_ENTRIES),
        )
    }

    fn into_entries(self) -> Vec<FilesystemEntry> {
        self.entries
    }
}

fn directory_signature(metadata: &std::fs::Metadata) -> DirectorySignature {
    DirectorySignature {
        device: metadata.dev(),
        inode: metadata.ino(),
        mtime_ns: metadata_mtime_ns(metadata),
        ctime_ns: metadata_ctime_ns(metadata),
    }
}

fn report_checking_if_needed(
    stats: &RebuildStats,
    on_progress: &mut impl FnMut(IndexMaintenanceProgress),
) {
    if stats.checked.is_multiple_of(PROGRESS_REPORT_INTERVAL) {
        on_progress(IndexMaintenanceProgress::Checking {
            checked_entries: stats.checked,
            changed_entries: stats.changed,
        });
    }
}

fn report_crawling_if_needed(
    stats: &RebuildStats,
    current_scope: &Path,
    on_progress: &mut impl FnMut(IndexMaintenanceProgress),
) {
    if stats.scanned.is_multiple_of(PROGRESS_REPORT_INTERVAL) {
        report_crawling(stats, current_scope, on_progress);
    }
}

fn report_crawling(
    stats: &RebuildStats,
    current_scope: &Path,
    on_progress: &mut impl FnMut(IndexMaintenanceProgress),
) {
    on_progress(IndexMaintenanceProgress::Crawling {
        scanned_entries: stats.scanned,
        current_scope: current_scope.to_path_buf(),
    });
}

#[cfg(test)]
#[path = "crawler/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "crawler/content_type_tests.rs"]
mod content_type_tests;
