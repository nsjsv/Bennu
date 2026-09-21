use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, FileKind};

use crate::operation_history::{
    completed_migrations_cross_directory_tree_boundary, path_after_completed_migrations,
    CompletedPathMigration,
};

use super::browser_panes::migrate_directory_entries;
use super::{
    BrowserPaneId, ExpandedDirectory, ExpandedDirectoryStatus, IconGridExpansionSessionId,
};

#[path = "icon_grid_expansion_follow.rs"]
mod follow;
pub(crate) use follow::IconGridExpansionFollowAdvance;
use follow::IconGridExpansionFollowPlan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconGridExpansionContext {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) current_dir: PathBuf,
    pub(crate) session_id: IconGridExpansionSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconGridExpansionAnchor {
    pub(crate) parent_directory: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) index: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct IconGridExpandedDirectory {
    pub(crate) parent_directory: PathBuf,
    pub(crate) anchor_index: usize,
    pub(crate) contents: ExpandedDirectory,
}

impl IconGridExpandedDirectory {
    pub(crate) fn new(anchor: &IconGridExpansionAnchor, contents: ExpandedDirectory) -> Self {
        Self {
            parent_directory: anchor.parent_directory.clone(),
            anchor_index: anchor.index,
            contents,
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.contents.is_expanded || self.contents.is_collapsing
    }

    pub(crate) fn is_interactive(&self) -> bool {
        self.contents.is_expanded
            && !self.contents.is_collapsing
            && matches!(self.contents.status, ExpandedDirectoryStatus::Loaded)
            && (1.0 - self.contents.animation_progress).abs() <= f32::EPSILON
    }

    fn cancel_pending_load(&mut self) {
        if let Some(cancellation) = self.contents.load_cancel.take() {
            cancellation.cancel();
        }
        self.contents.load_generation = self.contents.load_generation.wrapping_add(1);
        self.contents.load_context = None;
    }

    fn begin_closing(&mut self) {
        self.contents.is_expanded = false;
        self.contents.animation_progress = self.contents.animation_progress.clamp(0.0, 1.0);
        self.contents.is_collapsing = self.contents.animation_progress > f32::EPSILON;
    }

    fn reopen(&mut self) {
        self.contents.is_expanded = true;
        self.contents.is_collapsing = false;
        self.contents.animation_progress = self.contents.animation_progress.clamp(0.0, 1.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IconGridRemovedPathReconciliation {
    Retained { hidden_paths: Vec<PathBuf> },
    RootRemoved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconGridExpansionMigration {
    Retained,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconGridAnchorReconciliation {
    Retained,
    RootRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconGridChildSwitch {
    pub(crate) hidden_paths: Vec<PathBuf>,
    pub(crate) closing_path: Option<PathBuf>,
    pub(crate) ready_child: Option<IconGridExpansionAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconGridAnimationAdvance {
    pub(crate) changed: bool,
    pub(crate) root_closed: bool,
    pub(crate) entries_became_interactive: bool,
    pub(crate) ready_children: Vec<IconGridExpansionAnchor>,
}

#[derive(Debug, Clone)]
pub(crate) struct IconGridExpansionState {
    context: IconGridExpansionContext,
    root_path: PathBuf,
    directories: HashMap<PathBuf, IconGridExpandedDirectory>,
    pending_root: Option<IconGridExpansionAnchor>,
    pending_children: HashMap<PathBuf, IconGridExpansionAnchor>,
    selection_directory: PathBuf,
    follow_plan: Option<IconGridExpansionFollowPlan>,
}

impl IconGridExpansionState {
    pub(crate) fn new(
        context: IconGridExpansionContext,
        root_anchor: IconGridExpansionAnchor,
        root_contents: ExpandedDirectory,
    ) -> Self {
        debug_assert_eq!(root_anchor.parent_directory, context.current_dir);
        let root_path = root_anchor.path.clone();
        let selection_directory = context.current_dir.clone();
        let mut directories = HashMap::new();
        directories.insert(
            root_path.clone(),
            IconGridExpandedDirectory::new(&root_anchor, root_contents),
        );
        Self {
            context,
            root_path,
            directories,
            pending_root: None,
            pending_children: HashMap::new(),
            selection_directory,
            follow_plan: None,
        }
    }

    pub(crate) fn context(&self) -> &IconGridExpansionContext {
        &self.context
    }

    pub(crate) fn matches_context(
        &self,
        pane_id: BrowserPaneId,
        current_dir: &Path,
        session_id: IconGridExpansionSessionId,
    ) -> bool {
        self.context.pane_id == pane_id
            && self.context.current_dir == current_dir
            && self.context.session_id == session_id
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub(crate) fn selection_directory(&self) -> &Path {
        &self.selection_directory
    }

    pub(crate) fn set_selection_directory(&mut self, directory: &Path) {
        if directory == self.context.current_dir || self.node_is_interactive(directory) {
            self.selection_directory = directory.to_path_buf();
        }
    }

    pub(crate) fn directory(&self, path: &Path) -> Option<&IconGridExpandedDirectory> {
        self.directories.get(path)
    }

    pub(crate) fn directory_mut(&mut self, path: &Path) -> Option<&mut IconGridExpandedDirectory> {
        self.directories.get_mut(path)
    }

    pub(crate) fn directories(&self) -> impl Iterator<Item = (&Path, &IconGridExpandedDirectory)> {
        self.directories
            .iter()
            .map(|(path, directory)| (path.as_path(), directory))
    }

    #[cfg(test)]
    pub(crate) fn directory_count(&self) -> usize {
        self.directories.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_root(&self) -> Option<&IconGridExpansionAnchor> {
        self.pending_root.as_ref()
    }

    pub(crate) fn take_pending_root(&mut self) -> Option<IconGridExpansionAnchor> {
        self.pending_root.take()
    }

    pub(crate) fn path_is_pending(&self, path: &Path) -> bool {
        self.pending_root
            .as_ref()
            .is_some_and(|pending| pending.path == path)
            || self
                .pending_children
                .values()
                .any(|pending| pending.path == path)
    }

    #[cfg(test)]
    pub(crate) fn pending_child(
        &self,
        parent_directory: &Path,
    ) -> Option<&IconGridExpansionAnchor> {
        self.pending_children.get(parent_directory)
    }

    pub(crate) fn insert_directory(
        &mut self,
        anchor: IconGridExpansionAnchor,
        contents: ExpandedDirectory,
    ) -> bool {
        if anchor.path.parent() != Some(anchor.parent_directory.as_path())
            || self.directories.contains_key(&anchor.path)
            || self
                .directories
                .values()
                .any(|directory| directory.parent_directory == anchor.parent_directory)
            || !self.directory_is_interactive(&anchor.parent_directory)
        {
            return false;
        }
        self.directories.insert(
            anchor.path.clone(),
            IconGridExpandedDirectory::new(&anchor, contents),
        );
        true
    }

    pub(crate) fn entry_paths_in_subtree(&self, path: &Path) -> Vec<PathBuf> {
        self.entry_paths_for_nodes(&self.subtree_paths(path))
    }

    pub(crate) fn contains_tree_path(&self, path: &Path) -> bool {
        path == self.root_path
            || self.directories.iter().any(|(directory_path, directory)| {
                self.node_is_interactive(directory_path)
                    && directory
                        .contents
                        .entries
                        .iter()
                        .any(|entry| entry.path == path)
            })
    }

    pub(crate) fn accepts_directory_load(&self, path: &Path) -> bool {
        let mut current = path.to_path_buf();
        loop {
            let Some(directory) = self.directories.get(&current) else {
                return false;
            };
            if !directory.contents.is_expanded || directory.contents.is_collapsing {
                return false;
            }
            if current == self.root_path {
                return true;
            }
            current = directory.parent_directory.clone();
        }
    }

    pub(crate) fn entries_in_interactive_directory(
        &self,
        directory: &Path,
    ) -> Option<&[DirectoryEntry]> {
        self.node_is_interactive(directory)
            .then(|| self.directories.get(directory))
            .flatten()
            .map(|expanded| expanded.contents.entries.as_slice())
    }

    pub(crate) fn entry(&self, path: &Path) -> Option<&DirectoryEntry> {
        self.directories
            .iter()
            .filter(|(directory, _)| self.node_is_interactive(directory))
            .flat_map(|(_, expanded)| expanded.contents.entries.iter())
            .find(|entry| entry.path == path)
    }

    fn begin_directory_close(&mut self, path: &Path) -> Vec<PathBuf> {
        let subtree_paths = self.subtree_paths(path);
        if subtree_paths.is_empty() {
            return Vec::new();
        }
        let hidden_paths = self.entry_paths_for_nodes(&subtree_paths);
        self.pending_children
            .retain(|parent, _| !subtree_paths.contains(parent));
        for subtree_path in &subtree_paths {
            if let Some(directory) = self.directories.get_mut(subtree_path) {
                directory.cancel_pending_load();
            }
        }
        if let Some(directory) = self.directories.get_mut(path) {
            directory.begin_closing();
        }
        if subtree_paths.contains(&self.selection_directory) {
            self.selection_directory = self
                .directories
                .get(path)
                .map(|directory| directory.parent_directory.clone())
                .unwrap_or_else(|| self.context.current_dir.clone());
        }
        let closed_immediately = self.directories.get(path).is_some_and(|directory| {
            !directory.contents.is_expanded
                && !directory.contents.is_collapsing
                && directory.contents.animation_progress <= f32::EPSILON
        });
        if closed_immediately && path != self.root_path {
            self.remove_subtree(path);
        }
        hidden_paths
    }

    pub(crate) fn begin_directory_dismissal(&mut self, path: &Path) -> Vec<PathBuf> {
        self.cancel_follow_plan();
        if let Some(parent_directory) = self
            .directories
            .get(path)
            .map(|directory| directory.parent_directory.clone())
        {
            self.pending_children.remove(&parent_directory);
        }
        self.begin_directory_close(path)
    }

    pub(crate) fn begin_child_switch(
        &mut self,
        next_child: IconGridExpansionAnchor,
    ) -> IconGridChildSwitch {
        self.cancel_follow_plan();
        let parent_directory = next_child.parent_directory.clone();
        if parent_directory == self.context.current_dir
            || next_child.path.parent() != Some(parent_directory.as_path())
            || self.directories.contains_key(&next_child.path)
            || !self.directory_is_interactive(&parent_directory)
        {
            return IconGridChildSwitch {
                hidden_paths: Vec::new(),
                closing_path: None,
                ready_child: None,
            };
        }

        let closing_path = self
            .directories
            .iter()
            .find(|(_, directory)| directory.parent_directory == parent_directory)
            .map(|(path, _)| path.clone());
        let Some(closing_path) = closing_path else {
            return IconGridChildSwitch {
                hidden_paths: Vec::new(),
                closing_path: None,
                ready_child: Some(next_child),
            };
        };

        self.pending_children
            .insert(parent_directory.clone(), next_child);
        let hidden_paths = self.begin_directory_close(&closing_path);
        let ready_child = if self.directories.contains_key(&closing_path) {
            None
        } else {
            self.pending_children.remove(&parent_directory)
        };
        IconGridChildSwitch {
            hidden_paths,
            closing_path: Some(closing_path),
            ready_child,
        }
    }

    pub(crate) fn begin_root_dismissal(&mut self) -> Vec<PathBuf> {
        self.cancel_follow_plan();
        self.pending_root = None;
        self.pending_children.clear();
        let root_path = self.root_path.clone();
        self.begin_directory_close(&root_path)
    }

    pub(crate) fn begin_root_replacement(
        &mut self,
        next_root: IconGridExpansionAnchor,
    ) -> Vec<PathBuf> {
        self.cancel_follow_plan();
        self.pending_root = Some(next_root);
        self.pending_children.clear();
        let root_path = self.root_path.clone();
        self.begin_directory_close(&root_path)
    }

    pub(crate) fn reopen_directory(&mut self, path: &Path) -> bool {
        self.cancel_follow_plan();
        let Some(parent_directory) = self.directories.get_mut(path).map(|directory| {
            directory.reopen();
            directory.parent_directory.clone()
        }) else {
            return false;
        };
        if path == self.root_path {
            self.pending_root = None;
        } else {
            self.pending_children.remove(&parent_directory);
        }
        true
    }

    pub(crate) fn loading_subtree_paths(&self, root: &Path) -> Vec<PathBuf> {
        let subtree_paths = self.subtree_paths(root);
        let mut loading_paths = self
            .directories
            .iter()
            .filter(|(path, directory)| {
                subtree_paths.contains(path.as_path())
                    && directory.contents.is_expanded
                    && matches!(directory.contents.status, ExpandedDirectoryStatus::Loading)
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        loading_paths.sort();
        loading_paths
    }

    pub(crate) fn root_is_closed(&self) -> bool {
        self.directories.get(&self.root_path).is_some_and(|root| {
            !root.contents.is_expanded
                && !root.contents.is_collapsing
                && root.contents.animation_progress <= f32::EPSILON
        })
    }

    pub(crate) fn animation_is_active(&self) -> bool {
        !self.pending_children.is_empty()
            || self.directories.values().any(|directory| {
                directory.contents.is_collapsing
                    || (directory.contents.is_expanded
                        && !matches!(directory.contents.status, ExpandedDirectoryStatus::Loading)
                        && directory.contents.animation_progress < 1.0)
            })
    }

    pub(crate) fn advance_animations(&mut self, step: f32) -> IconGridAnimationAdvance {
        let step = step.max(0.0);
        let mut changed = false;
        let mut entries_became_interactive = false;
        let mut closed_paths = Vec::new();
        for (path, directory) in &mut self.directories {
            if directory.contents.is_collapsing {
                let next = (directory.contents.animation_progress - step).max(0.0);
                changed |= (next - directory.contents.animation_progress).abs() > f32::EPSILON;
                directory.contents.animation_progress = next;
                if next <= f32::EPSILON {
                    directory.contents.is_collapsing = false;
                    closed_paths.push(path.clone());
                }
            } else if directory.contents.is_expanded
                && !matches!(directory.contents.status, ExpandedDirectoryStatus::Loading)
                && directory.contents.animation_progress < 1.0
            {
                let next = (directory.contents.animation_progress + step).min(1.0);
                changed |= (next - directory.contents.animation_progress).abs() > f32::EPSILON;
                entries_became_interactive |= next >= 1.0;
                directory.contents.animation_progress = next;
            }
        }

        let root_closed = closed_paths.iter().any(|path| path == &self.root_path);
        for path in closed_paths {
            if path != self.root_path {
                self.remove_subtree(&path);
            }
        }

        let occupied_parents = self
            .directories
            .values()
            .map(|directory| directory.parent_directory.clone())
            .collect::<HashSet<_>>();
        let mut ready_parents = self
            .pending_children
            .keys()
            .filter(|parent| {
                !occupied_parents.contains(*parent) && self.directory_is_interactive(parent)
            })
            .cloned()
            .collect::<Vec<_>>();
        ready_parents.sort();
        let mut ready_children = ready_parents
            .into_iter()
            .filter_map(|parent| self.pending_children.remove(&parent))
            .collect::<Vec<_>>();
        ready_children.sort_by(|left, right| left.path.cmp(&right.path));

        IconGridAnimationAdvance {
            changed,
            root_closed,
            entries_became_interactive,
            ready_children,
        }
    }

    pub(crate) fn reconcile_child_anchors(
        &mut self,
        parent_directory: &Path,
        entries: &[DirectoryEntry],
    ) -> IconGridAnchorReconciliation {
        let directory_positions = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind == FileKind::Directory)
            .map(|(index, entry)| (entry.path.clone(), index))
            .collect::<HashMap<_, _>>();
        let child_paths = self
            .directories
            .iter()
            .filter(|(_, directory)| directory.parent_directory == parent_directory)
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let mut missing_paths = Vec::new();
        for child_path in child_paths {
            match directory_positions.get(&child_path) {
                Some(index) => {
                    if let Some(directory) = self.directories.get_mut(&child_path) {
                        directory.anchor_index = *index;
                    }
                }
                None => missing_paths.push(child_path),
            }
        }

        if let Some(pending_root) = self
            .pending_root
            .as_mut()
            .filter(|pending| pending.parent_directory == parent_directory)
        {
            match directory_positions.get(&pending_root.path) {
                Some(index) => pending_root.index = *index,
                None => self.pending_root = None,
            }
        }

        let remove_pending_child =
            self.pending_children
                .get_mut(parent_directory)
                .is_some_and(|pending| match directory_positions.get(&pending.path) {
                    Some(index) => {
                        pending.index = *index;
                        false
                    }
                    None => true,
                });
        if remove_pending_child {
            self.pending_children.remove(parent_directory);
        }

        if missing_paths.iter().any(|path| path == &self.root_path) {
            self.cancel_all_loads();
            return IconGridAnchorReconciliation::RootRemoved;
        }
        for missing_path in missing_paths {
            self.remove_subtree(&missing_path);
        }
        IconGridAnchorReconciliation::Retained
    }

    pub(crate) fn reconcile_removed_paths(
        &mut self,
        removed_paths: &[PathBuf],
    ) -> IconGridRemovedPathReconciliation {
        let mut removed_nodes = self
            .directories
            .keys()
            .filter(|node| {
                removed_paths
                    .iter()
                    .any(|removed| node.as_path() == removed || node.starts_with(removed))
            })
            .cloned()
            .collect::<Vec<_>>();
        if removed_nodes.iter().any(|node| node == &self.root_path) {
            self.cancel_all_loads();
            return IconGridRemovedPathReconciliation::RootRemoved;
        }

        removed_nodes.sort_by_key(|path| path.components().count());
        let mut subtree_roots = Vec::<PathBuf>::new();
        for node in removed_nodes {
            if !subtree_roots.iter().any(|root| node.starts_with(root)) {
                subtree_roots.push(node);
            }
        }
        let mut hidden_paths = Vec::new();
        for root in subtree_roots {
            let subtree_paths = self.subtree_paths(&root);
            hidden_paths.push(root.clone());
            hidden_paths.extend(self.entry_paths_for_nodes(&subtree_paths));
            self.remove_subtree(&root);
        }
        if self.pending_root.as_ref().is_some_and(|pending| {
            removed_paths
                .iter()
                .any(|removed| pending.path == *removed || pending.path.starts_with(removed))
        }) {
            self.pending_root = None;
        }
        self.pending_children.retain(|parent, pending| {
            !removed_paths.iter().any(|removed| {
                parent.as_path() == removed
                    || parent.starts_with(removed)
                    || pending.path == *removed
                    || pending.path.starts_with(removed)
            })
        });
        IconGridRemovedPathReconciliation::Retained { hidden_paths }
    }

    pub(crate) fn migrate_completed_paths(
        &mut self,
        migrations: &[CompletedPathMigration],
    ) -> IconGridExpansionMigration {
        let original_current_dir = self.context.current_dir.clone();
        let migrated_current_dir =
            path_after_completed_migrations(&original_current_dir, migrations);
        let current_dir_was_migrated = migrated_current_dir != original_current_dir;
        if !current_dir_was_migrated
            && completed_migrations_cross_directory_tree_boundary(&original_current_dir, migrations)
        {
            self.cancel_all_loads();
            return IconGridExpansionMigration::Invalidated;
        }

        self.context.current_dir = migrated_current_dir;
        self.root_path = path_after_completed_migrations(&self.root_path, migrations);
        self.selection_directory =
            path_after_completed_migrations(&self.selection_directory, migrations);
        if let Some(pending_root) = &mut self.pending_root {
            pending_root.parent_directory =
                path_after_completed_migrations(&pending_root.parent_directory, migrations);
            pending_root.path = path_after_completed_migrations(&pending_root.path, migrations);
        }
        let old_pending_children = std::mem::take(&mut self.pending_children);
        self.pending_children = old_pending_children
            .into_iter()
            .map(|(parent, mut pending)| {
                let migrated_parent = path_after_completed_migrations(&parent, migrations);
                pending.parent_directory = migrated_parent.clone();
                pending.path = path_after_completed_migrations(&pending.path, migrations);
                (migrated_parent, pending)
            })
            .collect();

        let old_directories = std::mem::take(&mut self.directories);
        self.directories = old_directories
            .into_iter()
            .map(|(path, mut directory)| {
                directory.cancel_pending_load();
                directory.parent_directory =
                    path_after_completed_migrations(&directory.parent_directory, migrations);
                migrate_directory_entries(&mut directory.contents.entries, migrations);
                (
                    path_after_completed_migrations(&path, migrations),
                    directory,
                )
            })
            .collect();

        if self.directories.contains_key(&self.root_path) && self.directory_tree_invariants_hold() {
            IconGridExpansionMigration::Retained
        } else {
            self.cancel_all_loads();
            IconGridExpansionMigration::Invalidated
        }
    }

    pub(crate) fn cancel_all_loads(&mut self) {
        for directory in self.directories.values_mut() {
            directory.cancel_pending_load();
        }
    }

    fn directory_tree_invariants_hold(&self) -> bool {
        let mut direct_child_parents = HashSet::new();
        let directories_hold = self.directories.iter().all(|(path, directory)| {
            path.parent() == Some(directory.parent_directory.as_path())
                && direct_child_parents.insert(directory.parent_directory.clone())
                && (path == &self.root_path
                    || self.directories.contains_key(&directory.parent_directory))
                && directory
                    .contents
                    .entries
                    .iter()
                    .all(|entry| entry.path.parent() == Some(path.as_path()))
        });
        directories_hold
            && self.pending_children.iter().all(|(parent, pending)| {
                pending.parent_directory == *parent
                    && pending.path.parent() == Some(parent.as_path())
                    && self.directories.contains_key(parent)
                    && !self.directories.contains_key(&pending.path)
            })
    }

    fn directory_is_interactive(&self, directory: &Path) -> bool {
        directory == self.context.current_dir || self.node_is_interactive(directory)
    }

    fn node_is_interactive(&self, path: &Path) -> bool {
        let mut current = path.to_path_buf();
        loop {
            let Some(directory) = self.directories.get(&current) else {
                return false;
            };
            if !directory.is_interactive() {
                return false;
            }
            if current == self.root_path {
                return true;
            }
            current = directory.parent_directory.clone();
        }
    }

    fn subtree_paths(&self, root: &Path) -> HashSet<PathBuf> {
        if !self.directories.contains_key(root) {
            return HashSet::new();
        }
        let mut paths = HashSet::from([root.to_path_buf()]);
        loop {
            let previous_len = paths.len();
            for (path, directory) in &self.directories {
                if paths.contains(&directory.parent_directory) {
                    paths.insert(path.clone());
                }
            }
            if paths.len() == previous_len {
                return paths;
            }
        }
    }

    fn entry_paths_for_nodes(&self, node_paths: &HashSet<PathBuf>) -> Vec<PathBuf> {
        self.directories
            .iter()
            .filter(|(path, _)| node_paths.contains(path.as_path()))
            .flat_map(|(_, directory)| {
                directory
                    .contents
                    .entries
                    .iter()
                    .map(|entry| entry.path.clone())
            })
            .collect()
    }

    fn remove_subtree(&mut self, root: &Path) {
        let paths = self.subtree_paths(root);
        if paths.contains(&self.selection_directory) {
            self.selection_directory = self
                .directories
                .get(root)
                .map(|directory| directory.parent_directory.clone())
                .unwrap_or_else(|| self.context.current_dir.clone());
        }
        self.pending_children
            .retain(|parent, pending| !paths.contains(parent) && !paths.contains(&pending.path));
        for path in paths {
            if let Some(mut directory) = self.directories.remove(&path) {
                directory.cancel_pending_load();
            }
        }
    }
}
