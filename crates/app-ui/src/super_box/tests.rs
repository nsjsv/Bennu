use std::time::Instant;

use super::matching::ranked;
use super::mode::{fold, split_prefix};
use super::targets::{jump_path, read_new_entry};
use super::*;
use crate::actions::ActionId;
use crate::model::SettingsCategory;
use crate::shortcuts::ShortcutAction;

fn context<'a>(taken: &'a dyn Fn(&str) -> bool) -> BoxContext<'a> {
    BoxContext {
        current_dir: Some(Path::new("/home/me/work")),
        home: Path::new("/home/me"),
        cannot_create: None,
        taken,
    }
}

fn nothing_taken(_: &str) -> bool {
    false
}

fn feature(title: &str, key: &str) -> BoxEntry {
    BoxEntry::new(
        EntryKind::Feature,
        title.to_owned(),
        BoxTarget::Builtin(ShortcutAction::NewFolder),
    )
    .with_keywords(key)
}

fn titles(entries: &[BoxEntry], text: &str) -> Vec<String> {
    ranked(entries, text)
        .into_iter()
        .map(|index| entries[index].title.clone())
        .collect()
}

#[test]
fn prefixes_switch_modes_with_either_colon() {
    assert_eq!(split_prefix("g:~/下载"), Some((BoxMode::Path, "~/下载")));
    assert_eq!(split_prefix("G：  /tmp"), Some((BoxMode::Path, "/tmp")));
    assert_eq!(split_prefix("n: 笔记.md"), Some((BoxMode::New, "笔记.md")));
    assert_eq!(split_prefix("f:报告"), Some((BoxMode::Files, "报告")));
    assert_eq!(split_prefix("s: ls -l"), Some((BoxMode::Shell, "ls -l")));
    assert_eq!(split_prefix("x: 别的"), None);
    assert_eq!(split_prefix("g"), None);
    assert_eq!(split_prefix("go:"), None);
}

#[test]
fn shift_tab_goes_round_the_modes() {
    let mut mode = BoxMode::Search;
    let mut seen = Vec::new();
    for _ in 0..BoxMode::ALL.len() {
        mode = mode.next();
        seen.push(mode);
    }
    assert_eq!(
        seen,
        [
            BoxMode::Files,
            BoxMode::Path,
            BoxMode::New,
            BoxMode::Shell,
            BoxMode::Search
        ]
    );
    for mode in BoxMode::ALL {
        assert_eq!(BoxMode::from_config_value(mode.config_value()), Some(mode));
    }
}

#[test]
fn full_width_letters_and_case_fold_away() {
    assert_eq!(fold("ＸＪ Ｗｊ　New"), "xj wj new");
}

#[test]
fn matches_rank_start_then_word_then_pinyin_then_anywhere_then_keywords() {
    let entries = vec![
        feature("复制路径", "copy_path"),
        feature("新建文件夹", "new_folder"),
        feature("打开新标签页", "new_tab"),
        feature("新建文件", "new_file"),
        feature("Show Hidden Files", "toggle_hidden_files"),
    ];
    assert_eq!(titles(&entries, "新建"), ["新建文件", "新建文件夹"]);
    assert_eq!(titles(&entries, "xjwjj"), ["新建文件夹"]);
    assert_eq!(titles(&entries, "hidden"), ["Show Hidden Files"]);
    assert_eq!(titles(&entries, "标签"), ["打开新标签页"]);
    // 中文界面里打英文配置名：排在名字命中的后面。
    assert_eq!(
        titles(&entries, "new"),
        ["新建文件", "新建文件夹", "打开新标签页"]
    );
    assert!(titles(&entries, "   ").is_empty());
}

#[test]
fn a_whole_match_comes_before_a_longer_one_of_any_kind() {
    let entries = vec![
        feature("快捷键一览", "show_shortcuts"),
        BoxEntry::new(
            EntryKind::Setting,
            "快捷键".to_owned(),
            BoxTarget::Settings(SettingsCategory::Shortcuts),
        ),
        feature("Search", "search"),
        feature("Searching Again", "search_again"),
    ];
    assert_eq!(titles(&entries, "kjj"), ["快捷键", "快捷键一览"]);
    assert_eq!(titles(&entries, "快捷键"), ["快捷键", "快捷键一览"]);
    assert_eq!(titles(&entries, "search"), ["Search", "Searching Again"]);
}

