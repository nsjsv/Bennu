use std::collections::HashSet;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use super::*;

fn state_for_names(names: &[&str]) -> BatchRenameState {
    state_for_sources(
        names
            .iter()
            .map(|name| BatchRenameSource {
                path: PathBuf::from("/tmp").join(name),
                source_name_text: (*name).to_owned(),
                modified: None,
            })
            .collect(),
    )
}

fn source_with_modified(name: &str, modified_secs: u64) -> BatchRenameSource {
    BatchRenameSource {
        path: PathBuf::from("/tmp").join(name),
        source_name_text: name.to_owned(),
        modified: Some(UNIX_EPOCH + Duration::from_secs(modified_secs)),
    }
}

fn state_for_sources(items: Vec<BatchRenameSource>) -> BatchRenameState {
    BatchRenameState::new_with_existing_sources(items, HashSet::new()).unwrap()
}

fn add_rule(state: &mut BatchRenameState, kind: BatchRenameRuleKind) -> u64 {
    state.apply_update(BatchRenameMessage::AddRuleSelected(kind));
    state.rules.last().expect("rule appended").id
}

fn rule_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameRuleParams {
    &mut state
        .rules
        .iter_mut()
        .find(|rule| rule.id == id)
        .expect("rule exists")
        .params
}

fn replace_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameReplaceRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Replace(params) => params,
        _ => panic!("expected replace rule"),
    }
}

fn insert_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameInsertRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Insert(params) => params,
        _ => panic!("expected insert rule"),
    }
}

fn slice_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameSliceRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Slice(params) => params,
        _ => panic!("expected slice rule"),
    }
}

fn remove_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameRemoveRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Remove(params) => params,
        _ => panic!("expected remove rule"),
    }
}

fn sequence_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameSequenceRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Sequence(params) => params,
        _ => panic!("expected sequence rule"),
    }
}

fn random_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameRandomRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Random(params) => params,
        _ => panic!("expected random rule"),
    }
}

fn extension_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameExtensionRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Extension(params) => params,
        _ => panic!("expected extension rule"),
    }
}

fn list_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameListRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::List(params) => params,
        _ => panic!("expected list rule"),
    }
}

fn regex_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameRegexRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Regex(params) => params,
        _ => panic!("expected regex rule"),
    }
}

fn template_params_mut(state: &mut BatchRenameState, id: u64) -> &mut BatchRenameTemplateRule {
    match rule_params_mut(state, id) {
        BatchRenameRuleParams::Template(params) => params,
        _ => panic!("expected template rule"),
    }
}

fn preview_names(state: &BatchRenameState) -> Vec<&str> {
    state
        .preview
        .rows
        .iter()
        .map(|row| row.target_name.as_str())
        .collect()
}

fn preview_original_names(state: &BatchRenameState) -> Vec<&str> {
    state
        .preview
        .rows
        .iter()
        .map(|row| row.original_name.as_str())
        .collect()
}

#[cfg(unix)]
#[test]
fn batch_rename_source_rejects_lossy_name_collisions_with_distinct_paths() {
    let first_path = PathBuf::from("/tmp").join(OsString::from_vec(b"entry-\x80".to_vec()));
    let second_path = PathBuf::from("/tmp").join(OsString::from_vec(b"entry-\x81".to_vec()));
    let first_entry = DirectoryEntry::new(
        first_path.clone(),
        file_core::FileKind::File,
        file_core::EntryMetadata::default(),
        false,
        false,
        false,
    );
    let second_entry = DirectoryEntry::new(
        second_path.clone(),
        file_core::FileKind::File,
        file_core::EntryMetadata::default(),
        false,
        false,
        false,
    );
    assert_eq!(
        first_entry.name().to_string_lossy(),
        second_entry.name().to_string_lossy()
    );

    let first_error = BatchRenameSource::try_from_entry(&first_entry).unwrap_err();
    let second_error = BatchRenameSource::try_from_entry(&second_entry).unwrap_err();

    assert_eq!(first_error.path, first_path);
    assert_eq!(second_error.path, second_path);
    assert_ne!(first_error.path, second_error.path);
}

