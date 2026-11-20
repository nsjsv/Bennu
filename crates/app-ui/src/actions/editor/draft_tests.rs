use std::path::Path;

use super::*;
use crate::actions::action_file::parse_action_file;
use crate::actions::ActionId;

fn types() -> std::sync::Arc<MimeDatabase> {
    MimeDatabase::system()
}

fn definition(text: &str) -> ActionDefinition {
    parse_action_file(
        ActionId::from_file_stem("a"),
        Path::new("/a.toml"),
        text,
        &types(),
    )
    .unwrap()
}

fn draft(text: &str) -> ActionDraft {
    ActionDraft::from_definition(&definition(text), &types())
}

#[test]
fn categories_are_recognised_in_the_type_list_and_the_rest_stays_as_text() {
    let draft = draft(
        "name = \"x\"\ntypes = [\"video/*\", \"inode/directory\", \"image/png\"]\nsteps = [{ copy_paths = true }]\n",
    );
    assert!(draft.types.contains(TypeCategory::Video));
    assert!(draft.types.contains(TypeCategory::Folders));
    assert!(!draft.types.contains(TypeCategory::Images));
    assert_eq!(draft.types.other().text(), "image/png");
    assert_eq!(
        draft.types.texts(&types()).unwrap(),
        ["inode/directory", "video/*", "image/png"]
    );
}

#[test]
fn a_category_written_out_reads_back_as_that_category() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    for category in TypeCategory::ALL {
        draft.apply(DraftEdit::Category(category, true), &types);
    }
    let texts = draft.types.texts(&types).unwrap();
    let filter = TypeFilter::Patterns(
        texts
            .iter()
            .map(|text| TypePattern::parse(text, &types).unwrap())
            .collect(),
    );
    let read_back = TypeChoice::from_filter(&filter, &types);
    for category in TypeCategory::ALL {
        assert!(read_back.contains(category), "{category:?}");
    }
    assert_eq!(read_back.other().text(), "");
}

#[test]
fn nothing_chosen_means_everything() {
    let types = types();
    let draft = ActionDraft::empty(&types);
    assert!(draft.types.texts(&types).unwrap().is_empty());
}

#[test]
fn other_types_are_checked_with_the_same_words_as_the_file() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    draft.apply(DraftEdit::OtherTypes("image/png videos".to_owned()), &types);
    assert_eq!(
        draft.types.other().mistake(),
        Some(parse_type_pattern("videos", &types).unwrap_err().as_str())
    );
    assert!(draft.types.texts(&types).is_none());
}

#[test]
fn switching_between_copy_and_move_keeps_the_folder() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    draft.apply(DraftEdit::StepAdded, &types);
    draft.apply(DraftEdit::StepKind(0, StepKind::CopyTo), &types);
    draft.apply(DraftEdit::StepText(0, "~/备份".to_owned()), &types);
    draft.apply(DraftEdit::StepKind(0, StepKind::MoveTo), &types);
    match &draft.steps[0] {
        StepDraft::MoveTo(folder) => {
            assert_eq!(folder.text(), "~/备份");
            assert!(folder.value().is_some());
        }
        other => panic!("{other:?}"),
    }
    draft.apply(DraftEdit::StepKind(0, StepKind::Run), &types);
    assert!(matches!(&draft.steps[0], StepDraft::Run(command) if command.text().is_empty()));
}

#[test]
fn a_chosen_option_turns_the_step_into_its_kind() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    draft.apply(DraftEdit::StepAdded, &types);
    draft.apply(
        DraftEdit::StepOption(0, StepOption::Archive(ArchiveFormat::SevenZip)),
        &types,
    );
    assert_eq!(draft.steps, [StepDraft::Compress(ArchiveFormat::SevenZip)]);
}