#[test]
fn within_a_tier_features_come_before_actions_settings_and_favorites() {
    let entries = vec![
        BoxEntry::new(
            EntryKind::Favorite,
            "Downloads".to_owned(),
            BoxTarget::Folder(PathBuf::from("/home/me/Downloads")),
        ),
        BoxEntry::new(
            EntryKind::Setting,
            "Display".to_owned(),
            BoxTarget::Settings(SettingsCategory::Appearance),
        ),
        BoxEntry::new(
            EntryKind::Action,
            "Deploy".to_owned(),
            BoxTarget::Action(ActionId::from_file_stem("deploy")),
        ),
        feature("Delete", "delete"),
    ];
    assert_eq!(
        titles(&entries, "d"),
        ["Delete", "Deploy", "Display", "Downloads"]
    );
}

#[test]
fn matching_stays_well_under_a_millisecond_per_keystroke() {
    let entries = (0..120)
        .map(|index| feature(&format!("新建文件夹 动作 {index}"), "new_folder_action"))
        .collect::<Vec<_>>();
    let typed = "xjwjj dz 1";
    let started = Instant::now();
    for end in 1..=typed.len() {
        std::hint::black_box(ranked(&entries, &typed[..end]));
    }
    let per_keystroke = started.elapsed() / typed.len() as u32;
    assert!(
        per_keystroke.as_micros() < 1000,
        "{per_keystroke:?} per keystroke"
    );
}

#[test]
fn jump_paths_expand_the_home_folder_and_follow_the_current_folder() {
    let home = Path::new("/home/me");
    let here = Some(Path::new("/home/me/work"));
    assert_eq!(jump_path("~", here, home), Some(PathBuf::from("/home/me")));
    assert_eq!(
        jump_path(" ~/下载 ", here, home),
        Some(PathBuf::from("/home/me/下载"))
    );
    assert_eq!(jump_path("/tmp", None, home), Some(PathBuf::from("/tmp")));
    assert_eq!(
        jump_path("src", here, home),
        Some(PathBuf::from("/home/me/work/src"))
    );
    assert_eq!(jump_path("src", None, home), None);
    assert_eq!(jump_path("  ", here, home), None);
}

#[test]
fn new_names_are_checked_against_the_rules_and_the_folder() {
    let taken = |name: &str| name == "已有.txt";
    assert_eq!(
        read_new_entry(" 笔记.md ", taken),
        Ok(NewEntry::File("笔记.md".to_owned()))
    );
    assert_eq!(
        read_new_entry("素材/", taken),
        Ok(NewEntry::Folder("素材".to_owned()))
    );
    assert!(read_new_entry("已有.txt", taken).is_err());
    for bad in ["", "/", "..", ".", "a/b", "a/b/"] {
        assert!(read_new_entry(bad, taken).is_err(), "{bad:?}");
    }
}

#[test]
fn typing_a_prefix_switches_the_mode_and_drops_the_prefix() {
    let taken = nothing_taken;
    let context = context(&taken);
    let mut super_box = SuperBox::open(BoxMode::Search, Vec::new(), Vec::new(), &context);
    let lookup = super_box.set_input("g:~/下载".to_owned(), &context);
    assert_eq!(super_box.mode(), BoxMode::Path);
    assert_eq!(super_box.input(), "~/下载");
    assert_eq!(
        lookup,
        Some(BoxLookup::List {
            generation: super_box.generation(),
            source: ListSource::Folder(PathBuf::from("/home/me/")),
            name: "下载".to_owned()
        })
    );
    assert_eq!(
        super_box.selected_row(),
        Some(&BoxRow::JumpTo(PathBuf::from("/home/me/下载")))
    );

    super_box.set_input("n:素材/".to_owned(), &context);
    assert_eq!(super_box.mode(), BoxMode::New);
    assert_eq!(
        super_box.rows(),
        [BoxRow::Create(NewEntry::Folder("素材".to_owned()))]
    );
    assert_eq!(super_box.history_text(), "n:素材/");

    let mut search = SuperBox::open(BoxMode::Search, Vec::new(), Vec::new(), &context);
    search.set_input("/tmp".to_owned(), &context);
    assert_eq!(search.mode(), BoxMode::Path);
    assert_eq!(search.input(), "/tmp");
}

