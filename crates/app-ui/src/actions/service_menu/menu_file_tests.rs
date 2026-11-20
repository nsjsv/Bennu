use std::path::PathBuf;

use super::*;
use crate::actions::type_filter::{ItemType, TargetTypes};

const CONVERT: &str = "[Desktop Entry]\n\
    Type=Service\n\
    MimeType=video/*;\n\
    ExcludeServiceTypes=video/x-matroska\n\
    X-KDE-Priority=TopLevel\n\
    X-KDE-Submenu=Convert\n\
    X-KDE-Submenu[zh_CN]=转换\n\
    X-KDE-MaxNumberOfUrls=3\n\
    Actions=toMp3;_SEPARATOR_;off;broken;noExec;\n\
    \n\
    [Desktop Action toMp3]\n\
    Name=To MP3\n\
    Name[zh_CN]=转成 MP3\n\
    Icon=audio-x-generic\n\
    Exec=ffmpeg -i %f %d/out.mp3\n\
    \n\
    [Desktop Action off]\n\
    Name=Hidden in Dolphin\n\
    Exec=true %f\n\
    \n\
    [Desktop Action broken]\n\
    Exec=sh -c \"open %f\n\
    \n\
    [Desktop Action noExec]\n\
    Name=No command\n";

fn read(text: &str, folder: MenuFolder, locale: Option<&str>) -> Result<MenuFile, String> {
    let types = MimeDatabase::system();
    let hidden = HashSet::from(["off".to_owned()]);
    let context = ReadContext {
        types: &types,
        locale,
        hidden: &hidden,
    };
    read_menu_file(
        Path::new("/menus/convert.desktop"),
        "convert.desktop",
        text,
        folder,
        &context,
    )
}

fn applies(action: &ServiceMenuAction, items: &[(&str, bool)]) -> bool {
    let types = MimeDatabase::system();
    let targets = TargetTypes::new(
        items
            .iter()
            .map(|(path, is_directory)| ItemType::of(Path::new(path), *is_directory, &types)),
        &types,
    );
    action.applies_to(&targets, items.len())
}

#[test]
fn a_menu_file_becomes_its_usable_actions_with_mistakes_listed_apart() {
    let menu = read(CONVERT, MenuFolder::Current, Some("zh_CN")).unwrap();

    assert_eq!(menu.actions.len(), 1);
    let action = &menu.actions[0];
    assert_eq!(action.key.file.as_ref(), "convert.desktop");
    assert_eq!(action.key.action.as_ref(), "toMp3");
    assert_eq!(action.name, "转成 MP3");
    assert_eq!(action.icon, "audio-x-generic");
    assert_eq!(action.file, PathBuf::from("/menus/convert.desktop"));
    assert_eq!(action.priority, Priority::TopLevel);
    assert_eq!(action.submenu.as_deref(), Some("转换"));
    assert_eq!(action.command.program(), Some("ffmpeg"));
    // 关掉的（off）和分隔线不算写错；不配对的引号、缺 Exec 的各报一条。
    assert_eq!(menu.mistakes.len(), 2, "{:?}", menu.mistakes);
    assert!(menu.mistakes[0].starts_with("[Desktop Action broken]"));
    assert!(menu.mistakes[1].starts_with("[Desktop Action noExec]"));

    let english = read(CONVERT, MenuFolder::Current, None).unwrap();
    assert_eq!(english.actions[0].name, "To MP3");
    assert_eq!(english.actions[0].submenu.as_deref(), Some("Convert"));
}

#[test]
fn types_exclusions_and_counts_decide_where_it_shows() {
    let menu = read(CONVERT, MenuFolder::Current, None).unwrap();
    let action = &menu.actions[0];
    assert!(applies(action, &[("/v/a.mp4", false)]));
    assert!(applies(
        action,
        &[("/v/a.mp4", false), ("/v/b.webm", false)]
    ));
    // 每一项都要是视频；一项是排除的类型也不行；最多 3 项。
    assert!(!applies(
        action,
        &[("/v/a.mp4", false), ("/v/b.txt", false)]
    ));
    assert!(!applies(
        action,
        &[("/v/a.mp4", false), ("/v/b.mkv", false)]
    ));
    assert!(!applies(
        action,
        &[
            ("/v/1.mp4", false),
            ("/v/2.mp4", false),
            ("/v/3.mp4", false),
            ("/v/4.mp4", false)
        ]
    ));
    assert!(!applies(action, &[("/v", true)]));
}