#[cfg(unix)]
#[test]
fn batch_rename_plan_preserves_non_utf8_parent_path() {
    let parent = PathBuf::from("/tmp").join(OsString::from_vec(b"parent-\x80".to_vec()));
    let mut state = state_for_sources(vec![
        BatchRenameSource {
            path: parent.join("first.txt"),
            source_name_text: "first.txt".to_owned(),
            modified: None,
        },
        BatchRenameSource {
            path: parent.join("second.txt"),
            source_name_text: "second.txt".to_owned(),
            modified: None,
        },
    ]);
    let sequence = add_rule(&mut state, BatchRenameRuleKind::Sequence);
    sequence_params_mut(&mut state, sequence).prefix = "renamed ".to_owned();
    state.rebuild_preview();

    let plan = state.plan().expect("valid batch rename plan");
    assert!(state
        .preview
        .rows
        .iter()
        .all(|row| row.target.parent() == Some(parent.as_path())));
    assert!(plan
        .iter()
        .all(|item| item.to.parent() == Some(parent.as_path())));
}

#[test]
fn empty_pipeline_leaves_all_names_unchanged() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["a.txt", "b.txt"]);
    assert!(state
        .preview
        .rows
        .iter()
        .all(|row| row.status == BatchRenamePreviewStatus::Unchanged));
    assert!(!state.can_apply());
}

#[test]
fn batch_rename_sequence_prefixes_number_and_preserves_extension() {
    let mut state = state_for_names(&["report.txt", "notes.txt"]);
    let sequence = add_rule(&mut state, BatchRenameRuleKind::Sequence);
    {
        let params = sequence_params_mut(&mut state, sequence);
        params.prefix = "File ".to_owned();
        params.start_input = "3".to_owned();
        params.padding_input = "3".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["File 003 report.txt", "File 004 notes.txt"]
    );
}

#[test]
fn batch_rename_sequence_respects_step() {
    let mut state = state_for_names(&["report.txt", "notes.txt"]);
    let sequence = add_rule(&mut state, BatchRenameRuleKind::Sequence);
    {
        let params = sequence_params_mut(&mut state, sequence);
        params.prefix = "File ".to_owned();
        params.start_input = "3".to_owned();
        params.step_input = "5".to_owned();
        params.padding_input = "3".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["File 003 report.txt", "File 008 notes.txt"]
    );
}

#[test]
fn batch_rename_replace_insert_slice_and_case_follow_pipeline_order() {
    let mut state = state_for_names(&["summer draft.txt", "winter draft.txt"]);
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "draft".to_owned();
        params.replacement = "photo".to_owned();
    }
    let insert = add_rule(&mut state, BatchRenameRuleKind::Insert);
    {
        let params = insert_params_mut(&mut state, insert);
        params.text = "2026 ".to_owned();
        params.position_input = "0".to_owned();
    }
    let slice = add_rule(&mut state, BatchRenameRuleKind::Slice);
    {
        let params = slice_params_mut(&mut state, slice);
        params.start_input = "0".to_owned();
        params.length_input = "11".to_owned();
    }
    let case = add_rule(&mut state, BatchRenameRuleKind::Case);
    assert!(matches!(
        rule_params_mut(&mut state, case),
        BatchRenameRuleParams::Case(BatchRenameCaseRule::Unchanged)
    ));
    if let BatchRenameRuleParams::Case(case) = rule_params_mut(&mut state, case) {
        *case = BatchRenameCaseRule::Uppercase;
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["2026 SUMMER.txt", "2026 WINTER.txt"]
    );
}

