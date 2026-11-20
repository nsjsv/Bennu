use std::os::unix::fs::PermissionsExt;

use super::*;

fn menu(name: &str, extra: &str) -> String {
    format!(
        "[Desktop Entry]\nMimeType=all/all\n{extra}\nActions=go\n[Desktop Action go]\nName={name}\nExec=true %f\n"
    )
}

fn put(directory: &Path, file: &str, text: &str, mode: u32) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(file);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn load(data_dirs: &[PathBuf], show_settings: &Path) -> (Vec<String>, Vec<ActionFileProblem>) {
    let types = MimeDatabase::system();
    let (actions, problems) = load_service_menus(data_dirs, show_settings, &types, None);
    (
        actions.iter().map(|action| action.name.clone()).collect(),
        problems,
    )
}

#[test]
fn the_first_file_of_a_name_wins_and_new_folders_come_before_old_ones() {
    let root = tempfile::tempdir().unwrap();
    let user = root.path().join("user");
    let system = root.path().join("system");
    put(
        &user.join("kio/servicemenus"),
        "b.desktop",
        &menu("user b", ""),
        0o700,
    );
    put(
        &system.join("kio/servicemenus"),
        "b.desktop",
        &menu("system b", ""),
        0o755,
    );
    put(
        &system.join("kio/servicemenus"),
        "a.desktop",
        &menu("system a", ""),
        0o755,
    );
    // 旧位置：同名的被新位置盖掉；没有 KonqPopupMenu/Plugin 的不是服务菜单。
    let legacy = "ServiceTypes=KonqPopupMenu/Plugin";
    put(
        &user.join("kservices5"),
        "a.desktop",
        &menu("legacy a", legacy),
        0o600,
    );
    put(
        &user.join("kservices5"),
        "c.desktop",
        &menu("legacy c", legacy),
        0o600,
    );
    put(
        &user.join("kservices5"),
        "app.desktop",
        &menu("not a menu", ""),
        0o600,
    );
    std::fs::create_dir_all(user.join("kio/servicemenus/sub")).unwrap();
    put(
        &user.join("kio/servicemenus/sub"),
        "deep.desktop",
        &menu("too deep", ""),
        0o700,
    );

    let (names, problems) = load(&[user, system], &root.path().join("kservicemenurc"));

    assert_eq!(names, ["user b", "system a", "legacy c"]);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_menu_in_the_new_folder_needs_the_executable_bit_and_others_must_not_change_it() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let menus = data.join("kio/servicemenus");
    put(&menus, "allowed.desktop", &menu("allowed", ""), 0o700);
    let plain = put(&menus, "plain.desktop", &menu("plain", ""), 0o600);
    put(&menus, "shared.desktop", &menu("shared", ""), 0o777);
    // kservices5 里的不要求执行权限（同 KDE）。
    put(
        &data.join("kservices5"),
        "legacy.desktop",
        &menu("legacy", "ServiceTypes=KonqPopupMenu/Plugin"),
        0o644,
    );

    let (names, problems) = load(&[data], &root.path().join("kservicemenurc"));

    assert_eq!(names, ["allowed", "legacy"]);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert_eq!(problems[0].file, plain);
    assert!(problems[0]
        .reason
        .contains(&format!("chmod +x {}", plain.display())));
    assert!(problems[1].file.ends_with("shared.desktop"));
}

#[test]
fn actions_turned_off_in_dolphin_are_left_out() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    put(
        &data.join("kio/servicemenus"),
        "one.desktop",
        &menu("one", ""),
        0o700,
    );
    let settings = root.path().join("kservicemenurc");
    std::fs::write(&settings, "[Show]\ngo=false\n").unwrap();

    let (names, problems) = load(std::slice::from_ref(&data), &settings);
    assert!(names.is_empty());
    assert!(problems.is_empty());

    std::fs::write(&settings, "[Show]\ngo=true\n").unwrap();
    assert_eq!(load(&[data], &settings).0, ["one"]);
}

#[test]
fn the_show_group_reads_false_values_the_way_kconfig_does() {
    let hidden = hidden_actions(
        "[General]\nskipped=false\n[Show]\na=false\nb = Off\nc=no\nd=0\ne=true\nf\n[Other]\ng=false\n",
    );
    let mut hidden = hidden.into_iter().collect::<Vec<_>>();
    hidden.sort();
    assert_eq!(hidden, ["a", "b", "c", "d"]);
}
