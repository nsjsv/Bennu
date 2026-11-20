use std::ffi::OsStr;

use super::*;
use crate::actions::handed_on::UnsafeName;

fn run() -> RunValues {
    RunValues {
        date: "2026-11-19".to_owned(),
        home: PathBuf::from("/home/u"),
    }
}

fn arguments(invocation: &[OsString]) -> Vec<&str> {
    invocation
        .iter()
        .map(|argument| argument.to_str().unwrap())
        .collect()
}

#[test]
fn folder_templates_expand_per_item_and_from_home() {
    let beside = PathTemplate::parse("{dir}/音频").unwrap();
    assert_eq!(
        beside.resolve(Path::new("/videos/a.mp4"), &run()),
        PathBuf::from("/videos/音频")
    );
    let by_date = PathTemplate::parse("~/备份/{date}/{stem}-{ext}").unwrap();
    assert_eq!(
        by_date.resolve(Path::new("/docs/report.tar.gz"), &run()),
        PathBuf::from("/home/u/备份/2026-11-19/report.tar-gz")
    );
    assert_eq!(
        PathTemplate::parse("~")
            .unwrap()
            .resolve(Path::new("/a"), &run()),
        PathBuf::from("/home/u")
    );
    assert_eq!(
        PathTemplate::parse("/srv/{{raw}}")
            .unwrap()
            .resolve(Path::new("/a"), &run()),
        PathBuf::from("/srv/{raw}")
    );
    // 根目录下的项：{dir} 就是根目录本身，展开出来仍是绝对路径。
    assert_eq!(
        beside.resolve(Path::new("/"), &run()),
        PathBuf::from("//音频")
    );
}

#[test]
fn folder_templates_must_start_absolute_and_use_known_variables() {
    for wrong in [
        "音频",
        "./x",
        "~user/x",
        "{name}/x",
        "",
        "{file}",
        "/x/{files}",
    ] {
        assert!(PathTemplate::parse(wrong).is_err(), "{wrong}");
    }
    assert!(PathTemplate::parse("/x/{size}").is_err());
    assert!(PathTemplate::parse("/x/{dir").is_err());
}

#[test]
fn per_item_variables_run_once_per_item() {
    let command = CommandTemplate::parse("ffmpeg -i {file} '{dir}/{stem}.wav'").unwrap();
    let items = [
        PathBuf::from("/v/a b.mp4"),
        PathBuf::from("/v/$(rm -rf ~);x.mkv"),
    ];
    let invocations = command.invocations(&items, &run()).unwrap();
    assert_eq!(invocations.len(), 2);
    assert_eq!(
        arguments(&invocations[0]),
        ["ffmpeg", "-i", "/v/a b.mp4", "/v/a b.wav"]
    );
    // 文件名里的 shell 字符只是参数内容。
    assert_eq!(
        arguments(&invocations[1]),
        [
            "ffmpeg",
            "-i",
            "/v/$(rm -rf ~);x.mkv",
            "/v/$(rm -rf ~);x.wav"
        ]
    );
}

#[test]
fn files_expands_into_one_argument_per_item() {
    let command = CommandTemplate::parse("kdeconnect-cli --share {files} --date {date}").unwrap();
    let items = [PathBuf::from("/a/1.png"), PathBuf::from("/a/two words.png")];
    let invocations = command.invocations(&items, &run()).unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        arguments(&invocations[0]),
        [
            "kdeconnect-cli",
            "--share",
            "/a/1.png",
            "/a/two words.png",
            "--date",
            "2026-11-19"
        ]
    );
}

#[test]
fn commands_without_item_variables_run_once() {
    let command = CommandTemplate::parse("git pull --ff-only").unwrap();
    let invocations = command
        .invocations(&[PathBuf::from("/a"), PathBuf::from("/b")], &run())
        .unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(arguments(&invocations[0]), ["git", "pull", "--ff-only"]);
}

#[test]
fn a_name_inside_a_command_line_for_another_shell_is_checked_first() {
    let command = CommandTemplate::parse("sh -c 'ffmpeg -i {file} {stem}.wav'").unwrap();
    let hostile = PathBuf::from("/v/$(id).mp4");
    assert_eq!(
        command.invocations(std::slice::from_ref(&hostile), &run()),
        Err(UnsafeName {
            path: hostile,
            character: '$',
        })
    );
    // 普通名字照常交下去；一个词里的变量（没有空白）不算交给别的命令行。
    let ordinary = PathBuf::from("/v/my clip (1) it's.mp4");
    let invocations = command
        .invocations(std::slice::from_ref(&ordinary), &run())
        .unwrap();
    assert_eq!(
        arguments(&invocations[0]),
        [
            "sh",
            "-c",
            "ffmpeg -i /v/my clip (1) it's.mp4 my clip (1) it's.wav"
        ]
    );
    let plain = CommandTemplate::parse("ffmpeg -i {file} --out={dir}/{stem}.wav").unwrap();
    assert!(plain
        .invocations(&[PathBuf::from("/v/a;b&c.mp4")], &run())
        .is_ok());
    // 安全的写法：名字作为单独的参数交给 sh。
    let positional = CommandTemplate::parse("sh -c 'ffmpeg -i \"$1\"' _ {file}").unwrap();
    assert!(positional
        .invocations(&[PathBuf::from("/v/$(id).mp4")], &run())
        .is_ok());
}

#[test]
fn command_mistakes_are_caught_when_reading_the_file() {
    for wrong in [
        "cp {files} {dir}",
        "tar -cf out.tar --files={files}",
        "echo \"unclosed",
        "",
        "echo {size}",
    ] {
        assert!(CommandTemplate::parse(wrong).is_err(), "{wrong}");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_file_names_pass_through_byte_for_byte() {
    use std::os::unix::ffi::OsStrExt;

    let command = CommandTemplate::parse("stat {file}").unwrap();
    let item = PathBuf::from(OsStr::from_bytes(b"/a/caf\xe9.txt"));
    let invocations = command
        .invocations(std::slice::from_ref(&item), &run())
        .unwrap();
    assert_eq!(invocations[0][1].as_bytes(), b"/a/caf\xe9.txt");
}
