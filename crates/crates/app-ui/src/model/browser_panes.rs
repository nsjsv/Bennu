use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    DirectoryDiscovery, DirectoryEntry, DirectoryMetadataRequirement, SortDirection, SortField,
    TrashEntry,
};
use tokio_util::sync::CancellationToken;

use crate::operation_history::{
    completed_migrations_have_source_in_directory_tree, completed_migrations_leave_directory_tree,
    completed_migrations_touch_directory_tree, path_after_completed_migrations,
    CompletedPathMigration,
};
use crate::thumbnail_cache::ColumnViewport;

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

#[derive(Debug, Clone)]
pub(crate) struct BrowserPane {
    pub(crate) id: BrowserPaneId,
    pub(crate) current_dir: PathBuf,
    pub(crate) is_trash_view: bool,
    pub(crate) entries: DirectoryEntrySnapshot,
    pub(crate) directory_discovery: Option<DirectoryDiscovery>,
    pub(crate) directory_loading_placeholder: Option<DirectoryLoadingPlaceholder>,
    pub(crate) trash_entries: Vec<TrashEntry>,
    pub(crate) selected: Option<PathBuf>,
    pub(crate) selected_paths: HashSet<PathBuf>,
    pub(crate) selection_anchor: Option<PathBuf>,
    pub(crate) deepest_open_column_directory: Option<PathBuf>,
    pub(crate) expanded_directories: HashMap<PathBuf, ExpandedDirectory>,
    pub(crate) view_mode: BrowserViewMode,
    pub(crate) column_browser_viewport: ColumnBrowserViewport,
    pub(crate) column_viewports: HashMap<PathBuf, ColumnViewport>,
    pub(crate) tabs: Vec<BrowserTab>,
    pub(crate) active_tab_id: usize,
    pub(crate) directory_load_generation: u64,
    pub(crate) directory_load_cancel: Option<CancellationToken>,
    pub(crate) back_stack: Vec<PathBuf>,
    pub(crate) forward_stack: Vec<PathBuf>,
    pub(crate) directory_collection_phase: DirectoryCollectionPhase,
    pub(crate) directory_order_phase: DirectoryOrderPhase,
}

