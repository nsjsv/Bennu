use super::*;

fn types() -> std::sync::Arc<MimeDatabase> {
    MimeDatabase::system()
}

fn run_values() -> RunValues {
    RunValues {
        date: "2026-11-19".to_owned(),
        home: PathBuf::from("/home/me"),
    }
}

const TEXT: &str = "# 留着\nname = \"打包\"\n\n[[steps]]\ncompress = \"zip\"\n";

fn opened() -> ActionEditor {
    let types = types();
    let path = PathBuf::from("/actions/打包.toml");
    let definition =
        parse_action_file(ActionId::from_file_stem("打包"), &path, TEXT, &types).unwrap();
    ActionEditor::from_file(path, TEXT.to_owned(), &definition, &types, run_values()).unwrap()
}

fn saved(path: &str, text: &str) -> SaveOutcome {
    SaveOutcome::Saved {
        path: PathBuf::from(path),
        text: text.to_owned(),
    }
}

#[test]
fn every_template_but_the_blank_one_can_be_saved_as_it_is() {
    let types = types();
    for template in ActionTemplate::ALL {
        let mut editor = ActionEditor::from_template(template, &types, run_values());
        assert!(!editor.has_unsaved_changes(), "{template:?}");
        if template == ActionTemplate::Blank {
            assert!(!editor.can_save());
            continue;
        }
        assert!(editor.can_save(), "{template:?}");
        let request = editor.begin_save(Path::new("/actions"), &types).unwrap();
        assert!(matches!(request.target, SaveTarget::New { .. }));
        assert!(editor.is_saving());
    }
}

#[test]
fn saving_an_edit_writes_only_the_change_against_the_text_on_disk() {
    let types = types();
    let mut editor = opened();
    assert!(!editor.can_save(), "nothing changed");
    editor.edit(DraftEdit::Name("打包 zip".to_owned()), &types);
    assert!(editor.has_unsaved_changes());

    let request = editor.begin_save(Path::new("/actions"), &types).unwrap();
    assert_eq!(
        request,
        SaveRequest {
            target: SaveTarget::Existing {
                path: PathBuf::from("/actions/打包.toml"),
                expected: Some(TEXT.to_owned()),
            },
            text: TEXT.replace("\"打包\"", "\"打包 zip\""),
        }
    );
    assert!(!editor.can_save(), "one save at a time");

    editor.finish_save(saved("/actions/打包.toml", &request.text));
    assert!(matches!(editor.status, EditorStatus::Saved));
    assert!(!editor.has_unsaved_changes());
    assert!(
        matches!(&editor.target, EditTarget::File { on_disk: Some(text), .. } if *text == request.text)
    );
}

#[test]
fn changes_made_while_saving_stay_unsaved() {
    let types = types();
    let mut editor = opened();
    editor.edit(DraftEdit::Name("一".to_owned()), &types);
    let request = editor.begin_save(Path::new("/actions"), &types).unwrap();
    editor.edit(DraftEdit::Name("二".to_owned()), &types);
    editor.finish_save(saved("/actions/打包.toml", &request.text));
    assert!(editor.has_unsaved_changes());
    let next = editor.begin_save(Path::new("/actions"), &types).unwrap();
    assert!(next.text.contains("name = \"二\""), "{}", next.text);
}

#[test]
fn a_new_action_gets_its_identity_from_the_saved_file() {
    let types = types();
    let mut editor =
        ActionEditor::from_template(ActionTemplate::ExtractAudio, &types, run_values());
    assert_eq!(editor.action_id(), None);
    let request = editor.begin_save(Path::new("/actions"), &types).unwrap();
    assert_eq!(
        request.target,
        SaveTarget::New {
            folder: PathBuf::from("/actions"),
            name: tr!("Extract Audio"),
        }
    );
    editor.finish_save(saved("/actions/提取音频 2.toml", &request.text));
    assert_eq!(
        editor.action_id(),
        Some(&ActionId::from_file_stem("提取音频 2"))
    );
    assert!(!editor.can_save());
}

#[test]
fn a_change_on_disk_is_shown_and_saving_anyway_expects_what_was_found() {
    let types = types();
    let mut editor = opened();
    editor.edit(DraftEdit::Name("新".to_owned()), &types);
    editor.begin_save(Path::new("/actions"), &types).unwrap();
    editor.finish_save(SaveOutcome::Changed {
        found: Some("别人写的".to_owned()),
    });
    assert!(matches!(
        &editor.status,
        EditorStatus::Changed { found: Some(_) }
    ));
    assert!(editor.has_unsaved_changes());

    editor.accept_change_on_disk();
    let request = editor.begin_save(Path::new("/actions"), &types).unwrap();
    assert!(matches!(
        request.target,
        SaveTarget::Existing { expected: Some(ref text), .. } if text == "别人写的"
    ));
}

#[test]
fn a_duplicate_is_a_new_action_with_the_comments() {
    let types = types();
    let mut copy = opened().duplicate(&types);
    assert_eq!(copy.draft.name, tr!("%1 Copy", "打包"));
    assert!(matches!(copy.target, EditTarget::New));
    let request = copy.begin_save(Path::new("/actions"), &types).unwrap();
    assert!(request.text.starts_with("# 留着\n"), "{}", request.text);
}
