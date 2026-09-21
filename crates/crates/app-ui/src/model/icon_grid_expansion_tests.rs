use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::operation_history::CompletedPathMigration;

fn entry(path: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn loaded(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 1,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn context() -> IconGridExpansionContext {
    IconGridExpansionContext {
        pane_id: BrowserPaneId(9),
        current_dir: PathBuf::from("/workspace"),
        session_id: IconGridExpansionSessionId::new(17),
    }
}

fn anchor(parent: &str, path: &str, index: usize) -> IconGridExpansionAnchor {
    IconGridExpansionAnchor {
        parent_directory: PathBuf::from(parent),
        path: PathBuf::from(path),
        index,
    }
}

fn state_with_root() -> IconGridExpansionState {
    IconGridExpansionState::new(
        context(),
        anchor("/workspace", "/workspace/root", 0),
        loaded(vec![
            entry("/workspace/root/alpha", FileKind::Directory),
            entry("/workspace/root/beta", FileKind::Directory),
        ]),
    )
}

#[test]
fn follow_plan_advances_only_through_loaded_direct_directories() {
    let target = PathBuf::from("/workspace/root/alpha/report.txt");
    let mut state = IconGridExpansionState::following_directory_chain(
        context(),
        anchor("/workspace", "/workspace/root", 0),
        loaded(vec![entry("/workspace/root/alpha", FileKind::Directory)]),
        vec![PathBuf::from("/workspace/root/alpha")],
        PathBuf::from("/workspace/root/alpha"),
        target.clone(),
    );

    let IconGridExpansionFollowAdvance::StartChild(next) = state.advance_follow_plan() else {
        panic!("loaded root must yield the next direct directory");
    };
    assert_eq!(next.path, Path::new("/workspace/root/alpha"));
    assert!(state.insert_directory(
        next,
        loaded(vec![entry(
            "/workspace/root/alpha/report.txt",
            FileKind::File,
        )]),
    ));
    assert_eq!(
        state.advance_follow_plan(),
        IconGridExpansionFollowAdvance::RestoreSelection(target)
    );
    assert!(!state.has_follow_plan());
}

#[test]
fn follow_plan_rejects_missing_or_non_directory_next_component() {
    let mut state = IconGridExpansionState::following_directory_chain(
        context(),
        anchor("/workspace", "/workspace/root", 0),
        loaded(vec![entry("/workspace/root/alpha", FileKind::File)]),
        vec![PathBuf::from("/workspace/root/alpha")],
        PathBuf::from("/workspace/root/alpha"),
        PathBuf::from("/workspace/root/alpha/report.txt"),
    );

    assert_eq!(
        state.advance_follow_plan(),
        IconGridExpansionFollowAdvance::Invalid
    );
    assert!(!state.has_follow_plan());
}

#[test]
fn interactive_selection_chain_uses_only_the_selected_branch() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(vec![entry(
            "/workspace/root/alpha/report.txt",
            FileKind::File,
        )]),
    ));

    assert_eq!(
        state.interactive_expansion_chain_for_selection(Path::new(
            "/workspace/root/alpha/report.txt"
        )),
        Some(vec![
            PathBuf::from("/workspace/root"),
            PathBuf::from("/workspace/root/alpha"),
        ])
    );
    assert_eq!(
        state.interactive_expansion_chain_for_selection(Path::new("/workspace/root/beta")),
        Some(vec![PathBuf::from("/workspace/root")])
    );
}

#[test]
fn manual_dismissal_cancels_follow_plan() {
    let mut state = IconGridExpansionState::following_directory_chain(
        context(),
        anchor("/workspace", "/workspace/root", 0),
        loaded(Vec::new()),
        Vec::new(),
        PathBuf::from("/workspace/root"),
        PathBuf::from("/workspace/root"),
    );

    state.begin_root_dismissal();

    assert!(!state.has_follow_plan());
}

#[test]
fn one_parent_rejects_multiple_direct_branches() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(vec![entry("/workspace/root/alpha/a.txt", FileKind::File)]),
    ));
    assert!(!state.insert_directory(
        anchor("/workspace/root", "/workspace/root/beta", 1),
        loaded(vec![entry("/workspace/root/beta/b.txt", FileKind::File)]),
    ));

    assert_eq!(state.directory_count(), 2);
    assert!(state.contains_tree_path(Path::new("/workspace/root/alpha/a.txt")));
    assert!(state.directory(Path::new("/workspace/root/beta")).is_none());
}

