use super::*;

fn parse(text: &str) -> Result<ActionDefinition, FileMistake> {
    parse_action_file(
        ActionId::from_file_stem("test"),
        Path::new("/home/u/.config/bennu/actions/test.toml"),
        text,
        &MimeDatabase::system(),
    )
}

fn mistake(text: &str) -> FileMistake {
    parse(text).expect_err("the file should be rejected")
}

#[test]
fn the_documented_example_reads_back() {
    let action = parse(
        r#"
name = "提取音频"
types = ["video/*"]

[[steps]]
convert = "m4a"

[[steps]]
move_to = "{dir}/音频"

[notify]
title = "音频提取好了"
body = "{count} 个文件 → {dir}"
"#,
    )
    .unwrap();

    assert_eq!(action.name, "提取音频");
    assert_eq!(action.id.config_key(), "action:test");
    assert!(matches!(&action.types, TypeFilter::Patterns(patterns) if patterns.len() == 1));
    assert_eq!(action.steps.len(), 2);
    assert!(matches!(&action.steps[0], ActionStep::Convert(format) if format.name() == "m4a"));
    assert!(
        matches!(&action.steps[1], ActionStep::MoveTo(folder) if folder.text() == "{dir}/音频")
    );
    assert!(matches!(
        action.notice,
        ActionNotice::Custom { body: Some(_), .. }
    ));
}

#[test]
fn every_step_kind_and_the_short_forms_read() {
    let action = parse(
        r#"
name = "everything"
steps = [
  { compress = "tar.gz" },
  { extract = "folder" },
  { copy_to = "~/backup/{date}" },
  { run = "sha256sum {files}" },
  { copy_paths = true },
  { trash = "selected" },
]
notify = "none"
"#,
    )
    .unwrap();
    assert!(matches!(action.types, TypeFilter::Anything));
    assert!(matches!(
        action.steps.as_slice(),
        [
            ActionStep::Compress(ArchiveFormat::TarGz),
            ActionStep::Extract(ArchiveExtractionPlacement::ArchiveFolder),
            ActionStep::CopyTo(_),
            ActionStep::Run(_),
            ActionStep::CopyPaths,
            ActionStep::Trash(TrashScope::Selected),
        ]
    ));
    assert_eq!(action.notice, ActionNotice::Silent);
}

#[test]
fn mistakes_point_at_their_line() {
    let cases = [
        ("steps = [{ convert = \"m4a\" }]", None),
        ("name = \"  \"\nsteps = [{ convert = \"m4a\" }]", Some(1)),
        ("name = \"x\"", None),
        (
            "name = \"x\"\n\n[[steps]]\nconvert = \"m4a\"\nmove_to = \"/x\"",
            Some(3),
        ),
        ("name = \"x\"\n[[steps]]\nconvert = \"jpeg\"", Some(3)),
        ("name = \"x\"\n[[steps]]\ncompress = \"rar\"", Some(3)),
        ("name = \"x\"\n[[steps]]\nmove_to = \"音频\"", Some(3)),
        (
            "name = \"x\"\n[[steps]]\nrun = \"cp {files} {dir}\"",
            Some(3),
        ),
        (
            "name = \"x\"\n[[steps]]\ntrash = \"current\"\n[[steps]]\ncopy_paths = true",
            Some(2),
        ),
        ("name = \"x\"\n[[steps]]\ncopy_paths = false", Some(2)),
        (
            "name = \"x\"\ntypes = [\"video\"]\nsteps = [{ copy_paths = true }]",
            Some(2),
        ),
        (
            "name = \"x\"\ntypes = []\nsteps = [{ copy_paths = true }]",
            Some(2),
        ),
        (
            "name = \"x\"\nsteps = [{ copy_paths = true }]\nnotify = \"loud\"",
            Some(3),
        ),
        (
            "name = \"x\"\nsteps = [{ copy_paths = true }]\n[notify]\nbody = \"no title\"",
            Some(3),
        ),
        (
            "name = \"x\"\nsteps = [{ copy_paths = true }]\n[notify]\ntitle = \"{size}\"",
            Some(3),
        ),
        ("name = \"x\"\nstep = []", Some(2)),
        ("name = \"x\"\n[[steps]]\nconver = \"m4a\"", Some(3)),
        ("name = \"x\nsteps = 1", Some(1)),
    ];
    for (text, line) in cases {
        let mistake = mistake(text);
        assert!(!mistake.reason.is_empty(), "{text}");
        assert_eq!(mistake.line, line, "{text}: {}", mistake.reason);
    }
}

#[test]
fn a_watch_table_makes_the_action_a_rule() {
    let action = parse(
        r#"
name = "整理截图"
[watch]
query = "截图 类型:图片"
folder = "~/图片/截图"
[[steps]]
move_to = "{dir}/{date}"
"#,
    )
    .unwrap();
    let watch = action.watch.expect("a rule");
    assert_eq!(watch.query, "截图 类型:图片");
    assert_eq!(
        watch.folder,
        file_core::app_dirs::home_dir().join("图片/截图")
    );
    assert!(parse("name = \"a\"\n[[steps]]\ncopy_paths = true\n")
        .unwrap()
        .watch
        .is_none());
}

#[test]
fn a_rule_cannot_trash_run_commands_or_watch_a_relative_folder() {
    let rule =
        |watch: &str, step: &str| format!("name = \"a\"\n[watch]\n{watch}\n[[steps]]\n{step}\n");
    let folder = "folder = \"/tmp/w\"";
    assert!(parse(&rule(folder, "copy_paths = true")).is_ok());
    for step in ["trash = \"current\"", "run = \"echo {file}\""] {
        assert!(parse(&rule(folder, step)).is_err(), "{step}");
    }
    assert!(parse(&rule("folder = \"下载\"", "copy_paths = true")).is_err());
    assert!(
        parse(&rule("query = \"x\"", "copy_paths = true")).is_err(),
        "no folder"
    );
    let bad_query = mistake(&rule(
        "query = \"类型:不存在\"\nfolder = \"/tmp/w\"",
        "copy_paths = true",
    ));
    assert!(bad_query.reason.contains("不存在"));
}
