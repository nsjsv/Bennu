use std::path::Path;

use super::*;
use crate::actions::action_file::parse_action_file;
use crate::actions::editor::draft::{DraftEdit, StepKind, StepOption, TypeCategory};
use crate::actions::{ActionDefinition, ActionId, ConvertFormat};

fn types() -> std::sync::Arc<MimeDatabase> {
    MimeDatabase::system()
}

fn read(text: &str) -> ActionDefinition {
    parse_action_file(
        ActionId::from_file_stem("a"),
        Path::new("/a.toml"),
        text,
        &types(),
    )
    .unwrap_or_else(|mistake| panic!("{mistake:?} in\n{text}"))
}

/// 读文件 → 改表单 → 写回。写出来的必须还是合法的动作文件，读回来和表单一样。
fn edited(text: &str, edits: Vec<DraftEdit>) -> String {
    let types = types();
    let before = ActionDraft::from_definition(&read(text), &types);
    let mut after = before.clone();
    for edit in edits {
        after.apply(edit, &types);
    }
    let mut document = text.parse::<DocumentMut>().unwrap();
    write_changes(&mut document, &before, &after, &types);
    let written = document.to_string();
    assert_eq!(
        ActionDraft::from_definition(&read(&written), &types),
        after,
        "{written}"
    );
    written
}

const COMMENTED: &str = r#"# 我的动作：提取音频
name = "提取音频"   # 菜单里的名字

types = ["video/*"]

# 先转格式
[[steps]]
convert = "m4a"

# 再放进一个文件夹
[[steps]]
move_to = "{dir}/音频"

[notify]
# 自己写的通知
title = "好了"
"#;