#[test]
fn batch_rename_pipeline_order_swap_changes_result() {
    // 替换 "1"→"9" 与序号规则的先后顺序决定结果是否受影响
    let mut replace_first = state_for_names(&["a.txt", "b.txt"]);
    let replace = add_rule(&mut replace_first, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut replace_first, replace);
        params.find = "1".to_owned();
        params.replacement = "9".to_owned();
    }
    let sequence = add_rule(&mut replace_first, BatchRenameRuleKind::Sequence);
    sequence_params_mut(&mut replace_first, sequence).include_original_stem = false;
    replace_first.rebuild_preview();
    assert_eq!(preview_names(&replace_first), vec!["01.txt", "02.txt"]);

    let mut sequence_first = state_for_names(&["a.txt", "b.txt"]);
    let sequence = add_rule(&mut sequence_first, BatchRenameRuleKind::Sequence);
    sequence_params_mut(&mut sequence_first, sequence).include_original_stem = false;
    let replace = add_rule(&mut sequence_first, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut sequence_first, replace);
        params.find = "1".to_owned();
        params.replacement = "9".to_owned();
    }
    sequence_first.rebuild_preview();
    assert_eq!(preview_names(&sequence_first), vec!["09.txt", "02.txt"]);

    // 在同一状态里用 RuleMoved 交换顺序，结果应与手工构建的相反管道一致
    let mut swapped = state_for_names(&["a.txt", "b.txt"]);
    let replace = add_rule(&mut swapped, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut swapped, replace);
        params.find = "1".to_owned();
        params.replacement = "9".to_owned();
    }
    let sequence = add_rule(&mut swapped, BatchRenameRuleKind::Sequence);
    sequence_params_mut(&mut swapped, sequence).include_original_stem = false;
    swapped.apply_update(BatchRenameMessage::RuleMoved(replace, 1));
    swapped.rebuild_preview();
    assert_eq!(preview_names(&swapped), preview_names(&sequence_first));
}

#[test]
fn batch_rename_disabled_rule_is_skipped_by_pipeline() {
    let mut state = state_for_names(&["draft-a.txt", "draft-b.txt"]);
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "draft".to_owned();
        params.replacement = "final".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["final-a.txt", "final-b.txt"]);

    state.apply_update(BatchRenameMessage::RuleEnabledToggled(replace));
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["draft-a.txt", "draft-b.txt"]);

    state.apply_update(BatchRenameMessage::RuleEnabledToggled(replace));
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["final-a.txt", "final-b.txt"]);
}

#[test]
fn batch_rename_rule_removal_and_selection_followups() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    let first = add_rule(&mut state, BatchRenameRuleKind::Replace);
    let second = add_rule(&mut state, BatchRenameRuleKind::Sequence);

    assert_eq!(state.selected_rule, Some(second));
    state.apply_update(BatchRenameMessage::RuleRemoved(second));
    assert_eq!(state.selected_rule, Some(first));
    assert_eq!(state.rules.len(), 1);

    state.apply_update(BatchRenameMessage::RuleRemoved(first));
    assert_eq!(state.selected_rule, None);
    assert!(state.rules.is_empty());
}

#[test]
fn batch_rename_replace_supports_first_last_range_and_ignore_case() {
    let mut state = state_for_names(&["foo-foo-FOO.txt", "demo.txt"]);
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "foo".to_owned();
        params.replacement = "bar".to_owned();
        params.scope = BatchRenameReplaceScope::First;
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "bar-foo-FOO.txt");

    {
        let params = replace_params_mut(&mut state, replace);
        params.scope = BatchRenameReplaceScope::Last;
        params.ignore_case = true;
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "foo-foo-bar.txt");

    {
        let params = replace_params_mut(&mut state, replace);
        params.scope = BatchRenameReplaceScope::Range;
        params.ignore_case = false;
        params.range_start_input = "4".to_owned();
        params.range_length_input = "7".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "foo-bar-FOO.txt");
}

#[test]
fn batch_rename_per_rule_params_are_isolated() {
    let mut state = state_for_names(&["photo.jpg", "notes.txt"]);
    let first = add_rule(&mut state, BatchRenameRuleKind::Replace);
    let second = add_rule(&mut state, BatchRenameRuleKind::Replace);
    replace_params_mut(&mut state, first).find = "first-find".to_owned();
    replace_params_mut(&mut state, second).find = "second-find".to_owned();

    assert_eq!(replace_params_mut(&mut state, first).find, "first-find");
    assert_eq!(replace_params_mut(&mut state, second).find, "second-find");
}