#[test]
fn child_switch_cancels_old_subtree_and_yields_pending_child_after_close() {
    let mut state = state_with_root();
    let alpha_cancel = CancellationToken::new();
    let nested_cancel = CancellationToken::new();
    let mut alpha = loaded(vec![
        entry("/workspace/root/alpha/nested", FileKind::Directory),
        entry("/workspace/root/alpha/a.txt", FileKind::File),
    ]);
    alpha.load_cancel = Some(alpha_cancel.clone());
    assert!(state.insert_directory(anchor("/workspace/root", "/workspace/root/alpha", 0), alpha,));
    let mut nested = loaded(vec![entry(
        "/workspace/root/alpha/nested/deep.txt",
        FileKind::File,
    )]);
    nested.load_cancel = Some(nested_cancel.clone());
    assert!(state.insert_directory(
        anchor("/workspace/root/alpha", "/workspace/root/alpha/nested", 0,),
        nested,
    ));

    let child_switch =
        state.begin_child_switch(anchor("/workspace/root", "/workspace/root/beta", 1));

    assert_eq!(
        child_switch.closing_path.as_deref(),
        Some(Path::new("/workspace/root/alpha"))
    );
    assert!(child_switch.ready_child.is_none());
    assert!(alpha_cancel.is_cancelled());
    assert!(nested_cancel.is_cancelled());
    assert!(child_switch
        .hidden_paths
        .contains(&PathBuf::from("/workspace/root/alpha/a.txt")));
    assert!(child_switch
        .hidden_paths
        .contains(&PathBuf::from("/workspace/root/alpha/nested/deep.txt")));
    assert_eq!(
        state
            .pending_child(Path::new("/workspace/root"))
            .map(|pending| pending.path.as_path()),
        Some(Path::new("/workspace/root/beta"))
    );
    assert!(state.directory(Path::new("/workspace/root/beta")).is_none());

    let advance = state.advance_animations(1.0);
    assert_eq!(
        advance
            .ready_children
            .iter()
            .map(|pending| pending.path.as_path())
            .collect::<Vec<_>>(),
        vec![Path::new("/workspace/root/beta")]
    );
    assert!(state
        .directory(Path::new("/workspace/root/alpha"))
        .is_none());
    assert!(state.directory(Path::new("/workspace/root/beta")).is_none());

    assert!(state.insert_directory(
        advance.ready_children.into_iter().next().unwrap(),
        loaded(vec![entry("/workspace/root/beta/b.txt", FileKind::File)]),
    ));
    assert_eq!(state.directory_count(), 2);
    assert!(state.contains_tree_path(Path::new("/workspace/root/beta/b.txt")));
}

#[test]
fn reopening_closing_child_cancels_pending_sibling() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(Vec::new()),
    ));
    let child_switch =
        state.begin_child_switch(anchor("/workspace/root", "/workspace/root/beta", 1));
    assert!(child_switch.ready_child.is_none());

    assert!(state.reopen_directory(Path::new("/workspace/root/alpha")));

    assert!(state.pending_child(Path::new("/workspace/root")).is_none());
    assert!(state
        .directory(Path::new("/workspace/root/alpha"))
        .is_some_and(|directory| directory.contents.is_expanded));
    assert!(!state.animation_is_active());
}

#[test]
fn latest_sibling_click_replaces_pending_child_intent() {
    let mut state = state_with_root();
    state
        .directory_mut(Path::new("/workspace/root"))
        .unwrap()
        .contents
        .entries
        .push(entry("/workspace/root/gamma", FileKind::Directory));
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(Vec::new()),
    ));
    state.begin_child_switch(anchor("/workspace/root", "/workspace/root/beta", 1));

    state.begin_child_switch(anchor("/workspace/root", "/workspace/root/gamma", 2));

    assert_eq!(
        state
            .pending_child(Path::new("/workspace/root"))
            .map(|pending| pending.path.as_path()),
        Some(Path::new("/workspace/root/gamma"))
    );
    let advance = state.advance_animations(1.0);
    assert_eq!(advance.ready_children.len(), 1);
    assert_eq!(
        advance.ready_children[0].path,
        Path::new("/workspace/root/gamma")
    );
}

#[test]
fn pending_child_migration_must_preserve_direct_parent_identity() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(Vec::new()),
    ));
    state.begin_child_switch(anchor("/workspace/root", "/workspace/root/beta", 1));
    let migrations = [CompletedPathMigration::new(
        PathBuf::from("/workspace/root/beta"),
        PathBuf::from("/workspace/other/beta"),
    )];

    assert_eq!(
        state.migrate_completed_paths(&migrations),
        IconGridExpansionMigration::Invalidated,
    );
}

#[test]
fn replacement_waits_for_root_close() {
    let mut state = state_with_root();
    let next_root = anchor("/workspace", "/workspace/next", 4);

    state.begin_root_replacement(next_root.clone());
    assert_eq!(state.pending_root(), Some(&next_root));
    assert!(!state.root_is_closed());

    let advance = state.advance_animations(1.0);
    assert!(advance.changed);
    assert!(advance.root_closed);
    assert!(state.root_is_closed());
    assert_eq!(state.take_pending_root(), Some(next_root));
}

