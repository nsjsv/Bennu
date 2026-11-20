use super::*;
use crate::actions::{ConvertFormat, PathTemplate, TrashScope};

fn actions() -> Vec<PipeAction> {
    vec![
        PipeAction {
            name: "提取音频".to_owned(),
            id: ActionId::from_file_stem("extract-audio"),
            ends_the_pipe: false,
        },
        PipeAction {
            name: "用 GIMP 打开".to_owned(),
            id: ActionId::service_menu("gimp.desktop", "open"),
            ends_the_pipe: true,
        },
    ]
}

fn pipe(mode: BoxMode, text: &str) -> Result<Pipe, String> {
    read_pipe(mode, text, &actions()).expect("a pipe")
}

#[test]
fn only_a_bar_outside_quotes_makes_a_pipe_and_only_in_search_and_files() {
    let actions = actions();
    assert_eq!(read_pipe(BoxMode::Search, "提取音频", &actions), None);
    assert_eq!(read_pipe(BoxMode::Shell, "ls | wc", &actions), None);
    assert_eq!(read_pipe(BoxMode::Path, "/a|b", &actions), None);
    assert_eq!(read_pipe(BoxMode::Search, "'a|b'", &actions), None);
    assert_eq!(read_pipe(BoxMode::Search, "a\\|b", &actions), None);
    let run = pipe(BoxMode::Search, "| 执行 sh -c 'sort | uniq' _ {files}").unwrap();
    assert_eq!(run.stages.len(), 1, "the quoted bar belongs to the command");
}

#[test]
fn the_source_is_the_selection_a_name_pattern_or_an_index_query() {
    assert_eq!(
        pipe(BoxMode::Search, " | 复制路径").unwrap().source,
        PipeSource::Selection
    );
    assert_eq!(
        pipe(BoxMode::Search, "*.mp4 | 复制路径").unwrap().source,
        PipeSource::Glob("*.mp4".to_owned())
    );
    assert!(pipe(BoxMode::Search, "a/*.mp4 | 复制路径")
        .unwrap_err()
        .contains("f:"));
    let PipeSource::Find(query) = pipe(BoxMode::Files, "会议 类型:视频 | 复制路径")
        .unwrap()
        .source
    else {
        panic!("an index query");
    };
    assert_eq!(query.terms, "会议");
    assert!(pipe(BoxMode::Files, "类型:视频 | 复制路径").is_err());
}

#[test]
fn steps_read_like_the_action_file_in_chinese_or_english() {
    let run = pipe(
        BoxMode::Search,
        "*.mp4 | 转换 m4a | move_to {dir}/音频 | 压缩 zip | 解压 | extract folder | 复制路径 | trash selected",
    )
    .unwrap();
    assert_eq!(
        run.stages,
        [
            Stage::Step(ActionStep::Convert(ConvertFormat::named("m4a").unwrap())),
            Stage::Step(ActionStep::MoveTo(
                PathTemplate::parse("{dir}/音频").unwrap()
            )),
            Stage::Step(ActionStep::Compress(file_core::ArchiveFormat::Zip)),
            Stage::Step(ActionStep::Extract(
                file_core::ArchiveExtractionPlacement::Smart
            )),
            Stage::Step(ActionStep::Extract(
                file_core::ArchiveExtractionPlacement::ArchiveFolder
            )),
            Stage::Step(ActionStep::CopyPaths),
            Stage::Step(ActionStep::Trash(TrashScope::Selected)),
        ]
    );
    assert_eq!(run.labels.len(), run.stages.len());
    // 写错的值说的和动作文件里一样。
    assert!(pipe(BoxMode::Search, "| 转换 doc")
        .unwrap_err()
        .contains("doc"));
    assert!(pipe(BoxMode::Search, "| 复制路径 x").is_err());
}

#[test]
fn an_action_is_used_by_its_name_and_some_steps_have_to_be_last() {
    let run = pipe(BoxMode::Search, "*.mov | 提取音频 | 回收站").unwrap();
    assert_eq!(
        run.stages[0],
        Stage::Action(ActionId::from_file_stem("extract-audio"))
    );
    assert_eq!(run.labels[0], "提取音频");
    assert!(pipe(BoxMode::Search, "| 回收站 | 复制路径")
        .unwrap_err()
        .contains("回收站"));
    assert!(pipe(BoxMode::Search, "| 用 gimp 打开").is_ok(), "any case");
    assert!(pipe(BoxMode::Search, "| 用 GIMP 打开 | 复制路径").is_err());
    let unknown = pipe(BoxMode::Search, "| 跳舞").unwrap_err();
    assert!(unknown.contains("跳舞") && unknown.contains("转换"));
    assert!(
        pipe(BoxMode::Search, "*.mp4 |").is_err(),
        "a step after each bar"
    );
}