#[test]
fn batch_rename_insert_supports_after_anchor() {
    let mut state = state_for_names(&["photo_final.txt", "notes.txt"]);
    let insert = add_rule(&mut state, BatchRenameRuleKind::Insert);
    {
        let params = insert_params_mut(&mut state, insert);
        params.mode = BatchRenameInsertMode::AfterAnchor;
        params.anchor = "photo".to_owned();
        params.text = "-2026".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["photo-2026_final.txt", "notes.txt-2026"]
    );
}

#[test]
fn batch_rename_insert_scope_controls_extension_inclusion() {
    let mut state = state_for_names(&["photo.jpg", "notes.txt"]);
    let insert = add_rule(&mut state, BatchRenameRuleKind::Insert);
    {
        let params = insert_params_mut(&mut state, insert);
        params.mode = BatchRenameInsertMode::Position;
        params.position_input = "7".to_owned();
        params.text = "-x".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo.j-xpg");

    insert_params_mut(&mut state, insert).ignore_extension = true;
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo-x.jpg");
}

#[test]
fn batch_rename_insert_scope_applies_to_after_and_anchor_modes() {
    let mut state = state_for_names(&["photo.jpg", "notes.txt"]);
    let insert = add_rule(&mut state, BatchRenameRuleKind::Insert);
    {
        let params = insert_params_mut(&mut state, insert);
        params.mode = BatchRenameInsertMode::After;
        params.text = "-x".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo.jpg-x");

    insert_params_mut(&mut state, insert).ignore_extension = true;
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo-x.jpg");

    {
        let params = insert_params_mut(&mut state, insert);
        params.mode = BatchRenameInsertMode::AfterAnchor;
        params.anchor = "jpg".to_owned();
        params.ignore_extension = false;
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo.jpg-x");

    insert_params_mut(&mut state, insert).ignore_extension = true;
    state.rebuild_preview();
    assert_eq!(preview_names(&state)[0], "photo-x.jpg");
}

#[test]
fn batch_rename_extension_rule_runs_after_full_name_insert() {
    let mut state = state_for_names(&["photo.jpg", "notes.txt"]);
    let insert = add_rule(&mut state, BatchRenameRuleKind::Insert);
    {
        let params = insert_params_mut(&mut state, insert);
        params.mode = BatchRenameInsertMode::Position;
        params.position_input = "7".to_owned();
        params.text = "-x".to_owned();
    }
    let extension = add_rule(&mut state, BatchRenameRuleKind::Extension);
    extension_params_mut(&mut state, extension).mode = BatchRenameExtensionMode::Uppercase;
    state.rebuild_preview();

    assert_eq!(preview_names(&state)[0], "photo.J-XPG");
}

#[test]
fn batch_rename_slice_supports_after_anchor() {
    let mut state = state_for_names(&["photo_final.txt", "notes.txt"]);
    let slice = add_rule(&mut state, BatchRenameRuleKind::Slice);
    {
        let params = slice_params_mut(&mut state, slice);
        params.mode = BatchRenameSliceMode::AfterAnchor;
        params.anchor = "photo_".to_owned();
        params.length_input = "5".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["final.txt", ".txt"]);
}

#[test]
fn batch_rename_case_invert_flips_stem_letter_case() {
    let mut state = state_for_names(&["AbC.txt", "xYz.txt"]);
    let case = add_rule(&mut state, BatchRenameRuleKind::Case);
    if let BatchRenameRuleParams::Case(value) = rule_params_mut(&mut state, case) {
        *value = BatchRenameCaseRule::InvertCase;
    }
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["aBc.txt", "XyZ.txt"]);
}

#[test]
fn batch_rename_sort_changes_sequence_order() {
    let mut state = state_for_names(&["b.txt", "a.txt"]);
    state.sort.mode = BatchRenameSortMode::NameAscending;
    let sequence = add_rule(&mut state, BatchRenameRuleKind::Sequence);
    {
        let params = sequence_params_mut(&mut state, sequence);
        params.prefix = "Item ".to_owned();
        params.include_original_stem = false;
    }
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["Item 01.txt", "Item 02.txt"]);
    assert_eq!(state.preview.rows[0].original_name, "a.txt");
}

#[test]
fn batch_rename_natural_sort_orders_embedded_numbers() {
    let mut state = state_for_names(&["file10.txt", "file2.txt", "file1.txt"]);
    state.sort.mode = BatchRenameSortMode::NaturalAscending;
    state.rebuild_preview();

    assert_eq!(
        preview_original_names(&state),
        vec!["file1.txt", "file2.txt", "file10.txt"]
    );
}

#[test]
fn batch_rename_modified_sort_uses_source_metadata() {
    let mut state = state_for_sources(vec![
        source_with_modified("middle.txt", 20),
        source_with_modified("oldest.txt", 10),
        source_with_modified("newest.txt", 30),
    ]);
    state.sort.mode = BatchRenameSortMode::ModifiedAscending;
    state.rebuild_preview();

    assert_eq!(
        preview_original_names(&state),
        vec!["oldest.txt", "middle.txt", "newest.txt"]
    );

    state.sort.mode = BatchRenameSortMode::ModifiedDescending;
    state.rebuild_preview();

    assert_eq!(
        preview_original_names(&state),
        vec!["newest.txt", "middle.txt", "oldest.txt"]
    );
}

#[test]
fn batch_rename_random_sort_is_deterministic() {
    let mut state = state_for_sources(vec![
        source_with_modified("alpha.txt", 10),
        source_with_modified("beta.txt", 20),
        source_with_modified("gamma.txt", 30),
    ]);
    state.sort.mode = BatchRenameSortMode::Random;
    state.rebuild_preview();
    let first_order = preview_original_names(&state)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    state.rebuild_preview();

    assert_eq!(preview_original_names(&state), first_order);
    assert_ne!(first_order, vec!["alpha.txt", "beta.txt", "gamma.txt"]);
}

#[test]
fn batch_rename_preview_drag_reorders_selection_order() {
    let mut state = state_for_names(&["b.txt", "a.txt", "c.txt"]);
    state.sort.mode = BatchRenameSortMode::NameAscending;
    state.rebuild_preview();

    assert_eq!(
        preview_original_names(&state),
        vec!["a.txt", "b.txt", "c.txt"]
    );

    let source = PathBuf::from("/tmp/a.txt");
    state.apply_update(BatchRenameMessage::PreviewDragStarted(source.clone()));
    state.apply_update(BatchRenameMessage::PreviewDragEntered(PathBuf::from(
        "/tmp/c.txt",
    )));
    state.rebuild_preview();

    assert_eq!(state.sort.mode, BatchRenameSortMode::SelectionOrder);
    assert_eq!(
        preview_original_names(&state),
        vec!["b.txt", "c.txt", "a.txt"]
    );
    assert_eq!(state.dragging_preview_source(), Some(source.as_path()));

    state.apply_update(BatchRenameMessage::PreviewDragFinished);
    assert_eq!(state.dragging_preview_source(), None);
}

#[test]
fn batch_rename_preview_name_edit_overrides_single_target() {
    let mut state = state_for_names(&["alpha.txt", "beta.txt"]);
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    let source = PathBuf::from("/tmp/beta.txt");

    state.apply_update(BatchRenameMessage::PreviewNameEditStarted(source.clone()));
    assert_eq!(state.editing_target_name_source(), Some(source.as_path()));
    assert_eq!(state.editing_target_name_input(), "beta.txt");

    state.apply_update(BatchRenameMessage::PreviewNameChanged(
        "chosen.md".to_owned(),
    ));
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["alpha.txt", "chosen.md"]);

    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "alpha".to_owned();
        params.replacement = "renamed".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["renamed.txt", "chosen.md"]);
    let plan = state.plan().expect("manual edit should produce a plan");
    assert_eq!(plan[1].from, source);
    assert_eq!(plan[1].to, PathBuf::from("/tmp/chosen.md"));

    state.apply_update(BatchRenameMessage::PreviewNameEditCommitted);
    assert_eq!(state.editing_target_name_source(), None);
}

