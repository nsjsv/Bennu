use super::*;

fn file(name: &str) -> Candidate {
    Candidate {
        name: name.to_owned(),
        is_dir: false,
    }
}

fn folder(name: &str) -> Candidate {
    Candidate {
        name: name.to_owned(),
        is_dir: true,
    }
}

fn listing(candidates: Vec<Candidate>) -> Listing {
    Listing {
        folder: Some(PathBuf::from("/w")),
        candidates,
        truncated: false,
    }
}

fn tab(mode: BoxMode, input: &str, candidates: Vec<Candidate>) -> Completed {
    complete(&word(mode, input).unwrap(), input, &listing(candidates))
}

#[test]
fn the_word_is_the_whole_path_or_the_last_word_of_a_command() {
    let home = Path::new("/home/me");
    let here = Path::new("/w");
    let path = word(BoxMode::Path, "~/Doc").unwrap();
    assert_eq!(path.name, "Doc");
    assert_eq!(
        path.source(Some(here), home),
        Some(ListSource::Folder(PathBuf::from("/home/me/")))
    );

    let program = word(BoxMode::Shell, "ech").unwrap();
    assert_eq!(program.name, "ech");
    assert_eq!(program.source(Some(here), home), Some(ListSource::Programs));

    // 后面的词是路径；转义的空格不算分隔，名字按去掉转义的找。
    let argument = word(BoxMode::Shell, "cp my\\ no").unwrap();
    assert_eq!(argument.name, "my no");
    assert_eq!(
        argument.source(Some(here), home),
        Some(ListSource::Folder(here.to_path_buf()))
    );
    let nested = word(BoxMode::Shell, "cat src/ma").unwrap();
    assert_eq!(nested.name, "ma");
    assert_eq!(
        nested.source(Some(here), home),
        Some(ListSource::Folder(PathBuf::from("/w/src/")))
    );
    // 命令的第一个词带了 / 就是路径。
    assert_eq!(word(BoxMode::Shell, "./bu").unwrap().name, "bu");
    assert_eq!(nested.source(None, home), None, "relative needs a folder");
    assert_eq!(word(BoxMode::Search, "x"), None);
}

#[test]
fn tab_finishes_a_single_match_and_a_shared_start() {
    // “Documents”和“Docker”只共享打了的“Doc”：补不动，列出来。
    assert_eq!(
        tab(
            BoxMode::Path,
            "/w/Doc",
            vec![folder("Documents"), file("Docker")]
        ),
        Completed::Choices(vec![folder("Documents"), file("Docker")])
    );
    // 唯一一个：文件夹带 `/`，命令模式里文件和命令带空格，路径模式里的文件什么都不带。
    assert_eq!(
        tab(BoxMode::Path, "/w/Doc", vec![folder("Documents")]),
        Completed::Text("/w/Documents/".to_owned())
    );
    assert_eq!(
        tab(BoxMode::Path, "/w/no", vec![file("notes.md")]),
        Completed::Text("/w/notes.md".to_owned())
    );
    assert_eq!(
        tab(BoxMode::Shell, "ech", vec![file("echo")]),
        Completed::Text("echo ".to_owned())
    );
    assert_eq!(
        tab(BoxMode::Shell, "cp my", vec![file("my notes (1).txt")]),
        Completed::Text("cp my\\ notes\\ \\(1\\).txt ".to_owned())
    );
    // 共同的开头。
    assert_eq!(
        tab(
            BoxMode::Shell,
            "cat re",
            vec![file("report-a"), file("report-b")]
        ),
        Completed::Text("cat report-".to_owned())
    );
}

#[test]
fn when_nothing_more_is_shared_the_choices_are_listed_and_tab_goes_through_them() {
    let input = "cd ~/Do";
    let Completed::Choices(candidates) = tab(
        BoxMode::Shell,
        input,
        vec![folder("Documents"), folder("Downloads")],
    ) else {
        panic!("choices");
    };
    let mut choices = Choices::new(
        word(BoxMode::Shell, input).unwrap(),
        input.to_owned(),
        candidates,
    );
    assert_eq!(choices.next(), "cd ~/Documents/");
    assert_eq!(choices.next(), "cd ~/Downloads/");
    assert_eq!(choices.next(), "cd ~/Documents/", "it wraps around");
}

#[test]
fn the_same_case_comes_first_then_any_case_and_dot_files_only_when_asked() {
    let candidates = vec![file(".hidden"), folder("Music"), file("music.txt")];
    let names = |name: &str| {
        matching(name, &candidates)
            .into_iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("mu"), ["music.txt"]);
    assert_eq!(names("MU"), ["Music", "music.txt"]);
    assert_eq!(names(""), ["Music", "music.txt"]);
    assert_eq!(names("."), [".hidden"]);
    // 大小写不同的唯一一个：照它的写法补。
    assert_eq!(
        tab(BoxMode::Path, "/w/mus", vec![folder("Music")]),
        Completed::Text("/w/Music/".to_owned())
    );
}

#[test]
fn a_cut_short_listing_never_guesses() {
    let input = "/w/a";
    let cut = Listing {
        truncated: true,
        ..listing(vec![file("a1")])
    };
    assert_eq!(
        complete(&word(BoxMode::Path, input).unwrap(), input, &cut),
        Completed::Choices(vec![file("a1")])
    );
}

#[test]
fn the_faded_suggestion_is_the_rest_of_the_first_match() {
    let input = "cd ~/Doc";
    let word = word(BoxMode::Shell, input).unwrap();
    let found = listing(vec![folder("Docker"), folder("Documents")]);
    assert_eq!(suggestion(&word, input, &found), Some("ker/".to_owned()));
    // 大小写对不上的不提示（提示只能接在打了的字后面）。
    let other_case = listing(vec![folder("docs")]);
    assert_eq!(suggestion(&word, input, &other_case), None);
    let empty = word_of("cd ~/");
    assert_eq!(
        suggestion(&empty, "cd ~/", &found),
        None,
        "nothing typed yet"
    );
}

fn word_of(input: &str) -> Word {
    word(BoxMode::Shell, input).unwrap()
}