#[test]
fn switching_modes_keeps_the_text_and_recomputes_the_rows() {
    let taken = nothing_taken;
    let context = context(&taken);
    let entries = vec![feature("新建文件夹", "new_folder")];
    let mut super_box = SuperBox::open(BoxMode::Search, entries, Vec::new(), &context);
    super_box.set_input("新建".to_owned(), &context);
    assert_eq!(super_box.rows(), [BoxRow::Entry(0)]);
    super_box.next_mode(&context);
    assert_eq!(super_box.mode(), BoxMode::Files);
    assert!(super_box.rows().is_empty(), "nothing found yet");
    super_box.next_mode(&context);
    assert_eq!(super_box.input(), "新建");
    assert!(matches!(super_box.rows(), [BoxRow::JumpTo(_)]));
    super_box.next_mode(&context);
    assert_eq!(
        super_box.rows(),
        [BoxRow::Create(NewEntry::File("新建".to_owned()))]
    );
    super_box.next_mode(&context);
    assert_eq!(super_box.rows(), [BoxRow::Run("新建".to_owned())]);
    super_box.next_mode(&context);
    assert_eq!(super_box.mode(), BoxMode::Search, "it wraps round");
}

#[test]
fn an_empty_box_lists_five_of_all_history_and_three_of_a_mode() {
    let taken = nothing_taken;
    let context = context(&taken);
    let history = ["g:/a", "新建", "g:/b", "s:ls", "g:/c", "g:/d", "s:make"]
        .map(str::to_owned)
        .to_vec();
    let rows = |mode| {
        SuperBox::open(mode, Vec::new(), history.clone(), &context)
            .rows()
            .to_vec()
    };
    assert_eq!(
        rows(BoxMode::Search),
        ["g:/a", "新建", "g:/b", "s:ls", "g:/c"].map(|entry| BoxRow::History(entry.to_owned()))
    );
    // 默认模式就是路径时，打开也没有胶囊：一样是混在一起的 5 条。
    assert_eq!(rows(BoxMode::Path), rows(BoxMode::Search));
    let mut tagged = SuperBox::open(BoxMode::Search, Vec::new(), history.clone(), &context);
    tagged.set_input("g:".to_owned(), &context);
    assert_eq!(
        tagged.rows(),
        ["g:/a", "g:/b", "g:/c"].map(|entry| BoxRow::History(entry.to_owned()))
    );
}

#[test]
fn only_a_typed_tag_shows_a_pill_and_tab_in_an_empty_box_lists_the_tags() {
    let taken = nothing_taken;
    let context = context(&taken);
    // 默认模式是命令：直接打字就是命令，没有胶囊。
    let mut super_box = SuperBox::open(BoxMode::Shell, Vec::new(), Vec::new(), &context);
    assert!(!super_box.shows_tag(super_box.mode()));
    super_box.set_input("g:".to_owned(), &context);
    assert!(super_box.shows_tag(super_box.mode()));
    // 退格删掉胶囊，回到默认模式。
    super_box.set_input(String::new(), &context);
    assert_eq!(super_box.mode(), BoxMode::Shell);

    super_box.complete(&context);
    assert_eq!(
        super_box.tab_list(),
        Some(&TabList::Tags {
            tags: vec![BoxMode::Search, BoxMode::Files, BoxMode::Path, BoxMode::New],
            chosen: None
        })
    );
    super_box.complete(&context);
    assert_eq!(super_box.mode(), BoxMode::Search);
    super_box.complete(&context);
    super_box.complete(&context);
    assert_eq!(super_box.mode(), BoxMode::Path);
    // 打字就收起，接着在这个模式里打。
    super_box.set_input("~/".to_owned(), &context);
    assert!(super_box.tab_list().is_none());
    assert_eq!(super_box.mode(), BoxMode::Path);
}