#[test]
fn batch_rename_extension_modes_transform_extension() {
    let mut state = state_for_names(&["photo.JPEG", "archive.tar"]);
    let extension = add_rule(&mut state, BatchRenameRuleKind::Extension);
    {
        let params = extension_params_mut(&mut state, extension);
        params.mode = BatchRenameExtensionMode::Replace;
        params.replacement = ".bak".to_owned();
    }
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["photo.bak", "archive.bak"]);

    extension_params_mut(&mut state, extension).mode = BatchRenameExtensionMode::Remove;
    state.rebuild_preview();
    assert_eq!(preview_names(&state), vec!["photo", "archive"]);
}

#[test]
fn batch_rename_random_is_deterministic() {
    let mut state = state_for_names(&["photo.jpg", "photo.jpg"]);
    let random = add_rule(&mut state, BatchRenameRuleKind::Random);
    {
        let params = random_params_mut(&mut state, random);
        params.mode = BatchRenameRandomMode::Suffix;
        params.length_input = "4".to_owned();
        params.alphabet = "ab".to_owned();
    }
    state.rebuild_preview();
    let first_preview = preview_names(&state)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    state.rebuild_preview();
    assert_eq!(preview_names(&state), first_preview);
    assert!(first_preview.iter().all(|name| name.ends_with(".jpg")));
}