impl BrowserPane {
    pub(crate) fn sync_active_tab_state(&mut self) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab_id)
        else {
            return;
        };

        tab.directory = self.current_dir.clone();
        tab.is_trash_view = self.is_trash_view;
        tab.entries = self.entries.clone();
        tab.directory_discovery = self.directory_discovery.clone();
        tab.trash_entries = self.trash_entries.clone();
        tab.selected = self.selected.clone();
        tab.selected_paths = self.selected_paths.clone();
        tab.selection_anchor = self.selection_anchor.clone();
        tab.deepest_open_column_directory = self.deepest_open_column_directory.clone();
        tab.expanded_directories = self.expanded_directories.clone();
        tab.view_mode = self.view_mode;
        tab.back_stack = self.back_stack.clone();
        tab.forward_stack = self.forward_stack.clone();
        tab.directory_collection_phase = self.directory_collection_phase;
        tab.directory_order_phase = self.directory_order_phase;
    }

    pub(crate) fn migrate_completed_paths(&mut self, migrations: &[CompletedPathMigration]) {
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
        if directory_was_migrated || directory_tree_was_touched {
            self.cancel_current_directory_load();
        }
        if !directory_was_migrated && migration_left_directory_tree {
            self.invalidate_cached_directory_tree();
        } else if directory_was_migrated || migration_has_source_in_directory_tree {
            self.migrate_cached_directory_tree_paths(migrations);
        }
        migrate_path_list(&mut self.back_stack, migrations);
        migrate_path_list(&mut self.forward_stack, migrations);
        for tab in &mut self.tabs {
            tab.migrate_completed_paths(migrations);
        }
    }

    pub(crate) fn recover_from_unavailable_directory(
        &mut self,
        unavailable_directory: &Path,
        parent_directory: PathBuf,
    ) {
        self.current_dir = parent_directory;
        self.is_trash_view = false;
        self.trash_entries.clear();
        self.invalidate_cached_directory_tree();
        self.back_stack
            .retain(|path| !path.starts_with(unavailable_directory));
        while self
            .back_stack
            .last()
            .is_some_and(|path| path == &self.current_dir)
        {
            self.back_stack.pop();
        }
        self.forward_stack
            .retain(|path| !path.starts_with(unavailable_directory));
        self.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.directory_discovery = None;
        self.sync_active_tab_state();
    }

    pub(crate) fn discard_unavailable_directory_subtree(&mut self, unavailable_directory: &Path) {
        detach_directory_discovery(
            Arc::make_mut(&mut self.entries).as_mut_slice(),
            &mut self.directory_discovery,
            &mut self.directory_order_phase,
        );
        Arc::make_mut(&mut self.entries)
            .retain(|entry| !entry.path.starts_with(unavailable_directory));
        self.directory_loading_placeholder = None;
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
        self.column_viewports
            .retain(|path, _| !path.starts_with(unavailable_directory));
        self.sync_active_tab_state();
    }

    fn cancel_current_directory_load(&mut self) {
        if let Some(cancellation) = self.directory_load_cancel.take() {
            cancellation.cancel();
        }
        // File-operation callers synchronously schedule the replacement load; that
        // request owns the single generation advance for the whole commit batch.
        detach_directory_discovery(
            Arc::make_mut(&mut self.entries).as_mut_slice(),
            &mut self.directory_discovery,
            &mut self.directory_order_phase,
        );
        self.directory_collection_phase = DirectoryCollectionPhase::Ready;
    }

    fn migrate_cached_directory_tree_paths(&mut self, migrations: &[CompletedPathMigration]) {
        detach_directory_discovery(
            Arc::make_mut(&mut self.entries).as_mut_slice(),
            &mut self.directory_discovery,
            &mut self.directory_order_phase,
        );
        migrate_directory_entries(Arc::make_mut(&mut self.entries).as_mut_slice(), migrations);
        if let Some(placeholder) = &mut self.directory_loading_placeholder {
            for placeholder_entry in &mut placeholder.entries {
                migrate_directory_entry(&mut placeholder_entry.entry, migrations);
            }
        }
        migrate_optional_path(&mut self.selected, migrations);
        migrate_path_set(&mut self.selected_paths, migrations);
        migrate_optional_path(&mut self.selection_anchor, migrations);
        migrate_optional_path(&mut self.deepest_open_column_directory, migrations);
        migrate_expanded_directories(&mut self.expanded_directories, migrations);
        migrate_path_map_keys(&mut self.column_viewports, migrations);
    }

    fn invalidate_cached_directory_tree(&mut self) {
        self.entries = empty_directory_entry_snapshot();
        self.directory_discovery = None;
        self.directory_loading_placeholder = None;
        self.selected = None;
        self.selected_paths.clear();
        self.selection_anchor = None;
        self.deepest_open_column_directory = None;
        cancel_and_clear_expanded_directories(&mut self.expanded_directories);
        self.column_browser_viewport = ColumnBrowserViewport::default();
        self.column_viewports.clear();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserTab {
    pub(crate) id: usize,
    pub(crate) directory: PathBuf,
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
}

impl BrowserTab {
    pub(crate) fn file_drop_destination(&self) -> TabDropDestination {
        if self.is_trash_view {
            return TabDropDestination::Trash;
        }

        TabDropDestination::Directory(
            displayed_address_directory(
                &self.directory,
                self.view_mode,
                self.deepest_open_column_directory.as_ref(),
            )
            .to_path_buf(),
        )
    }

    pub(crate) fn directory(id: usize, directory: PathBuf) -> Self {
        Self {
            id,
            directory,
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
        }
    }

    pub(crate) fn trash(id: usize) -> Self {
        Self {
            id,
            directory: super::trash_location_path(),
            is_trash_view: true,
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
        }
    }

    fn migrate_completed_paths(&mut self, migrations: &[CompletedPathMigration]) {
        let original_directory = self.directory.clone();
        let migrated_directory = path_after_completed_migrations(&original_directory, migrations);
        let directory_was_migrated = migrated_directory != original_directory;
        let migration_left_directory_tree =
            completed_migrations_leave_directory_tree(&original_directory, migrations);
        let migration_has_source_in_directory_tree =
            completed_migrations_have_source_in_directory_tree(&original_directory, migrations);

        self.directory = migrated_directory;
        if !directory_was_migrated && migration_left_directory_tree {
            self.invalidate_cached_directory_tree();
        } else if directory_was_migrated || migration_has_source_in_directory_tree {
            self.migrate_cached_directory_tree_paths(migrations);
        }
        migrate_path_list(&mut self.back_stack, migrations);
        migrate_path_list(&mut self.forward_stack, migrations);
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

pub(super) fn migrate_directory_entries(
    entries: &mut [DirectoryEntry],
    migrations: &[CompletedPathMigration],
) {
    for entry in entries {
        migrate_directory_entry(entry, migrations);
    }
}

fn migrate_directory_entry(entry: &mut DirectoryEntry, migrations: &[CompletedPathMigration]) {
    let migrated_path = path_after_completed_migrations(&entry.path, migrations);
    if migrated_path == entry.path {
        return;
    }
    entry.name = migrated_path
        .file_name()
        .unwrap_or_else(|| migrated_path.as_os_str())
        .to_os_string();
    entry.path = migrated_path;
}

fn migrate_optional_path(path: &mut Option<PathBuf>, migrations: &[CompletedPathMigration]) {
    if let Some(path) = path {
        *path = path_after_completed_migrations(path, migrations);
    }
}

fn retain_optional_path_outside_subtree(path: &mut Option<PathBuf>, subtree: &Path) {
    if path
        .as_ref()
        .is_some_and(|candidate| candidate.starts_with(subtree))
    {
        *path = None;
    }
}

fn migrate_path_list(paths: &mut [PathBuf], migrations: &[CompletedPathMigration]) {
    for path in paths {
        *path = path_after_completed_migrations(path, migrations);
    }
}

fn migrate_path_set(paths: &mut HashSet<PathBuf>, migrations: &[CompletedPathMigration]) {
    *paths = paths
        .drain()
        .map(|path| path_after_completed_migrations(&path, migrations))
        .collect();
}

fn migrate_path_map_keys<Value>(
    paths: &mut HashMap<PathBuf, Value>,
    migrations: &[CompletedPathMigration],
) {
    *paths = paths
        .drain()
        .map(|(path, value)| (path_after_completed_migrations(&path, migrations), value))
        .collect();
}

fn detach_directory_discovery(
    entries: &mut [DirectoryEntry],
    discovery: &mut Option<DirectoryDiscovery>,
    order_phase: &mut DirectoryOrderPhase,
) {
    if let Some(discovery) = discovery.take() {
        for entry in entries {
            let Some(index) = entry.discovery_index.take() else {
                continue;
            };
            let Some(discovered) = discovery.entries.get(index) else {
                continue;
            };
            let materialized = discovered.display_entry();
            entry.metadata = materialized.metadata;
            entry.is_broken_symlink = materialized.is_broken_symlink;
        }
    }
    if let DirectoryOrderPhase::WaitingForMetadata {
        field, direction, ..
    } = *order_phase
    {
        *order_phase = DirectoryOrderPhase::Ready { field, direction };
    }
}

fn migrate_expanded_directories(
    directories: &mut HashMap<PathBuf, ExpandedDirectory>,
    migrations: &[CompletedPathMigration],
) {
    *directories = directories
        .drain()
        .filter_map(|(path, mut directory)| {
            if let Some(cancellation) = directory.load_cancel.take() {
                cancellation.cancel();
            }
            if matches!(directory.status, ExpandedDirectoryStatus::Loading) {
                return None;
            }
            detach_directory_discovery(
                &mut directory.entries,
                &mut directory.directory_discovery,
                &mut directory.directory_order_phase,
            );
            migrate_directory_entries(&mut directory.entries, migrations);
            Some((
                path_after_completed_migrations(&path, migrations),
                directory,
            ))
        })
        .collect();
}

fn cancel_and_clear_expanded_directories(directories: &mut HashMap<PathBuf, ExpandedDirectory>) {
    for directory in directories.values_mut() {
        if let Some(cancellation) = directory.load_cancel.take() {
            cancellation.cancel();
        }
    }
    directories.clear();
}
