use std::os::unix::fs::PermissionsExt;

use super::*;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn names_in(folder: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn file_names_come_from_the_action_name() {
    assert_eq!(file_stem_for("  提取音频 "), "提取音频");
    assert_eq!(file_stem_for("a/b"), "a b");
    assert_eq!(file_stem_for("..隐藏"), "隐藏");
    assert_eq!(file_stem_for("tab\there"), "tab here");
    assert_eq!(file_stem_for(" / . "), tr!("Action"));
    let long = file_stem_for(&"音".repeat(100));
    assert!(long.len() <= 200 && long.chars().all(|character| character == '音'));
}

#[test]
fn a_new_action_never_replaces_a_file_and_is_private() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("清理.toml"), "old").unwrap();
    let saved = save(
        SaveTarget::New {
            folder: folder.path().to_path_buf(),
            name: "清理".to_owned(),
        },
        "new".to_owned(),
    );
    let path = folder.path().join("清理 2.toml");
    assert_eq!(
        saved,
        SaveOutcome::Saved {
            path: path.clone(),
            text: "new".to_owned()
        }
    );
    assert_eq!(
        std::fs::read_to_string(folder.path().join("清理.toml")).unwrap(),
        "old"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(mode(&path), 0o600);
    assert_eq!(
        names_in(folder.path()),
        ["清理 2.toml", "清理.toml"],
        "no temporary file is left"
    );
}

#[test]
fn replacing_keeps_the_mode_and_writes_through_a_symlink() {
    let folder = tempfile::tempdir().unwrap();
    let real = folder.path().join("dotfiles/a.toml");
    std::fs::create_dir(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "before").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = folder.path().join("a.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let saved = save(
        SaveTarget::Existing {
            path: link.clone(),
            expected: Some("before".to_owned()),
        },
        "after".to_owned(),
    );

    assert!(matches!(saved, SaveOutcome::Saved { ref path, .. } if *path == link));
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "after");
    assert_eq!(mode(&real), 0o640);
    assert_eq!(names_in(real.parent().unwrap()), ["a.toml"]);
}

#[test]
fn a_file_changed_elsewhere_is_not_written() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("a.toml");
    std::fs::write(&path, "changed elsewhere").unwrap();
    let saved = save(
        SaveTarget::Existing {
            path: path.clone(),
            expected: Some("opened".to_owned()),
        },
        "mine".to_owned(),
    );
    assert_eq!(
        saved,
        SaveOutcome::Changed {
            found: Some("changed elsewhere".to_owned())
        }
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed elsewhere");

    std::fs::remove_file(&path).unwrap();
    let saved = save(
        SaveTarget::Existing {
            path: path.clone(),
            expected: Some("changed elsewhere".to_owned()),
        },
        "mine".to_owned(),
    );
    assert_eq!(saved, SaveOutcome::Changed { found: None });
    assert!(!path.exists());
}

#[test]
fn saving_a_deleted_file_anyway_puts_it_back_but_never_over_a_new_one() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("a.toml");
    let saved = save(
        SaveTarget::Existing {
            path: path.clone(),
            expected: None,
        },
        "mine".to_owned(),
    );
    assert!(matches!(saved, SaveOutcome::Saved { .. }));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");

    let saved = save(
        SaveTarget::Existing {
            path: path.clone(),
            expected: None,
        },
        "again".to_owned(),
    );
    assert_eq!(
        saved,
        SaveOutcome::Changed {
            found: Some("mine".to_owned())
        }
    );
}