#[test]
fn batch_rename_remove_text_and_range() {
    let mut state = state_for_names(&["draft-2026-final.txt", "draft-2027-final.txt"]);
    let remove = add_rule(&mut state, BatchRenameRuleKind::Remove);
    {
        let params = remove_params_mut(&mut state, remove);
        params.text = "draft-".to_owned();
        params.start_input = "4".to_owned();
        params.length_input = "1".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["2026final.txt", "2027final.txt"]
    );
}

#[test]
fn batch_rename_remove_character_classes() {
    let mut state = state_for_names(&["Ab 中-12(测).txt", "xY 9!.txt"]);
    let remove = add_rule(&mut state, BatchRenameRuleKind::Remove);
    {
        let params = remove_params_mut(&mut state, remove);
        params.mode = BatchRenameRemoveMode::CharacterClasses;
        params.classes = vec![
            BatchRenameRemoveClass::Uppercase,
            BatchRenameRemoveClass::Digits,
            BatchRenameRemoveClass::Symbols,
            BatchRenameRemoveClass::Brackets,
            BatchRenameRemoveClass::Whitespace,
            BatchRenameRemoveClass::Hanzi,
        ];
    }
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["b.txt", "x.txt"]);
}

#[test]
fn batch_rename_list_overrides_target_names_by_order() {
    let mut state = state_for_names(&["b.txt", "a.txt"]);
    state.sort.mode = BatchRenameSortMode::NameAscending;
    let list = add_rule(&mut state, BatchRenameRuleKind::List);
    list_params_mut(&mut state, list).names = "first.md|second.md".to_owned();
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["first.md", "second.md"]);
}

#[test]
fn batch_rename_template_numbering_is_independent_with_padding() {
    let mut state = state_for_names(&["report.txt", "notes.md"]);
    let template = add_rule(&mut state, BatchRenameRuleKind::Template);
    template_params_mut(&mut state, template).template = "{n3}-{original_stem}.{ext}".to_owned();
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["001-report.txt", "002-notes.md"]
    );
}

#[test]
fn batch_rename_template_applies_to_pipeline_output() {
    let mut state = state_for_names(&["draft-a.txt", "draft-b.txt"]);
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "draft".to_owned();
        params.replacement = "final".to_owned();
    }
    let template = add_rule(&mut state, BatchRenameRuleKind::Template);
    template_params_mut(&mut state, template).template = "[{stem}]".to_owned();
    state.rebuild_preview();

    // 模板的 {stem} 引用上一条规则的输出（final-a），而非原始名
    assert_eq!(preview_names(&state), vec!["[final-a]", "[final-b]"]);
}