#[test]
fn backspace_in_an_empty_box_removes_the_mode() {
    let taken = nothing_taken;
    let context = context(&taken);
    let mut super_box = SuperBox::open(BoxMode::Search, Vec::new(), Vec::new(), &context);
    super_box.set_input("s:".to_owned(), &context);
    assert_eq!(super_box.mode(), BoxMode::Shell);
    super_box.set_input("l".to_owned(), &context);
    // 删掉最后一个字：胶囊还在。
    super_box.set_input(String::new(), &context);
    assert_eq!(super_box.mode(), BoxMode::Shell);
    // 空着再退格（输入框又报一次空的）：胶囊没了。
    super_box.set_input(String::new(), &context);
    assert_eq!(super_box.mode(), BoxMode::Search);
}

fn candidate(name: &str, is_dir: bool) -> Candidate {
    Candidate {
        name: name.to_owned(),
        is_dir,
    }
}

/// 按框里这次的查询把候选交回去（应用层在后台列出来的样子）。
fn list(super_box: &mut SuperBox, candidates: Vec<Candidate>, context: &BoxContext<'_>) {
    super_box.accept_listing(
        super_box.generation(),
        Listing {
            folder: Some(PathBuf::from("/home/me/work")),
            candidates,
            truncated: false,
        },
        context,
    );
}

#[test]
fn tab_completes_and_then_goes_through_the_choices() {
    let taken = nothing_taken;
    let context = context(&taken);
    let mut super_box = SuperBox::open(BoxMode::Shell, Vec::new(), Vec::new(), &context);
    super_box.set_input("cd Do".to_owned(), &context);
    assert_eq!(super_box.complete(&context), None, "nothing listed yet");
    list(
        &mut super_box,
        vec![candidate("Documents", true), candidate("Downloads", true)],
        &context,
    );
    super_box.complete(&context);
    assert_eq!(super_box.input(), "cd Do", "nothing more is shared");
    let Some(TabList::Names(choices)) = super_box.tab_list() else {
        panic!("names are listed");
    };
    assert_eq!(choices.candidates.len(), 2);
    super_box.complete(&context);
    assert_eq!(super_box.input(), "cd Documents/");
    super_box.complete(&context);
    assert_eq!(super_box.input(), "cd Downloads/");
    assert!(matches!(
        super_box.tab_list(),
        Some(TabList::Names(choices)) if choices.chosen == Some(1)
    ));
    // 打字就收起。
    super_box.set_input("cd Downloads/x".to_owned(), &context);
    assert!(super_box.tab_list().is_none());

    super_box.set_input("cd Doc".to_owned(), &context);
    list(&mut super_box, vec![candidate("Documents", true)], &context);
    super_box.complete(&context);
    assert_eq!(super_box.input(), "cd Documents/");
}

#[test]
fn the_faded_suggestion_comes_from_history_first_then_from_the_folder() {
    let taken = nothing_taken;
    let context = context(&taken);
    let history = vec!["g:~/下载/新的".to_owned(), "s:make install".to_owned()];
    let mut super_box = SuperBox::open(BoxMode::Path, Vec::new(), history, &context);
    super_box.set_input("~/下".to_owned(), &context);
    list(&mut super_box, vec![candidate("下面", true)], &context);
    assert_eq!(super_box.suggestion(), Some("载/新的"));
    super_box.set_input("~/下面".to_owned(), &context);
    assert_eq!(
        super_box.suggestion(),
        None,
        "nothing listed for this input yet"
    );
    list(&mut super_box, vec![candidate("下面的", true)], &context);
    assert_eq!(super_box.suggestion(), Some("的/"));
    super_box.accept_suggestion(&context);
    assert_eq!(super_box.input(), "~/下面的/");

    super_box.set_input("s:make".to_owned(), &context);
    assert_eq!(super_box.suggestion(), Some(" install"));
    super_box.set_input("s:".to_owned(), &context);
    assert_eq!(super_box.suggestion(), None);
}

