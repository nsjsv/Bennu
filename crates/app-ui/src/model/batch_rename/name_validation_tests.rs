use std::collections::HashSet;
use std::path::PathBuf;

use super::*;

fn state_with_manual_name(target_name: &str) -> BatchRenameState {
    state_with_manual_name_among(target_name, HashSet::new())
}

fn state_with_manual_name_among(
    target_name: &str,
    existing_paths: HashSet<PathBuf>,
) -> BatchRenameState {
    let sources = ["alpha.txt", "beta.txt"]
        .iter()
        .map(|name| BatchRenameSource {
            path: PathBuf::from("/tmp").join(name),
            source_name_text: (*name).to_owned(),
            modified: None,
        })
        .collect();
    let mut state = BatchRenameState::new_with_existing_sources(sources, existing_paths).unwrap();
    state.apply_update(BatchRenameMessage::PreviewNameEditStarted(PathBuf::from(
        "/tmp/beta.txt",
    )));
    state.apply_update(BatchRenameMessage::PreviewNameChanged(
        target_name.to_owned(),
    ));
    state.rebuild_preview();
    state
}

#[test]
fn names_the_executor_would_reject_block_apply() {
    for (target_name, reason) in [
        ("", InvalidEntryName::Empty),
        (".", InvalidEntryName::CurrentDirectory),
        ("..", InvalidEntryName::ParentDirectory),
        ("archive/beta.txt", InvalidEntryName::ContainsSlash),
        ("beta\0.txt", InvalidEntryName::ContainsNul),
    ] {
        let state = state_with_manual_name(target_name);

        assert_eq!(
            state.preview.rows[1].status,
            BatchRenamePreviewStatus::InvalidName(reason),
            "{target_name:?}"
        );
        assert!(!state.can_apply(), "{target_name:?}");
        assert_eq!(state.plan(), None, "{target_name:?}");
    }
}

#[test]
fn a_name_with_a_slash_is_reported_as_invalid_even_when_the_joined_path_exists() {
    // "sub/x" 拼出的路径落在子目录里，恰好已存在；名字本身的问题优先于已存在判定。
    let existing = HashSet::from([PathBuf::from("/tmp/sub/x")]);
    let state = state_with_manual_name_among("sub/x", existing);

    assert_eq!(
        state.preview.rows[1].status,
        BatchRenamePreviewStatus::InvalidName(InvalidEntryName::ContainsSlash)
    );
}

#[test]
fn a_legal_manual_name_stays_ready() {
    let state = state_with_manual_name("gamma.txt");

    assert_eq!(
        state.preview.rows[1].status,
        BatchRenamePreviewStatus::Ready
    );
    assert!(state.can_apply());
}
