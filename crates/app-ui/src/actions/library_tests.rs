use std::os::unix::fs::PermissionsExt;

use super::*;

const EXTRACT_AUDIO: &str = r#"
name = "提取音频"
types = ["video/*"]
steps = [{ convert = "m4a" }]
"#;

/// 只有这个动作文件夹，没有服务菜单。
fn sources(folder: &Path) -> ActionSources {
    ActionSources {
        action_folder: folder.to_path_buf(),
        data_dirs: Vec::new(),
        show_settings: folder.join("kservicemenurc"),
        language: UiLanguage::English,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_missing_folder_has_no_actions_and_no_problems() {
    let directory = tempfile::tempdir().unwrap();
    let library = ActionLibrary::load(&sources(&directory.path().join("actions")));
    assert!(library.definitions().is_empty());
    assert!(library.problems().is_empty());
}

#[test]
fn good_files_load_and_bad_ones_are_listed_without_stopping_the_rest() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    std::fs::create_dir(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    write(&folder.join("extract-audio.toml"), EXTRACT_AUDIO);
    write(
        &folder.join("broken.toml"),
        "name = \"x\"\n[[steps]]\nconvert = \"jpeg\"\n",
    );
    write(&folder.join("notes.txt"), "not an action");
    let shared = folder.join("shared.toml");
    write(&shared, EXTRACT_AUDIO);
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o666)).unwrap();
    // 叫这个名字的管道不会卡住读取。
    let fifo = std::ffi::CString::new(
        folder
            .join("pipe.toml")
            .into_os_string()
            .into_encoded_bytes(),
    )
    .unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let library = ActionLibrary::load(&sources(&folder));

    let names = library
        .definitions()
        .iter()
        .map(|definition| definition.id.fallback_label())
        .collect::<Vec<_>>();
    assert_eq!(names, ["extract-audio"]);
    let problems = library
        .problems()
        .iter()
        .map(|problem| {
            (
                problem.file.file_name().unwrap().to_str().unwrap(),
                problem.line,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        problems,
        [
            ("broken.toml", Some(3)),
            ("pipe.toml", None),
            ("shared.toml", None)
        ]
    );
}

#[test]
fn a_folder_other_users_can_change_loads_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    std::fs::create_dir(&folder).unwrap();
    write(&folder.join("extract-audio.toml"), EXTRACT_AUDIO);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o777)).unwrap();

    let library = ActionLibrary::load(&sources(&folder));

    assert!(library.definitions().is_empty());
    assert_eq!(library.problems().len(), 1);
    assert_eq!(library.problems()[0].file, folder);
}

#[test]
fn only_the_user_or_root_may_own_action_files() {
    assert_eq!(trust_problem(1000, 0o100600, 1000), None);
    assert_eq!(trust_problem(0, 0o100644, 1000), None);
    assert!(trust_problem(1001, 0o100600, 1000).is_some());
    assert!(trust_problem(1000, 0o100602, 1000).is_some());
    // 同组可写不算：很多发行版的默认 umask 是 002，组是用户自己的组。
    assert_eq!(trust_problem(1000, 0o100660, 1000), None);
}

#[test]
fn matching_follows_the_types_of_the_whole_selection() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("actions");
    std::fs::create_dir(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    write(&folder.join("extract-audio.toml"), EXTRACT_AUDIO);
    write(
        &folder.join("anything.toml"),
        "name = \"Anything\"\nsteps = [{ copy_paths = true }]\n",
    );
    let library = ActionLibrary::load(&sources(&folder));
    let names = |items: &[(&str, bool)]| {
        let target = library.target(
            items
                .iter()
                .map(|(path, is_directory)| (Path::new(*path), *is_directory)),
        );
        library
            .applicable(&target)
            .own
            .iter()
            .map(|definition| definition.name.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(names(&[("/v/a.mp4", false)]), ["Anything", "提取音频"]);
    assert_eq!(
        names(&[("/v/a.mp4", false), ("/v/b.txt", false)]),
        ["Anything"]
    );
    assert_eq!(names(&[("/v", true)]), ["Anything"]);
    assert!(names(&[]).is_empty());
}