#[test]
fn stale_suggestions_are_dropped_and_problems_cannot_be_selected() {
    let taken = |name: &str| name == "已有";
    let context = context(&taken);
    let mut super_box = SuperBox::open(BoxMode::Path, Vec::new(), Vec::new(), &context);
    let Some(BoxLookup::List {
        generation: first, ..
    }) = super_box.set_input("~/d".to_owned(), &context)
    else {
        panic!("a path lookup");
    };
    super_box.set_input("~/do".to_owned(), &context);
    let listing = |names: &[&str]| Listing {
        folder: Some(PathBuf::from("/home/me")),
        candidates: names.iter().map(|name| candidate(name, true)).collect(),
        truncated: false,
    };
    super_box.accept_listing(first, listing(&["desktop"]), &context);
    assert_eq!(super_box.rows().len(), 1, "the older lookup is ignored");
    super_box.accept_listing(
        super_box.generation(),
        listing(&["documents", "downloads"]),
        &context,
    );
    assert_eq!(
        super_box.rows()[1..],
        [
            BoxRow::JumpSuggestion(PathBuf::from("/home/me/documents")),
            BoxRow::JumpSuggestion(PathBuf::from("/home/me/downloads"))
        ]
    );
    super_box.select_next();
    super_box.select_next();
    super_box.select_next();
    assert_eq!(super_box.selected(), Some(0), "selection wraps round");
    super_box.select_previous();
    assert_eq!(super_box.selected(), Some(2));

    super_box.set_input("n:已有".to_owned(), &context);
    assert!(matches!(super_box.rows(), [BoxRow::Problem(_)]));
    assert_eq!(super_box.selected(), None);
    super_box.select_next();
    super_box.select(0);
    assert_eq!(super_box.selected(), None);
}

#[test]
fn nothing_is_created_where_creating_is_not_possible() {
    let taken = nothing_taken;
    let context = BoxContext {
        cannot_create: Some("回收站里不能新建".to_owned()),
        ..context(&taken)
    };
    let mut super_box = SuperBox::open(BoxMode::New, Vec::new(), Vec::new(), &context);
    super_box.set_input("a.txt".to_owned(), &context);
    assert_eq!(
        super_box.rows(),
        [BoxRow::Problem("回收站里不能新建".to_owned())]
    );
}

#[test]
fn the_history_keeps_a_hundred_distinct_entries() {
    let mut history = SuperBoxHistory::default();
    for index in 0..150 {
        history.record_submission(&format!("g:/tmp/{index}"));
    }
    history.record_submission("g:/tmp/120");
    assert_eq!(history.entries().len(), 100);
    assert_eq!(history.entries()[0], "g:/tmp/120");
    assert_eq!(history.entries()[1], "g:/tmp/149");
}

fn find(text: &str) -> super::find::FindQuery {
    super::find::read_find_query(text).unwrap_or_else(|reason| panic!("{reason}"))
}

#[test]
fn filters_are_read_from_anywhere_in_both_languages() {
    use crate::model::{SearchDatePreset, SearchEntryTypePreset, SearchSizePreset};
    let query = find("报告 类型:图片,video time:7d 季度 size:>10m");
    assert_eq!(query.terms, "报告 季度");
    assert_eq!(
        query.filters.selected_entry_types,
        [SearchEntryTypePreset::Images, SearchEntryTypePreset::Video]
    );
    assert_eq!(query.filters.date_preset, SearchDatePreset::PastSevenDays);
    assert_eq!(query.filters.size_preset, SearchSizePreset::Custom);
    assert_eq!(query.filters.custom_size_min, "10m");
    // 算出来的查询条件就是搜索页对同一个筛选状态算出来的那份。
    let page = query
        .filters
        .query_filters_at(chrono::Local::now())
        .unwrap();
    assert_eq!(page.entry_type_rules, query.search_filters.entry_type_rules);
    assert_eq!(page.size, query.search_filters.size);
    assert!(query.search_filters.modified.is_some());

    let wide = find("类型：文件夹 时间:今天 大小:1m-1g");
    assert_eq!(wide.terms, "");
    assert_eq!(
        wide.filters.selected_entry_types,
        [SearchEntryTypePreset::Folders]
    );
    assert_eq!(wide.filters.date_preset, SearchDatePreset::Today);
    assert_eq!(
        (
            wide.filters.custom_size_min.as_str(),
            wide.filters.custom_size_max.as_str()
        ),
        ("1m", "1g")
    );
    // 不是筛选名的冒号是要找的词。
    assert_eq!(find("C:报告 note:1").terms, "C:报告 note:1");
}