#[test]
fn service_types_stand_in_for_a_missing_mime_type_and_exact_counts_are_honoured() {
    let text = "[Desktop Entry]\n\
        ServiceTypes=KonqPopupMenu/Plugin,inode/directory\n\
        X-KDE-RequiredNumberOfUrls=1,3\n\
        Actions=open\n\
        [Desktop Action open]\n\
        Exec=konsole --workdir %f\n";
    let menu = read(text, MenuFolder::Legacy, None).unwrap();
    let action = &menu.actions[0];
    assert_eq!(action.name, "open");
    assert!(applies(action, &[("/a", true)]));
    assert!(!applies(action, &[("/a", true), ("/b", true)]));
    assert!(applies(action, &[("/a", true), ("/b", true), ("/c", true)]));
    assert!(!applies(action, &[("/a.txt", false)]));
}

#[test]
fn files_that_are_not_for_local_files_or_not_service_menus_are_skipped_quietly() {
    let with = |extra: &str| {
        format!(
            "[Desktop Entry]\nMimeType=all/all\n{extra}\nActions=a\n[Desktop Action a]\nExec=true\n"
        )
    };
    let count = |text: &str, folder| read(text, folder, None).unwrap().actions.len();
    assert_eq!(count(&with(""), MenuFolder::Current), 1);
    assert_eq!(count(&with("X-KDE-Protocol=file"), MenuFolder::Current), 1);
    assert_eq!(
        count(&with("X-KDE-Protocol=!trash"), MenuFolder::Current),
        1
    );
    assert_eq!(count(&with("X-KDE-Protocol=smb"), MenuFolder::Current), 0);
    assert_eq!(count(&with("X-KDE-Protocol=!file"), MenuFolder::Current), 0);
    assert_eq!(
        count(&with("X-KDE-Protocols=trash,file"), MenuFolder::Current),
        1
    );
    assert_eq!(
        count(&with("X-KDE-Protocols=trash"), MenuFolder::Current),
        0
    );
    // kservices5 里没有 KonqPopupMenu/Plugin 的不是服务菜单。
    assert_eq!(count(&with(""), MenuFolder::Legacy), 0);
    assert_eq!(
        count(
            &with("ServiceTypes=KonqPopupMenu/Plugin"),
            MenuFolder::Legacy
        ),
        1
    );
}

#[test]
fn a_file_without_types_actions_or_its_main_group_is_unusable() {
    for text in [
        "[Desktop Entry]\nActions=a\n[Desktop Action a]\nExec=true\n",
        "[Desktop Entry]\nMimeType=not a type;\nActions=a\n[Desktop Action a]\nExec=true\n",
        "[Desktop Entry]\nMimeType=all/all\n",
        "[Something Else]\nMimeType=all/all\n",
        "not a desktop file",
    ] {
        assert!(read(text, MenuFolder::Current, None).is_err(), "{text}");
    }
}

#[test]
fn an_exec_line_written_for_dolphin_with_escaped_quotes_is_read_like_kde_reads_it() {
    let text = "[Desktop Entry]\nMimeType=all/all\nActions=play\n\
        [Desktop Action play]\nExec=bash -c \"mpv \\\"%f\\\"\"\n";
    let menu = read(text, MenuFolder::Current, None).unwrap();
    assert!(menu.mistakes.is_empty(), "{:?}", menu.mistakes);
    let values = super::super::exec::ExecValues {
        name: "play",
        icon: "",
        desktop_file: Path::new("/menus/convert.desktop"),
    };
    let command = &menu.actions[0].command;
    // 文件名进了第二层 shell 的命令行：普通名字放行，危险的拦下。
    assert!(command
        .launches(&[PathBuf::from("/v/a b.mp4")], &values)
        .is_ok());
    assert!(command
        .launches(&[PathBuf::from("/v/$(id).mp4")], &values)
        .is_err());
}