#[test]
fn the_last_stage_starts_after_the_last_open_bar() {
    assert_eq!(last_stage_start("*.mp4 | 转"), Some(7));
    assert_eq!(last_stage_start("a | 'x|y"), Some(3));
    assert_eq!(last_stage_start("abc"), None);
    assert!(takes_a_folder("移动到") && takes_a_folder("copy_to") && !takes_a_folder("压缩"));
}

mod in_the_box {
    use std::path::{Path, PathBuf};

    use super::actions;
    use crate::super_box::{
        BoxContext, BoxLookup, BoxMode, BoxRow, ListSource, PipeSource, SuperBox, MOST_PICKED,
    };

    fn nothing_taken(_: &str) -> bool {
        false
    }

    fn context(taken: &dyn Fn(&str) -> bool) -> BoxContext<'_> {
        BoxContext {
            current_dir: Some(Path::new("/home/me/work")),
            home: Path::new("/home/me"),
            cannot_create: None,
            taken,
        }
    }

    fn open(context: &BoxContext<'_>) -> SuperBox {
        SuperBox::open(BoxMode::Search, Vec::new(), actions(), Vec::new(), context)
    }

    fn files(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| PathBuf::from(format!("/home/me/work/{index}.mp4")))
            .collect()
    }

    #[test]
    fn the_items_are_picked_once_per_way_of_picking_and_then_the_pipe_can_run() {
        let taken = nothing_taken;
        let context = context(&taken);
        let mut super_box = open(&context);
        let glob = PipeSource::Glob("*.mp4".to_owned());
        assert_eq!(
            super_box.set_input("*.mp4 | 提".to_owned(), &context),
            [BoxLookup::Pick(glob.clone())]
        );
        // 步骤还没打完就说读不懂（同 fish 里打错的命令马上标红）；项照样先挑起来。
        assert!(matches!(super_box.rows(), [BoxRow::Problem(_)]));
        // 只改后面的步骤：不重挑。
        let lookups = super_box.set_input("*.mp4 | 提取音频".to_owned(), &context);
        assert!(!lookups
            .iter()
            .any(|lookup| matches!(lookup, BoxLookup::Pick(_))));

        super_box.accept_picked(&glob, Ok(files(3)), &context);
        assert_eq!(super_box.rows(), [BoxRow::Pipe]);
        assert_eq!(super_box.picked().map(<[_]>::len), Some(3));
        assert_eq!(super_box.pipe().unwrap().labels, ["提取音频"]);

        // 挑法变了：之前那份结果不认。
        super_box.set_input("*.mov | 提取音频".to_owned(), &context);
        super_box.accept_picked(&glob, Ok(files(3)), &context);
        assert!(super_box.picked().is_none());
    }

    #[test]
    fn nothing_too_much_or_a_mistake_is_said_instead_of_running() {
        let taken = nothing_taken;
        let context = context(&taken);
        let mut super_box = open(&context);
        super_box.set_input("| 提取音频".to_owned(), &context);
        super_box.accept_picked(&PipeSource::Selection, Ok(Vec::new()), &context);
        assert!(matches!(super_box.rows(), [BoxRow::Problem(reason)] if reason.contains('|')));

        super_box.set_input("*.mp4 | 提取音频".to_owned(), &context);
        let glob = PipeSource::Glob("*.mp4".to_owned());
        super_box.accept_picked(&glob, Ok(files(MOST_PICKED + 1)), &context);
        assert!(matches!(super_box.rows(), [BoxRow::Problem(reason)] if reason.contains("1000")));
        super_box.accept_picked(&glob, Ok(files(MOST_PICKED)), &context);
        assert_eq!(super_box.rows(), [BoxRow::Pipe], "exactly the most is fine");

        super_box.set_input("*.mp4 | 跳舞".to_owned(), &context);
        assert!(matches!(super_box.rows(), [BoxRow::Problem(reason)] if reason.contains("跳舞")));
        assert_eq!(super_box.selected(), None);
    }

    #[test]
    fn tab_and_the_faded_suggestion_know_the_steps_and_their_folders() {
        let taken = nothing_taken;
        let context = context(&taken);
        let mut super_box = open(&context);
        super_box.set_input("*.mp4 | 转".to_owned(), &context);
        assert_eq!(super_box.suggestion(), Some("换"));
        super_box.complete(&context);
        assert_eq!(super_box.input(), "*.mp4 | 转换 ");
        super_box.set_input("*.mp4 | 提".to_owned(), &context);
        super_box.complete(&context);
        assert_eq!(super_box.input(), "*.mp4 | 提取音频 ");

        // 移动到后面是文件夹：到后台去列。
        let lookups = super_box.set_input("*.mp4 | 移动到 ~/备".to_owned(), &context);
        assert!(lookups.contains(&BoxLookup::List {
            generation: super_box.generation(),
            source: ListSource::Folder(PathBuf::from("/home/me/")),
            name: "备".to_owned(),
        }));
    }
}