#[test]
fn batch_rename_template_accepts_localized_labels() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    let template = add_rule(&mut state, BatchRenameRuleKind::Template);
    template_params_mut(&mut state, template).template = format!(
        "{}{}{}",
        BatchRenameTemplateToken::OriginalNameWithoutExtension.localized_labels()[1],
        BatchRenameTemplateToken::Number001.localized_labels()[1],
        BatchRenameTemplateToken::OriginalExtension.localized_labels()[1],
    );
    state.rebuild_preview();

    assert_eq!(preview_names(&state), vec!["a001.txt", "b002.txt"]);
    assert!(state.can_apply());
}

#[test]
fn batch_rename_regex_replaces_whole_target_name() {
    let mut state = state_for_names(&["IMG_0001.JPG", "IMG_0002.JPG"]);
    let regex = add_rule(&mut state, BatchRenameRuleKind::Regex);
    {
        let params = regex_params_mut(&mut state, regex);
        params.pattern = r"IMG_(\d+)".to_owned();
        params.replacement = "photo-$1".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        preview_names(&state),
        vec!["photo-0001.JPG", "photo-0002.JPG"]
    );
}

#[test]
fn batch_rename_invalid_regex_blocks_apply() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    let regex = add_rule(&mut state, BatchRenameRuleKind::Regex);
    regex_params_mut(&mut state, regex).pattern = "(".to_owned();
    state.rebuild_preview();

    assert!(state
        .preview
        .rows
        .iter()
        .all(|row| row.status == BatchRenamePreviewStatus::RuleError));
    assert!(!state.can_apply());
}

#[test]
fn batch_rename_preview_marks_duplicate_targets() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    let slice = add_rule(&mut state, BatchRenameRuleKind::Slice);
    slice_params_mut(&mut state, slice).start_input = "99".to_owned();
    state.rebuild_preview();

    assert!(state
        .preview
        .rows
        .iter()
        .all(|row| row.status == BatchRenamePreviewStatus::DuplicateTarget));
    assert!(!state.can_apply());
}

#[test]
fn batch_rename_preview_marks_existing_unselected_target() {
    let existing = [PathBuf::from("/tmp/taken.txt")]
        .into_iter()
        .collect::<HashSet<_>>();
    let mut state = BatchRenameState::new_with_existing_sources(
        vec![
            BatchRenameSource {
                path: PathBuf::from("/tmp/report.txt"),
                source_name_text: "report.txt".to_owned(),
                modified: None,
            },
            BatchRenameSource {
                path: PathBuf::from("/tmp/notes.txt"),
                source_name_text: "notes.txt".to_owned(),
                modified: None,
            },
        ],
        existing,
    )
    .unwrap();
    let replace = add_rule(&mut state, BatchRenameRuleKind::Replace);
    {
        let params = replace_params_mut(&mut state, replace);
        params.find = "report".to_owned();
        params.replacement = "taken".to_owned();
    }
    state.rebuild_preview();

    assert_eq!(
        state.preview.rows[0].status,
        BatchRenamePreviewStatus::ExistingTarget
    );
    assert!(!state.can_apply());
}

#[test]
fn token_selection_appends_label_and_closes_menu() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    let template = add_rule(&mut state, BatchRenameRuleKind::Template);
    template_params_mut(&mut state, template).token_menu_open = true;

    state.apply_update(BatchRenameMessage::TemplateTokenSelected(
        template,
        BatchRenameTemplateToken::Number01,
    ));

    assert_eq!(
        template_params_mut(&mut state, template).template,
        format!(
            "{}{}",
            BatchRenameTemplateToken::OriginalName.label(),
            BatchRenameTemplateToken::Number01.label(),
        )
    );
    assert!(!template_params_mut(&mut state, template).token_menu_open);
}

#[test]
fn add_rule_menu_closes_after_selection() {
    let mut state = state_for_names(&["a.txt", "b.txt"]);
    state.apply_update(BatchRenameMessage::AddRuleMenuToggled);
    assert!(state.add_rule_menu_open);

    state.apply_update(BatchRenameMessage::AddRuleSelected(
        BatchRenameRuleKind::Replace,
    ));
    assert!(!state.add_rule_menu_open);
    assert_eq!(state.rules.len(), 1);
}