#[test]
fn context_identity_rejects_each_stale_dimension() {
    let state = state_with_root();
    assert!(state.matches_context(
        BrowserPaneId(9),
        Path::new("/workspace"),
        IconGridExpansionSessionId::new(17),
    ));
    assert!(!state.matches_context(
        BrowserPaneId(10),
        Path::new("/workspace"),
        IconGridExpansionSessionId::new(17),
    ));
    assert!(!state.matches_context(
        BrowserPaneId(9),
        Path::new("/other"),
        IconGridExpansionSessionId::new(17),
    ));
    assert!(!state.matches_context(
        BrowserPaneId(9),
        Path::new("/workspace"),
        IconGridExpansionSessionId::new(18),
    ));
}

#[test]
fn parent_reload_removes_closing_child_and_preserves_pending_sibling() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(vec![entry("/workspace/root/alpha/a.txt", FileKind::File)]),
    ));
    state.set_selection_directory(Path::new("/workspace/root/alpha"));
    let child_switch =
        state.begin_child_switch(anchor("/workspace/root", "/workspace/root/beta", 1));
    assert!(child_switch.ready_child.is_none());

    let reconciliation = state.reconcile_child_anchors(
        Path::new("/workspace/root"),
        &[entry("/workspace/root/beta", FileKind::Directory)],
    );

    assert_eq!(reconciliation, IconGridAnchorReconciliation::Retained);
    assert!(state
        .directory(Path::new("/workspace/root/alpha"))
        .is_none());
    assert_eq!(state.selection_directory(), Path::new("/workspace/root"));
    assert_eq!(
        state
            .pending_child(Path::new("/workspace/root"))
            .map(|pending| (pending.path.as_path(), pending.index)),
        Some((Path::new("/workspace/root/beta"), 0))
    );

    let advance = state.advance_animations(0.0);
    assert_eq!(advance.ready_children.len(), 1);
    assert_eq!(
        advance.ready_children[0].path,
        Path::new("/workspace/root/beta")
    );
}

#[test]
fn pending_root_anchor_realigns_after_parent_reload() {
    let mut state = state_with_root();
    let pending = anchor("/workspace", "/workspace/next", 5);
    state.begin_root_replacement(pending);

    assert_eq!(
        state.reconcile_child_anchors(
            Path::new("/workspace"),
            &[
                entry("/workspace/root", FileKind::Directory),
                entry("/workspace/next", FileKind::Directory),
            ],
        ),
        IconGridAnchorReconciliation::Retained,
    );
    assert_eq!(state.pending_root().map(|anchor| anchor.index), Some(1));
}

#[test]
fn root_reload_reports_removed_root() {
    let mut state = state_with_root();
    assert_eq!(
        state.reconcile_child_anchors(Path::new("/workspace"), &[]),
        IconGridAnchorReconciliation::RootRemoved,
    );
}

#[test]
fn in_tree_path_migration_preserves_expansion_identity() {
    let mut state = state_with_root();
    let migrations = [CompletedPathMigration::new(
        PathBuf::from("/workspace/root"),
        PathBuf::from("/workspace/renamed"),
    )];

    assert_eq!(
        state.migrate_completed_paths(&migrations),
        IconGridExpansionMigration::Retained,
    );
    assert_eq!(state.root_path(), Path::new("/workspace/renamed"));
    assert!(state.directory(Path::new("/workspace/renamed")).is_some());
}

#[test]
fn successful_removal_prunes_affected_branch_and_preserves_root() {
    let mut state = state_with_root();
    assert!(state.insert_directory(
        anchor("/workspace/root", "/workspace/root/alpha", 0),
        loaded(vec![entry(
            "/workspace/root/alpha/report.txt",
            FileKind::File,
        )]),
    ));

    let reconciliation = state.reconcile_removed_paths(&[PathBuf::from("/workspace/root/alpha")]);

    let IconGridRemovedPathReconciliation::Retained { hidden_paths } = reconciliation else {
        panic!("nested removal must preserve the root tree");
    };
    assert!(hidden_paths.contains(&PathBuf::from("/workspace/root/alpha")));
    assert!(hidden_paths.contains(&PathBuf::from("/workspace/root/alpha/report.txt")));
    assert!(state
        .directory(Path::new("/workspace/root/alpha"))
        .is_none());
    assert!(state.directory(Path::new("/workspace/root")).is_some());
}

#[test]
fn cross_panel_path_migration_invalidates_expansion() {
    let mut state = state_with_root();
    let migrations = [CompletedPathMigration::new(
        PathBuf::from("/workspace/root/alpha"),
        PathBuf::from("/workspace/other/alpha"),
    )];

    assert_eq!(
        state.migrate_completed_paths(&migrations),
        IconGridExpansionMigration::Invalidated,
    );
}

#[test]
fn cross_tree_path_migration_invalidates_expansion() {
    let mut state = state_with_root();
    let migrations = [CompletedPathMigration::new(
        PathBuf::from("/workspace/root"),
        PathBuf::from("/archive/root"),
    )];

    assert_eq!(
        state.migrate_completed_paths(&migrations),
        IconGridExpansionMigration::Invalidated,
    );
}
