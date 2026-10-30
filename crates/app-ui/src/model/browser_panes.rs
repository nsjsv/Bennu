use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    DirectoryDiscovery, DirectoryEntry, DirectoryMetadataRequirement, ResolvedEntryChange,
    SortDirection, SortField, TrashEntry,
};
use tokio_util::sync::CancellationToken;

use crate::operation_history::{
    completed_migrations_have_source_in_directory_tree, completed_migrations_leave_directory_tree,
    completed_migrations_touch_directory_tree, path_after_completed_migrations,
    CompletedPathMigration,
};
use crate::thumbnail_cache::ColumnViewport;

use super::path_migration::{
    cancel_and_clear_expanded_directories, detach_directory_discovery, migrate_directory_entries,
    migrate_directory_entry, migrate_expanded_directories, migrate_optional_path,
    migrate_path_list, migrate_path_map_keys, migrate_path_set,
    retain_optional_path_outside_subtree,
};
use super::{displayed_address_directory, TabDropDestination};

pub(crate) type DirectoryEntrySnapshot = Arc<Vec<DirectoryEntry>>;

pub(crate) fn empty_directory_entry_snapshot() -> DirectoryEntrySnapshot {
    Arc::new(Vec::new())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BrowserPaneId(pub(crate) u64);

impl BrowserPaneId {
    pub(crate) const PRIMARY: Self = Self(0);

    pub(crate) fn key(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IconGridExpansionSessionId(u64);

impl IconGridExpansionSessionId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ListExpansionFollowSessionId(u64);

impl ListExpansionFollowSessionId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryLoadRequest {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DirectoryMetadataLoadContext {
    Root {
        pane_id: BrowserPaneId,
        path: PathBuf,
        collection_generation: u64,
    },
    Expanded {
        pane_id: BrowserPaneId,
        path: PathBuf,
        load_generation: u64,
    },
}

impl DirectoryMetadataLoadContext {
    pub(crate) fn pane_id(&self) -> BrowserPaneId {
        match self {
            Self::Root { pane_id, .. } | Self::Expanded { pane_id, .. } => *pane_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryMetadataLoadRequest {
    pub(crate) context: DirectoryMetadataLoadContext,
    pub(crate) request_generation: u64,
    pub(crate) requirement: DirectoryMetadataRequirement,
    pub(crate) targets: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryMetadataLoadFailure {
    Cancelled,
    ReadFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryExpansionLoadContext {
    BrowserTree {
        pane_id: BrowserPaneId,
    },
    ListFollow {
        pane_id: BrowserPaneId,
        tab_id: usize,
        current_dir: PathBuf,
        session_id: ListExpansionFollowSessionId,
    },
    IconGrid {
        pane_id: BrowserPaneId,
        current_dir: PathBuf,
        session_id: IconGridExpansionSessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedDirectoryLoadRequest {
    pub(crate) context: DirectoryExpansionLoadContext,
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryLoadFailure {
    DirectoryUnavailable { message: String },
    ReadFailed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryCollectionPhase {
    Discovering,
    Ready,
}

impl DirectoryCollectionPhase {
    pub(crate) fn is_discovering(self) -> bool {
        matches!(self, Self::Discovering)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryOrderPhase {
    Ready {
        field: SortField,
        direction: SortDirection,
    },
    WaitingForMetadata {
        request_generation: u64,
        field: SortField,
        direction: SortDirection,
    },
}

impl DirectoryOrderPhase {
    pub(crate) fn is_ready(self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitRegion {
    Left,
    Right,
    Top,
    Bottom,
}

impl SplitRegion {
    pub(crate) fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Top | Self::Bottom => SplitAxis::Vertical,
        }
    }

    pub(crate) fn places_dragged_first(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserPaneLayout {
    Single {
        active: BrowserPaneId,
    },
    Split {
        axis: SplitAxis,
        first: BrowserPaneId,
        second: BrowserPaneId,
        active: BrowserPaneId,
        first_portion: u16,
    },
}

impl BrowserPaneLayout {
    pub(crate) fn active(self) -> BrowserPaneId {
        match self {
            Self::Single { active } | Self::Split { active, .. } => active,
        }
    }

    pub(crate) fn visible_pane_ids(self) -> Vec<BrowserPaneId> {
        match self {
            Self::Single { active } => vec![active],
            Self::Split { first, second, .. } => vec![first, second],
        }
    }

    pub(crate) fn with_active(self, next_active: BrowserPaneId) -> Self {
        match self {
            Self::Single { .. } => Self::Single {
                active: next_active,
            },
            Self::Split {
                axis,
                first,
                second,
                first_portion,
                ..
            } => Self::Split {
                axis,
                first,
                second,
                active: next_active,
                first_portion,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserViewMode {
    Columns,
    List,
    Icons,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ColumnBrowserViewport {
    pub(crate) offset_x: f32,
    pub(crate) width: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct IconGridViewport {
    pub(crate) offset_y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectoryLoadingPlaceholder {
    pub(crate) before_height: f32,
    pub(crate) entries: Vec<DirectoryLoadingPlaceholderEntry>,
    pub(crate) after_height: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectoryLoadingPlaceholderEntry {
    pub(crate) entry: DirectoryEntry,
    pub(crate) depth: usize,
    pub(crate) animation_progress: f32,
    pub(crate) row_index: usize,
    pub(crate) trailing_status_height: f32,
}

/// 窗格：一组标签，加上只属于窗格自己的加载与滚动状态。浏览状态（目录、
/// 条目、选择、历史……）只存在于标签里；窗格同一时刻只加载活动标签，所以
/// 加载代号、取消令牌、占位和滚动视口跟着窗格走。「活动标签」只是
/// `active_tab_id` 这个引用，`tab()` 取到的就是唯一的那份。
#[derive(Debug, Clone)]
pub(crate) struct BrowserPane {
    pub(crate) id: BrowserPaneId,
    pub(crate) tabs: Vec<BrowserTab>,
    pub(crate) active_tab_id: usize,
    pub(crate) directory_loading_placeholder: Option<DirectoryLoadingPlaceholder>,
    pub(crate) column_browser_viewport: ColumnBrowserViewport,
    pub(crate) column_viewports: HashMap<PathBuf, ColumnViewport>,
    pub(crate) directory_load_generation: u64,
    pub(crate) directory_load_cancel: Option<CancellationToken>,
}

/// 标签迁移路径后对窗格层面的影响，由窗格对其活动标签落实。
#[derive(Clone, Copy, Default)]
struct TabMigrationEffect {
    load_interrupted: bool,
    cached_tree: CachedTreeEffect,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CachedTreeEffect {
    #[default]
    Kept,
    Invalidated,
    PathsMigrated,
}

impl BrowserPane {
    pub(crate) fn new(id: BrowserPaneId, tab: BrowserTab) -> Self {
        let active_tab_id = tab.id;
        Self::with_tabs(id, vec![tab], active_tab_id)
    }

    /// 调用方保证 `active_tab_id` 在 `tabs` 里（窗格的不变量）。
    pub(crate) fn with_tabs(
        id: BrowserPaneId,
        tabs: Vec<BrowserTab>,
        active_tab_id: usize,
    ) -> Self {
        debug_assert!(tabs.iter().any(|tab| tab.id == active_tab_id));
        Self {
            id,
            active_tab_id,
            tabs,
            directory_loading_placeholder: None,
            column_browser_viewport: ColumnBrowserViewport::default(),
            column_viewports: HashMap::new(),
            directory_load_generation: 0,
            directory_load_cancel: None,
        }
    }

    /// 窗格的活动标签。「`active_tab_id` 必在 `tabs` 里」是窗格的不变量，
    /// 由新建、关闭、拆分、合并、会话恢复这几个改变标签集合的入口维护。
    pub(crate) fn tab(&self) -> &BrowserTab {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .expect("a pane owns its active tab")
    }

    pub(crate) fn tab_mut(&mut self) -> &mut BrowserTab {
        let active_tab_id = self.active_tab_id;
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == active_tab_id)
            .expect("a pane owns its active tab")
    }

    /// 切换活动标签。占位属于离开的那个目录，不带给新标签。
    pub(crate) fn activate_tab(&mut self, tab_id: usize) {
        debug_assert!(self.tabs.iter().any(|tab| tab.id == tab_id));
        self.active_tab_id = tab_id;
        self.directory_loading_placeholder = None;
    }

    /// 开始活动标签的一次目录加载：作废上一次（取消令牌、换代号），迟到的
    /// 响应按代号被拒收。
    pub(crate) fn begin_directory_load(
        &mut self,
        path: PathBuf,
    ) -> (DirectoryLoadRequest, CancellationToken) {
        self.cancel_directory_load();
        let cancellation = CancellationToken::new();
        self.directory_load_cancel = Some(cancellation.clone());
        let request = DirectoryLoadRequest {
            pane_id: self.id,
            path,
            generation: self.directory_load_generation,
        };
        (request, cancellation)
    }

    /// 作废进行中的加载但不发起新的：令牌取消、代号前进、丢弃半成品 discovery。
    pub(crate) fn cancel_directory_load(&mut self) {
        if let Some(cancel) = self.directory_load_cancel.take() {
            cancel.cancel();
        }
        self.directory_load_generation = self.directory_load_generation.wrapping_add(1);
        self.tab_mut().directory_discovery = None;
    }

    pub(crate) fn migrate_completed_paths(&mut self, migrations: &[CompletedPathMigration]) {
        let active_tab_id = self.active_tab_id;
        let mut active_effect = TabMigrationEffect::default();
        for tab in &mut self.tabs {
            let effect = tab.migrate_completed_paths(migrations);
            if tab.id == active_tab_id {
                active_effect = effect;
            }
        }
        if active_effect.load_interrupted {
            self.cancel_current_directory_load();
        }
        match active_effect.cached_tree {
            CachedTreeEffect::Kept => {}
            CachedTreeEffect::Invalidated => self.reset_directory_view(),
            CachedTreeEffect::PathsMigrated => {
                if let Some(placeholder) = &mut self.directory_loading_placeholder {
                    for placeholder_entry in &mut placeholder.entries {
                        migrate_directory_entry(&mut placeholder_entry.entry, migrations);
                    }
                }
                migrate_path_map_keys(&mut self.column_viewports, migrations);
            }
        }
    }

    pub(crate) fn recover_from_unavailable_directory(
        &mut self,
        unavailable_directory: &Path,
        parent_directory: PathBuf,
    ) {
        let tab = self.tab_mut();
        tab.forget_history_into(unavailable_directory, &parent_directory);
        tab.current_dir = parent_directory;
        tab.is_trash_view = false;
        tab.trash_entries.clear();
        tab.invalidate_cached_directory_tree();
        tab.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        tab.directory_discovery = None;
        self.reset_directory_view();
    }

    pub(crate) fn discard_unavailable_directory_subtree(&mut self, unavailable_directory: &Path) {
        self.tab_mut()
            .discard_unavailable_directory_subtree(unavailable_directory);
        self.directory_loading_placeholder = None;
        self.column_viewports
            .retain(|path, _| !path.starts_with(unavailable_directory));
    }

    fn cancel_current_directory_load(&mut self) {
        if let Some(cancellation) = self.directory_load_cancel.take() {
            cancellation.cancel();
        }
        // File-operation callers synchronously schedule the replacement load; that
        // request owns the single generation advance for the whole commit batch.
        let tab = self.tab_mut();
        detach_directory_discovery(
            Arc::make_mut(&mut tab.entries).as_mut_slice(),
            &mut tab.directory_discovery,
            &mut tab.directory_order_phase,
        );
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
    }

    /// 活动标签换了目录或缓存的目录树作废时，窗格自己持有的那部分：占位与
    /// 多栏滚动位置。
    pub(crate) fn reset_directory_view(&mut self) {
        self.directory_loading_placeholder = None;
        self.column_browser_viewport = ColumnBrowserViewport::default();
        self.column_viewports.clear();
    }
}

/// 一个标签的全部浏览状态。每个标签只存一份：活动窗格的活动标签与其它
/// 标签没有区别，只是被引用得更多。
#[derive(Debug, Clone)]
pub(crate) struct BrowserTab {
    pub(crate) id: usize,
    pub(crate) current_dir: PathBuf,
    pub(crate) is_trash_view: bool,
    pub(crate) entries: DirectoryEntrySnapshot,
    pub(crate) directory_discovery: Option<DirectoryDiscovery>,
    pub(crate) trash_entries: Vec<TrashEntry>,
    pub(crate) selected: Option<PathBuf>,
    pub(crate) selected_paths: HashSet<PathBuf>,
    pub(crate) selection_anchor: Option<PathBuf>,
    pub(crate) deepest_open_column_directory: Option<PathBuf>,
    pub(crate) expanded_directories: HashMap<PathBuf, ExpandedDirectory>,
    pub(crate) view_mode: BrowserViewMode,
    pub(crate) back_stack: Vec<PathBuf>,
    pub(crate) forward_stack: Vec<PathBuf>,
    pub(crate) directory_collection_phase: DirectoryCollectionPhase,
    pub(crate) directory_order_phase: DirectoryOrderPhase,
    /// 目录 → 它正在被扫描时到达、还没落到列表上的变化。
    pub(crate) pending_changes: HashMap<PathBuf, PendingDirectoryChanges>,
}

/// 一次扫描期间到达的目录变化，按到达顺序排队，等这次扫描落地后重放到结果上。
/// 扫描读到的是启动那一刻的磁盘状态，期间的变化不排队就会被快照覆盖。
#[derive(Debug, Clone)]
pub(crate) struct PendingDirectoryChanges {
    /// 排队时该目录那次扫描的代号。换了新扫描（代号前进）这份就作废：新扫描
    /// 读到的已是这些变化之后的磁盘状态，重放只会把旧事件再套一遍。靠代号
    /// 比对失效，所有重新发起扫描的入口就不必各自记得清队列。
    scan_generation: u64,
    changes: Vec<ResolvedEntryChange>,
}

impl BrowserTab {
    pub(crate) fn file_drop_destination(&self) -> TabDropDestination {
        if self.is_trash_view {
            return TabDropDestination::Trash;
        }

        TabDropDestination::Directory(
            displayed_address_directory(
                &self.current_dir,
                self.view_mode,
                self.deepest_open_column_directory.as_ref(),
            )
            .to_path_buf(),
        )
    }

    pub(crate) fn directory(id: usize, directory: PathBuf) -> Self {
        Self {
            id,
            current_dir: directory,
            is_trash_view: false,
            entries: empty_directory_entry_snapshot(),
            directory_discovery: None,
            trash_entries: Vec::new(),
            selected: None,
            selected_paths: HashSet::new(),
            selection_anchor: None,
            deepest_open_column_directory: None,
            expanded_directories: HashMap::new(),
            view_mode: BrowserViewMode::Columns,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            directory_collection_phase: DirectoryCollectionPhase::Discovering,
            directory_order_phase: DirectoryOrderPhase::Ready {
                field: SortField::Name,
                direction: SortDirection::Ascending,
            },
            pending_changes: HashMap::new(),
        }
    }

    pub(crate) fn queue_changes_during_scan(
        &mut self,
        directory: &Path,
        scan_generation: u64,
        changes: &[ResolvedEntryChange],
    ) {
        let pending = self
            .pending_changes
            .entry(directory.to_path_buf())
            .or_insert_with(|| PendingDirectoryChanges {
                scan_generation,
                changes: Vec::new(),
            });
        if pending.scan_generation != scan_generation {
            pending.scan_generation = scan_generation;
            pending.changes.clear();
        }
        pending.changes.extend_from_slice(changes);
    }

    /// 取走 `scan_generation` 这次扫描期间排队的变化。调用方已确认这次扫描
    /// 就是该目录当前的扫描，所以其它代号的队列属于被取代的扫描，直接丢弃。
    pub(crate) fn take_changes_queued_during_scan(
        &mut self,
        directory: &Path,
        scan_generation: u64,
    ) -> Vec<ResolvedEntryChange> {
        let changes = self
            .pending_changes
            .remove(directory)
            .filter(|pending| pending.scan_generation == scan_generation)
            .map(|pending| pending.changes)
            .unwrap_or_default();
        // 队列只属于标签仍在显示的目录；导航走开后遗留的队列在此清掉。
        let current_dir = &self.current_dir;
        let expanded_directories = &self.expanded_directories;
        self.pending_changes.retain(|queued_directory, _| {
            queued_directory == current_dir || expanded_directories.contains_key(queued_directory)
        });
        changes
    }

    pub(crate) fn trash(id: usize) -> Self {
        let mut tab = Self::directory(id, super::trash_location_path());
        tab.is_trash_view = true;
        tab
    }

    /// 目录消失后让历史不再指向它（及其子树），也不让「后退」落回 `landing`
    /// 自己。
    pub(crate) fn forget_history_into(&mut self, unavailable_directory: &Path, landing: &Path) {
        self.back_stack
            .retain(|path| !path.starts_with(unavailable_directory));
        while self.back_stack.last().is_some_and(|path| path == landing) {
            self.back_stack.pop();
        }
        self.forward_stack
            .retain(|path| !path.starts_with(unavailable_directory));
    }

    /// 离开当前目录树：取消展开目录的加载，清空展开状态与多栏链。
    pub(crate) fn close_expanded_directories(&mut self) {
        self.deepest_open_column_directory = None;
        cancel_and_clear_expanded_directories(&mut self.expanded_directories);
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_paths.clear();
        self.selection_anchor = None;
    }

    /// 只保留仍是主列表直接条目的选择（切换到/离开图标视图时展开内容不再可见）。
    pub(crate) fn retain_direct_entry_selection(&mut self) {
        retain_direct_entry_selection(
            &self.entries,
            &mut self.selected,
            &mut self.selected_paths,
            &mut self.selection_anchor,
        );
    }

    fn migrate_completed_paths(
        &mut self,
        migrations: &[CompletedPathMigration],
    ) -> TabMigrationEffect {
        let original_directory = self.current_dir.clone();
        let migrated_directory = path_after_completed_migrations(&original_directory, migrations);
        let directory_was_migrated = migrated_directory != original_directory;
        let directory_tree_was_touched =
            completed_migrations_touch_directory_tree(&original_directory, migrations);
        let migration_left_directory_tree =
            completed_migrations_leave_directory_tree(&original_directory, migrations);
        let migration_has_source_in_directory_tree =
            completed_migrations_have_source_in_directory_tree(&original_directory, migrations);

        self.current_dir = migrated_directory;
        let cached_tree = if !directory_was_migrated && migration_left_directory_tree {
            self.invalidate_cached_directory_tree();
            CachedTreeEffect::Invalidated
        } else if directory_was_migrated || migration_has_source_in_directory_tree {
            self.migrate_cached_directory_tree_paths(migrations);
            CachedTreeEffect::PathsMigrated
        } else {
            CachedTreeEffect::Kept
        };
        migrate_path_list(&mut self.back_stack, migrations);
        migrate_path_list(&mut self.forward_stack, migrations);
        TabMigrationEffect {
            load_interrupted: directory_was_migrated || directory_tree_was_touched,
            cached_tree,
        }
    }

    fn migrate_cached_directory_tree_paths(&mut self, migrations: &[CompletedPathMigration]) {
        detach_directory_discovery(
            Arc::make_mut(&mut self.entries).as_mut_slice(),
            &mut self.directory_discovery,
            &mut self.directory_order_phase,
        );
        migrate_directory_entries(Arc::make_mut(&mut self.entries).as_mut_slice(), migrations);
        migrate_optional_path(&mut self.selected, migrations);
        migrate_path_set(&mut self.selected_paths, migrations);
        migrate_optional_path(&mut self.selection_anchor, migrations);
        migrate_optional_path(&mut self.deepest_open_column_directory, migrations);
        migrate_expanded_directories(&mut self.expanded_directories, migrations);
    }

    fn invalidate_cached_directory_tree(&mut self) {
        self.entries = empty_directory_entry_snapshot();
        self.directory_discovery = None;
        self.selected = None;
        self.selected_paths.clear();
        self.selection_anchor = None;
        self.deepest_open_column_directory = None;
        cancel_and_clear_expanded_directories(&mut self.expanded_directories);
    }

    fn discard_unavailable_directory_subtree(&mut self, unavailable_directory: &Path) {
        detach_directory_discovery(
            Arc::make_mut(&mut self.entries).as_mut_slice(),
            &mut self.directory_discovery,
            &mut self.directory_order_phase,
        );
        Arc::make_mut(&mut self.entries)
            .retain(|entry| !entry.path.starts_with(unavailable_directory));
        retain_optional_path_outside_subtree(&mut self.selected, unavailable_directory);
        self.selected_paths
            .retain(|path| !path.starts_with(unavailable_directory));
        retain_optional_path_outside_subtree(&mut self.selection_anchor, unavailable_directory);
        if self
            .deepest_open_column_directory
            .as_ref()
            .is_some_and(|directory| directory.starts_with(unavailable_directory))
        {
            self.deepest_open_column_directory =
                unavailable_directory.parent().and_then(|parent| {
                    if parent == self.current_dir.as_path()
                        || !parent.starts_with(&self.current_dir)
                    {
                        None
                    } else {
                        Some(parent.to_path_buf())
                    }
                });
        }
        for expanded_directory in self.expanded_directories.values_mut() {
            if expanded_directory
                .entries
                .iter()
                .any(|entry| entry.path.starts_with(unavailable_directory))
            {
                detach_directory_discovery(
                    &mut expanded_directory.entries,
                    &mut expanded_directory.directory_discovery,
                    &mut expanded_directory.directory_order_phase,
                );
                expanded_directory
                    .entries
                    .retain(|entry| !entry.path.starts_with(unavailable_directory));
            }
        }
        self.expanded_directories
            .retain(|path, expanded_directory| {
                let directory_remains_available = !path.starts_with(unavailable_directory);
                if !directory_remains_available {
                    if let Some(cancellation) = expanded_directory.load_cancel.take() {
                        cancellation.cancel();
                    }
                }
                directory_remains_available
            });
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedDirectory {
    pub(crate) entries: Vec<DirectoryEntry>,
    pub(crate) directory_discovery: Option<DirectoryDiscovery>,
    pub(crate) status: ExpandedDirectoryStatus,
    pub(crate) is_expanded: bool,
    pub(crate) is_collapsing: bool,
    pub(crate) animation_progress: f32,
    pub(crate) load_generation: u64,
    pub(crate) load_context: Option<DirectoryExpansionLoadContext>,
    pub(crate) load_cancel: Option<CancellationToken>,
    pub(crate) directory_order_phase: DirectoryOrderPhase,
}

#[derive(Debug, Clone)]
pub(crate) enum ExpandedDirectoryStatus {
    Loading,
    Loaded,
    Error,
}

pub(crate) fn retain_direct_entry_selection(
    entries: &[DirectoryEntry],
    selected: &mut Option<PathBuf>,
    selected_paths: &mut HashSet<PathBuf>,
    selection_anchor: &mut Option<PathBuf>,
) {
    if selected.is_none() && selected_paths.is_empty() && selection_anchor.is_none() {
        return;
    }

    let direct_paths = entries
        .iter()
        .map(|entry| entry.path.as_path())
        .collect::<HashSet<_>>();
    selected_paths.retain(|path| direct_paths.contains(path.as_path()));
    if !selected
        .as_ref()
        .is_some_and(|path| direct_paths.contains(path.as_path()))
    {
        *selected = entries
            .iter()
            .rev()
            .find(|entry| selected_paths.contains(&entry.path))
            .map(|entry| entry.path.clone());
    }
    if selection_anchor
        .as_ref()
        .is_some_and(|path| !direct_paths.contains(path.as_path()))
    {
        *selection_anchor = None;
    }
}
