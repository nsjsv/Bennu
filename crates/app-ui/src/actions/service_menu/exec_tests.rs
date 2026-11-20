use std::process::Command;

use super::*;

fn values() -> ExecValues<'static> {
    ExecValues {
        name: "Convert It",
        icon: "video-x-generic",
        desktop_file: Path::new("/menus/convert.desktop"),
    }
}

fn paths(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

fn launches(exec: &str, items: &[&str]) -> Vec<ShellLaunch> {
    ExecTemplate::parse(exec)
        .unwrap()
        .launches(&paths(items), &values())
        .unwrap()
}

/// 真的交给 `/bin/sh` 跑，返回它打印的东西：值有没有原样到达，只有 shell 说了算。
fn run(launch: &ShellLaunch) -> String {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&launch.script)
        .arg("sh")
        .args(&launch.arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "{launch:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn run_once(exec: &str, items: &[&str]) -> String {
    let launches = launches(exec, items);
    assert_eq!(launches.len(), 1);
    run(&launches[0])
}

#[test]
fn every_field_code_reaches_the_program_as_kio_would_pass_it() {
    let items = ["/data/my clip.mp4", "/data/b.mp4"];
    assert_eq!(
        run_once("printf '[%s]' %F", &items),
        "[/data/my clip.mp4][/data/b.mp4]"
    );
    assert_eq!(
        run_once("printf '[%s]' %U", &items),
        "[/data/my clip.mp4][/data/b.mp4]"
    );
    assert_eq!(run_once("printf '[%s]' %N", &items), "[my clip.mp4][b.mp4]");
    assert_eq!(run_once("printf '[%s]' %D", &items), "[/data/][/data/]");
    assert_eq!(
        run_once("printf '[%s]' %c %k %i %F", &items[..1]),
        "[Convert It][/menus/convert.desktop][--icon][video-x-generic][/data/my clip.mp4]"
    );
    // %% 是文字 %；不认识的 %s 原样留下给 printf。
    assert_eq!(
        run_once("printf '[%s]' 100%% %F", &items[..1]),
        "[100%][/data/my clip.mp4]"
    );
}

#[test]
fn a_single_file_code_launches_once_per_item_in_its_own_folder() {
    let runs = launches("printf '[%s|%s]' %f %n", &["/a/one.txt", "/b/two.txt"]);
    assert_eq!(runs.len(), 2);
    assert_eq!(run(&runs[0]), "[/a/one.txt|one.txt]");
    assert_eq!(run(&runs[1]), "[/b/two.txt|two.txt]");
    assert_eq!(runs[0].directory, PathBuf::from("/a"));
    assert_eq!(runs[1].directory, PathBuf::from("/b"));
}

#[test]
fn without_any_file_code_the_path_is_appended_like_kio_does() {
    let runs = launches("printf '[%s]'", &["/a/one.txt", "/a/two.txt"]);
    assert_eq!(runs.len(), 2);
    assert_eq!(run(&runs[1]), "[/a/two.txt]");
    // %c 不算文件字段码。
    assert_eq!(
        run_once("printf '[%s]' %c", &["/a/x"]),
        "[Convert It][/a/x]"
    );
}

#[test]
fn a_singular_code_next_to_a_plural_one_is_empty_when_several_items_run_together() {
    assert_eq!(
        run_once("printf '[%s]' %f %F", &["/a/1", "/a/2"]),
        "[/a/1][/a/2]"
    );
    assert_eq!(run_once("printf '[%s]' %f %F", &["/a/1"]), "[/a/1][/a/1]");
}

#[test]
fn inside_quotes_the_value_becomes_part_of_that_string() {
    let item = ["/data/my clip.mp4"];
    assert_eq!(
        run_once(r#"printf '[%s]' "file=%f""#, &item),
        "[file=/data/my clip.mp4]"
    );
    assert_eq!(
        run_once("printf '[%s]' 'file=%f'", &item),
        "[file=/data/my clip.mp4]"
    );
    assert_eq!(
        run_once("printf '[%s]' $'file=%f'", &item),
        "[file=/data/my clip.mp4]"
    );
    assert_eq!(
        run_once(r#"printf '[%s]' "%F""#, &["/a/1", "/a/2"]),
        "[/a/1 /a/2]"
    );
    // $(…) 里面重新算作不在引号里。
    assert_eq!(
        run_once(r#"printf '[%s]' "$(printf '<%s>' %f)""#, &item),
        "[</data/my clip.mp4>]"
    );
    assert_eq!(
        run_once("printf '[%s]' `printf '<%s>' %f`", &["/a/b"]),
        "[</a/b>]"
    );
    assert_eq!(
        run_once(r#"printf '[%s]' ${UNSET:-%f}"#, &item),
        "[/data/my clip.mp4]"
    );
}

#[test]
fn shell_syntax_in_the_line_itself_still_works() {
    assert_eq!(
        run_once(
            "printf '%s\\n' %F | sort -r | tr '\\n' ' '; echo done",
            &["/a/1", "/a/2"]
        ),
        "/a/2 /a/1 done\n"
    );
    // 反斜杠转义的 % 不展开（同 KIO）。
    assert_eq!(run_once(r"printf '[%s]' \%f %F", &["/a/1"]), "[%f][/a/1]");
}

#[test]
fn a_hostile_name_is_only_ever_data_for_the_first_shell() {
    let hostile = "/tmp/$(touch pwned)`id`;rm -rf x|y&\"'\\.mp4";
    for exec in [
        "printf '[%s]' %f",
        r#"printf '[%s]' "%f""#,
        "printf '[%s]' '%f'",
        r#"printf '[%s]' "--input=%f""#,
    ] {
        let printed = run_once(exec, &[hostile]);
        assert!(
            printed.contains(hostile.trim_start_matches("/tmp/")),
            "{exec}: {printed}"
        );
    }
}

#[test]
fn a_name_handed_on_to_another_command_line_is_refused_when_it_could_run_code() {
    let template = ExecTemplate::parse(r#"bash -c "mpv \"%f\"""#).unwrap();
    for (name, character) in [
        ("/v/$(id).mp4", '$'),
        ("/v/`id`.mp4", '`'),
        ("/v/a\";id;\".mp4", '"'),
        ("/v/Tom & Jerry.mkv", '&'),
        ("/v/line\nbreak.mkv", '\n'),
    ] {
        assert_eq!(
            template.launches(&paths(&[name]), &values()),
            Err(UnsafeName {
                path: PathBuf::from(name),
                character,
            })
        );
    }
    // 平常的名字照常交下去：空格、括号、' 和中文。
    let ordinary = "/v/my clip (1) it's 视频.mp4";
    let launch = &template.launches(&paths(&[ordinary]), &values()).unwrap()[0];
    assert_eq!(launch.arguments, [OsString::from(ordinary)]);

    // 单独一个 "%f"、拼在一个词里的 "%d/out.mp3" 只是普通参数，不拦。
    for exec in [
        r#"ffmpeg -i "%f""#,
        r#"ffmpeg -o "%d/out.mp3" %f"#,
        "sh -c 'x' _ %f",
    ] {
        let template = ExecTemplate::parse(exec).unwrap();
        assert!(
            template
                .launches(&paths(&["/v/Tom & $(Jerry).mkv"]), &values())
                .is_ok(),
            "{exec}"
        );
    }
    // 单引号、$'…' 里交下去的命令行同样拦。
    for exec in ["sh -c 'mpv %f'", "sh -c $'mpv %f'", r#"sh -c "x ${A:-%f}""#] {
        let template = ExecTemplate::parse(exec).unwrap();
        assert!(
            template.launches(&paths(&["/v/a;b"]), &values()).is_err(),
            "{exec}"
        );
    }
    // %c %k 来自服务菜单文件本身，不检查。
    let template = ExecTemplate::parse(r#"sh -c "echo %c" %f"#).unwrap();
    let odd = ExecValues {
        name: "a & b",
        ..values()
    };
    assert!(template.launches(&paths(&["/v/x"]), &odd).is_ok());
}

#[test]
fn unbalanced_quotes_and_a_trailing_percent_are_mistakes() {
    for exec in [
        r#"sh -c "open"#,
        "sh -c 'open",
        "echo $(date",
        "echo )",
        "echo 100%",
    ] {
        assert!(ExecTemplate::parse(exec).is_err(), "{exec}");
    }
    assert!(ExecTemplate::parse("echo } {a,b} $((1 + (2))) $( (true) )").is_ok());
}

#[test]
fn the_program_is_checked_only_when_the_line_starts_with_a_plain_word() {
    let program = |exec: &str| {
        ExecTemplate::parse(exec)
            .unwrap()
            .program()
            .map(str::to_owned)
    };
    assert_eq!(program("ffmpeg -i %f").as_deref(), Some("ffmpeg"));
    assert_eq!(
        program("  /usr/bin/kate %U").as_deref(),
        Some("/usr/bin/kate")
    );
    assert_eq!(program("FOO=1 kate %U"), None);
    assert_eq!(program(r#""/opt/my app/run" %f"#), None);
    assert_eq!(program("$EDITOR %f"), None);
}