#[test]
fn a_wrong_filter_says_what_can_be_written() {
    use super::find::read_find_query;
    assert!(read_find_query("x type:pictures").is_err());
    assert!(read_find_query("x time:soon").is_err());
    assert!(read_find_query("x size:big").is_err());
    assert!(
        read_find_query("x size:>lots").is_err(),
        "the search page's size reader refuses it"
    );
}

#[test]
fn the_selection_reaches_the_shell_as_arguments() {
    let items = [PathBuf::from("/w/a b.txt"), PathBuf::from("/w/$(x)")];
    let posix = shell_invocation(Path::new("/bin/bash"), "ls \"$@\"", &items, Path::new("/w"));
    assert_eq!(posix.program, "/bin/bash");
    assert_eq!(
        posix.arguments,
        ["-c", "ls \"$@\"", "bennu", "/w/a b.txt", "/w/$(x)"]
    );
    assert_eq!(posix.working_directory, PathBuf::from("/w"));
    let fish = shell_invocation(
        Path::new("/usr/bin/fish"),
        "ls $argv",
        &items,
        Path::new("/w"),
    );
    assert_eq!(fish.arguments, ["-c", "ls $argv", "/w/a b.txt", "/w/$(x)"]);
}

fn hit(name: &str) -> file_search::SearchHit {
    file_search::SearchHit {
        path: PathBuf::from("/home/me").join(name),
        display_name: name.to_owned(),
        kind: file_search::SearchFileKind::File,
        size: 0,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        rank: 1.0,
        snippet: None,
        match_source: file_search::MatchSource::Name,
    }
}

#[test]
fn index_results_follow_the_other_entries_and_stale_ones_are_dropped() {
    let taken = nothing_taken;
    let context = context(&taken);
    let entries = vec![feature("Search", "search")];
    let mut super_box = SuperBox::open(BoxMode::Search, entries, Vec::new(), &context);
    let Some(BoxLookup::Find {
        generation: first,
        text_scope,
        limit,
        ..
    }) = super_box.set_input("sea".to_owned(), &context)
    else {
        panic!("searching also asks the index");
    };
    assert_eq!(text_scope, file_search::SearchTextScope::NameOnly);
    assert_eq!(limit, 5);
    super_box.set_input("sear".to_owned(), &context);
    super_box.accept_found(first, Ok(vec![hit("old.txt")]), &context);
    assert_eq!(
        super_box.rows(),
        [BoxRow::Entry(0)],
        "the older result is ignored"
    );
    super_box.accept_found(
        super_box.generation(),
        Ok(vec![hit("seashell.md")]),
        &context,
    );
    assert_eq!(super_box.rows(), [BoxRow::Entry(0), BoxRow::Found(0)]);

    super_box.set_input("f:sea type:pdf".to_owned(), &context);
    assert_eq!(super_box.find_status(), &FindStatus::Searching);
    super_box.accept_found(
        super_box.generation(),
        Err("not running".to_owned()),
        &context,
    );
    assert_eq!(
        super_box.rows(),
        [BoxRow::Problem("not running".to_owned())]
    );
}

#[test]
fn a_command_run_belongs_to_its_session() {
    let taken = nothing_taken;
    let context = context(&taken);
    let mut super_box = SuperBox::open(BoxMode::Shell, Vec::new(), Vec::new(), &context);
    super_box.set_input("make".to_owned(), &context);
    assert_eq!(super_box.history_text(), "s:make");
    super_box.start_run(7, "make".to_owned());
    super_box.finish_run(8, RunEnd::Failed("other".to_owned()));
    assert_eq!(super_box.run().unwrap().state, RunState::Running);
    super_box.finish_run(
        7,
        RunEnd::Exited {
            exit: Exit::Code(0),
            output: ShownOutput::default(),
        },
    );
    assert!(matches!(
        super_box.run().unwrap().state,
        RunState::Ended(RunEnd::Exited { .. })
    ));

    let nowhere = BoxContext {
        current_dir: None,
        ..super::tests::context(&taken)
    };
    super_box.set_input("make".to_owned(), &nowhere);
    assert!(matches!(super_box.rows(), [BoxRow::Problem(_)]));
}