#[test]
fn a_new_document_holds_every_kind_of_step() {
    let types = types();
    let mut draft = ActionDraft::empty(&types);
    let edits = [
        DraftEdit::Name("全部".to_owned()),
        DraftEdit::Category(TypeCategory::Archives, true),
        DraftEdit::OtherTypes("image/png".to_owned()),
    ];
    for edit in edits {
        draft.apply(edit, &types);
    }
    let kinds = [
        StepKind::Convert,
        StepKind::Compress,
        StepKind::Extract,
        StepKind::CopyTo,
        StepKind::MoveTo,
        StepKind::Run,
        StepKind::CopyPaths,
        StepKind::Trash,
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        draft.apply(DraftEdit::StepAdded, &types);
        draft.apply(DraftEdit::StepKind(index, kind), &types);
    }
    draft.apply(DraftEdit::StepText(3, "~/备份/{date}".to_owned()), &types);
    draft.apply(DraftEdit::StepText(4, "{dir}/\"引号\"".to_owned()), &types);
    draft.apply(
        DraftEdit::StepText(5, r#"sh -c 'ffmpeg -i "$1" "$1.wav"' _ {file}"#.to_owned()),
        &types,
    );
    draft.apply(DraftEdit::Notice(NoticeChoice::Custom), &types);
    draft.apply(DraftEdit::NoticeTitle("{action} 好了".to_owned()), &types);
    assert!(draft.is_complete());

    let mut document = DocumentMut::new();
    write_changes(&mut document, &ActionDraft::empty(&types), &draft, &types);
    let text = document.to_string();
    assert_eq!(
        ActionDraft::from_definition(&read(&text), &types),
        draft,
        "{text}"
    );
    assert!(text.contains("[[steps]]"), "{text}");
    assert!(!text.contains("body"), "{text}");
}

#[test]
fn renaming_keeps_every_comment_and_blank_line() {
    let written = edited(COMMENTED, vec![DraftEdit::Name("抽音轨".to_owned())]);
    assert_eq!(
        written,
        COMMENTED.replace("name = \"提取音频\"", "name = \"抽音轨\"")
    );
}

#[test]
fn reordered_steps_take_their_comments_along() {
    let written = edited(COMMENTED, vec![DraftEdit::StepMovedUp(1)]);
    let move_comment = written.find("# 再放进一个文件夹").unwrap();
    let convert_comment = written.find("# 先转格式").unwrap();
    let notify = written.find("[notify]").unwrap();
    assert!(
        move_comment < convert_comment && convert_comment < notify,
        "{written}"
    );
    assert!(
        written.starts_with("# 我的动作：提取音频\nname"),
        "{written}"
    );
}

#[test]
fn a_new_step_goes_after_the_others_and_before_notify() {
    let written = edited(
        COMMENTED,
        vec![DraftEdit::StepAdded, DraftEdit::StepRemoved(0)],
    );
    let copy_paths = written.find("copy_paths = true").unwrap();
    assert!(copy_paths > written.find("move_to").unwrap(), "{written}");
    assert!(copy_paths < written.find("[notify]").unwrap(), "{written}");
    assert!(
        !written.contains("# 先转格式"),
        "the removed step's comment goes with it"
    );
    assert!(written.contains("# 自己写的通知"), "{written}");
}

#[test]
fn types_are_written_only_when_they_change() {
    let written = edited(
        COMMENTED,
        vec![
            DraftEdit::Category(TypeCategory::Video, false),
            DraftEdit::Category(TypeCategory::Audio, true),
        ],
    );
    assert!(written.contains("types = [\"audio/*\"]"), "{written}");
    let written = edited(
        COMMENTED,
        vec![DraftEdit::Category(TypeCategory::Video, false)],
    );
    assert!(!written.contains("types"), "{written}");
}

#[test]
fn steps_written_on_one_line_stay_on_one_line() {
    let text = "name = \"x\"\nsteps = [{ convert = \"m4a\" }, { move_to = \"{dir}/a\" }]  # 两步\n";
    let written = edited(
        text,
        vec![
            DraftEdit::StepMovedUp(1),
            DraftEdit::StepAdded,
            DraftEdit::StepOption(2, StepOption::Format(ConvertFormat::named("mp3").unwrap())),
        ],
    );
    assert!(!written.contains("[[steps]]"), "{written}");
    assert!(written.contains("{ move_to = \"{dir}/a\" }"), "{written}");
    assert!(written.contains("# 两步"), "{written}");
    assert_eq!(written.lines().count(), 2, "{written}");
}

#[test]
fn the_three_kinds_of_notice() {
    let silent = edited(COMMENTED, vec![DraftEdit::Notice(NoticeChoice::Silent)]);
    assert!(silent.contains("notify = \"none\""), "{silent}");
    assert!(!silent.contains("[notify]"), "{silent}");

    let standard = edited(&silent, vec![DraftEdit::Notice(NoticeChoice::Standard)]);
    assert!(!standard.contains("notify"), "{standard}");

    let custom = edited(
        &silent,
        vec![
            DraftEdit::Notice(NoticeChoice::Custom),
            DraftEdit::NoticeTitle("好了".to_owned()),
            DraftEdit::NoticeBody("{count} 个".to_owned()),
        ],
    );
    assert!(
        custom.find("[notify]").unwrap() > custom.rfind("[[steps]]").unwrap(),
        "{custom}"
    );

    let body_removed = edited(&custom, vec![DraftEdit::NoticeBody(String::new())]);
    assert!(!body_removed.contains("body"), "{body_removed}");
}

#[test]
fn an_inline_notify_table_is_edited_in_place() {
    let text =
        "name = \"x\"\nnotify = { title = \"a\" }  # 短的\nsteps = [{ copy_paths = true }]\n";
    let written = edited(text, vec![DraftEdit::NoticeTitle("b".to_owned())]);
    assert!(
        written.contains("notify = { title = \"b\" }  # 短的"),
        "{written}"
    );
}

#[test]
fn a_long_type_list_is_written_one_per_line() {
    let written = edited(
        "name = \"x\"\nsteps = [{ copy_paths = true }]\n",
        vec![DraftEdit::Category(TypeCategory::Archives, true)],
    );
    assert!(
        written.contains("types = [\n    \"application/zip\",\n"),
        "{written}"
    );
    assert!(written.contains(",\n]\n"), "{written}");
}