#[test]
fn moving_and_removing_steps() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    for _ in 0..3 {
        draft.apply(DraftEdit::StepAdded, &types);
    }
    draft.apply(DraftEdit::StepKind(0, StepKind::Compress), &types);
    draft.apply(DraftEdit::StepKind(2, StepKind::Extract), &types);
    draft.apply(DraftEdit::StepMovedDown(0), &types);
    draft.apply(DraftEdit::StepMovedUp(2), &types);
    let kinds = |draft: &ActionDraft| draft.steps.iter().map(StepDraft::kind).collect::<Vec<_>>();
    assert_eq!(
        kinds(&draft),
        [StepKind::CopyPaths, StepKind::Extract, StepKind::Compress]
    );
    // 已经在两头的、下标不存在的（连点两下）都不动。
    draft.apply(DraftEdit::StepMovedUp(0), &types);
    draft.apply(DraftEdit::StepMovedDown(2), &types);
    draft.apply(DraftEdit::StepRemoved(1), &types);
    draft.apply(DraftEdit::StepRemoved(5), &types);
    assert_eq!(kinds(&draft), [StepKind::CopyPaths, StepKind::Compress]);
}

#[test]
fn structure_mistakes_block_saving() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    assert!(draft.name_mistake().is_some());
    assert!(draft.steps_mistake().is_some());
    assert!(!draft.is_complete());

    draft.apply(DraftEdit::Name(" 清理 ".to_owned()), &types);
    draft.apply(DraftEdit::StepAdded, &types);
    draft.apply(DraftEdit::StepKind(0, StepKind::Trash), &types);
    draft.apply(DraftEdit::StepAdded, &types);
    assert!(draft.step_mistake(0).is_some());
    assert!(!draft.is_complete());

    draft.apply(DraftEdit::StepMovedDown(0), &types);
    assert_eq!(draft.step_mistake(1), None);
    assert!(draft.is_complete());

    draft.apply(DraftEdit::StepKind(0, StepKind::Run), &types);
    assert!(draft.step_mistake(0).is_some(), "an empty command");
    assert!(!draft.is_complete());
}

#[test]
fn a_custom_notice_needs_a_title_and_only_counts_when_chosen() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    let standard = draft.clone();
    draft.apply(DraftEdit::NoticeTitle("{oops}".to_owned()), &types);
    // 没选“自己写”时，框里的字不会写进文件。
    assert_eq!(draft, standard);
    draft.apply(DraftEdit::Notice(NoticeChoice::Custom), &types);
    assert!(draft.notice.title().mistake().is_some());
    draft.apply(DraftEdit::NoticeTitle("  ".to_owned()), &types);
    assert!(draft.notice.title().mistake().is_some());
    draft.apply(DraftEdit::NoticeTitle("{count} 个".to_owned()), &types);
    draft.apply(DraftEdit::NoticeBody(" ".to_owned()), &types);
    assert_eq!(draft.notice.title().mistake(), None);
    assert!(matches!(draft.notice.body().value(), Some(None)));
}

#[test]
fn a_definition_comes_back_as_the_same_values() {
    let draft = draft(
        r#"name = "全部"
[[steps]]
convert = "m4a"
[[steps]]
compress = "tar.gz"
[[steps]]
extract = "folder"
[[steps]]
copy_to = "~/备份/{date}"
[[steps]]
move_to = "{dir}/音频"
[[steps]]
run = "sha256sum {files}"
[[steps]]
copy_paths = true
[[steps]]
trash = "selected"
[notify]
title = "{action}"
body = "{count}"
"#,
    );
    assert_eq!(draft.name, "全部");
    let texts = draft
        .steps
        .iter()
        .filter_map(|step| match step {
            StepDraft::CopyTo(field) | StepDraft::MoveTo(field) => Some(field.text().to_owned()),
            StepDraft::Run(field) => Some(field.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["~/备份/{date}", "{dir}/音频", "sha256sum {files}"]);
    assert_eq!(
        draft.steps[0],
        StepDraft::Convert(ConvertFormat::named("m4a").unwrap())
    );
    assert_eq!(draft.steps[1], StepDraft::Compress(ArchiveFormat::TarGz));
    assert_eq!(
        draft.steps[2],
        StepDraft::Extract(ArchiveExtractionPlacement::ArchiveFolder)
    );
    assert_eq!(draft.steps[7], StepDraft::Trash(TrashScope::Selected));
    assert_eq!(draft.notice.choice(), NoticeChoice::Custom);
    assert_eq!(draft.notice.title().text(), "{action}");
    assert_eq!(draft.notice.body().text(), "{count}");
    assert!(draft.is_complete());
}
