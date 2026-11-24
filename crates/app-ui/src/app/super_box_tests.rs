use std::path::Path;

use super::*;
use crate::actions::type_filter::TypeFilter;
use crate::actions::{
    ActionDefinition, ActionId, ActionLibrary, ActionNotice, ActionStep, PathTemplate,
};
use crate::app::task_driving_tests::drive;
use crate::config;
use crate::model::BrowserLocation;
use crate::super_box::{BoxMode, Exit, RunEnd, RunState};
use crate::terminal_panel::session::SessionId;
use iced::futures::{Stream, StreamExt};

/// 窗口在前台：普通操作的完成通知不弹。
fn browser_in(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    browser
}

async fn send(browser: &mut FileBrowser, message: SuperBoxMessage) {
    let task = browser.update(Message::SuperBox(message));
    drive(browser, task).await;
}

async fn press(
    browser: &mut FileBrowser,
    key: iced::keyboard::key::Named,
    modifiers: iced::keyboard::Modifiers,
) {
    let task = browser
        .super_box_key(&iced::keyboard::Key::Named(key), modifiers)
        .expect("the box takes the key");
    drive(browser, task).await;
}

async fn enter(browser: &mut FileBrowser) {
    press(
        browser,
        iced::keyboard::key::Named::Enter,
        iced::keyboard::Modifiers::empty(),
    )
    .await;
}

async fn type_and_run(browser: &mut FileBrowser, text: &str) {
    if browser.super_box.is_none() {
        drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    }
    send(browser, SuperBoxMessage::InputChanged(text.to_owned())).await;
    enter(browser).await;
}

#[test]
fn the_shortcut_opens_and_closes_the_box_in_the_default_mode() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    browser.user_config.super_box_mode = BoxMode::Path;
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::Path)
    );
    assert!(
        browser.main_window_dialog_is_open(),
        "file-area keys are off"
    );

    // 框开着时 Shift+Tab 换模式（Tab 留给补全），打开它的键关掉它。
    let tab = iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab);
    drop(browser.super_box_key(&tab, iced::keyboard::Modifiers::SHIFT));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::New)
    );
    drop(browser.super_box_key(&tab, iced::keyboard::Modifiers::empty()));
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::New),
        "Tab completes instead"
    );
    let space = iced::keyboard::Key::Named(iced::keyboard::key::Named::Space);
    drop(browser.super_box_key(&space, iced::keyboard::Modifiers::ALT));
    assert!(browser.super_box.is_none());

    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    drop(browser.update(Message::DismissFloating));
    assert!(browser.super_box.is_none());
}

#[tokio::test]
async fn a_feature_runs_like_its_shortcut_and_is_remembered() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    let visible = browser.sidebar_visible();

    type_and_run(&mut browser, "toggle sidebar").await;

    assert!(browser.super_box.is_none());
    assert_ne!(browser.sidebar_visible(), visible);
    assert_eq!(
        browser.user_config.super_box_history.entries(),
        ["toggle sidebar"]
    );
}

#[tokio::test]
async fn an_action_runs_on_the_current_folder() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let mut browser = browser_in(&work);
    browser.action_library = ActionLibrary::from_definitions(vec![ActionDefinition {
        id: ActionId::from_file_stem("tidy"),
        name: "收进归档".to_owned(),
        file: root.join("tidy.toml"),
        types: TypeFilter::Anything,
        steps: vec![ActionStep::MoveTo(
            PathTemplate::parse("{dir}/归档").unwrap(),
        )],
        notice: ActionNotice::Silent,
        watch: None,
    }]);

    type_and_run(&mut browser, "sjgd").await;

    assert!(
        root.join("归档/work").is_dir(),
        "the folder was moved by the action"
    );
}

#[tokio::test]
async fn a_settings_page_opens_the_settings_window_there() {
    let directory = tempfile::tempdir().unwrap();
    let mut browser = browser_in(directory.path());
    type_and_run(&mut browser, "shortcuts").await;
    assert!(browser.settings_window.is_some());
    assert_eq!(
        browser.selected_settings_category,
        SettingsCategory::Shortcuts
    );
}

#[tokio::test]
async fn going_to_a_folder_opens_it_and_going_to_a_file_selects_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("资料")).unwrap();
    std::fs::write(root.join("资料/报告.pdf"), b"%PDF").unwrap();
    let mut browser = browser_in(&root);

    type_and_run(&mut browser, "g:资料").await;
    assert!(browser.super_box.is_none());
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(root.join("资料"))
    );
    assert_eq!(browser.user_config.super_box_history.entries(), ["g:资料"]);

    type_and_run(&mut browser, &format!("{}/资料/报告.pdf", root.display())).await;
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(root.join("资料"))
    );
    assert!(browser.is_path_selected(&root.join("资料/报告.pdf")));
}

#[tokio::test]
async fn a_path_that_is_not_there_keeps_the_box_open_and_says_so() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);
    type_and_run(&mut browser, "g:没有这个").await;
    let super_box = browser.super_box.as_ref().expect("the box stays open");
    assert!(super_box.notice().is_some());
    assert_eq!(browser.tab().location, BrowserLocation::Directory(root));
    assert!(browser.user_config.super_box_history.entries().is_empty());
}

#[tokio::test]
async fn new_items_are_made_with_their_names_selected_and_undoable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);

    type_and_run(&mut browser, "n:笔记.md").await;
    assert!(root.join("笔记.md").is_file());
    assert!(browser.is_path_selected(&root.join("笔记.md")));
    assert!(browser.renaming.is_none(), "the name was already given");

    type_and_run(&mut browser, "n：素材/").await;
    assert!(root.join("素材").is_dir());
    assert_eq!(browser.operation_history.undo_depth(), 2);
    assert_eq!(
        browser.user_config.super_box_history.entries(),
        ["n:素材/", "n:笔记.md"]
    );
}

#[tokio::test]
async fn a_taken_name_is_refused_before_anything_is_queued() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("已有.txt"), b"x").unwrap();
    let mut browser = browser_in(&root);
    let load = browser.navigate_to(root.clone(), crate::model::NavigationMode::RecordHistory);
    drive(&mut browser, load).await;

    type_and_run(&mut browser, "n:已有.txt").await;
    assert!(browser.operation_queue.tasks().is_empty());
    assert!(browser.super_box.is_some());
}

#[tokio::test]
async fn a_history_row_fills_the_box_instead_of_running() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_in(&root);
    browser
        .user_config
        .super_box_history
        .record_submission("n:不要马上建.txt");
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    // 没有模式胶囊时各种模式的历史混在一起。
    enter(&mut browser).await;
    let super_box = browser.super_box.as_ref().unwrap();
    assert_eq!(super_box.mode(), BoxMode::New);
    assert_eq!(super_box.input(), "不要马上建.txt");
    assert!(!root.join("不要马上建.txt").exists());
}

/// 列出这个文件夹（选中项要在列表里），shell 用 /bin/sh。
async fn browser_listing(directory: &Path) -> FileBrowser {
    let mut browser = browser_in(directory);
    browser.user_config.terminal_shell = "/bin/sh".to_owned();
    let load = browser.navigate_to(
        directory.to_path_buf(),
        crate::model::NavigationMode::RecordHistory,
    );
    drive(&mut browser, load).await;
    browser
}

fn run_state(browser: &FileBrowser) -> crate::super_box::RunState {
    browser
        .super_box
        .as_ref()
        .and_then(SuperBox::run)
        .map(|run| run.state.clone())
        .expect("a command ran")
}

fn exited(browser: &FileBrowser) -> (Exit, String) {
    match run_state(browser) {
        RunState::Ended(RunEnd::Exited { exit, output }) => (exit, output.text),
        other => panic!("{other:?}"),
    }
}

/// 测试里没有订阅：用订阅的那条事件流把终端会话的事交给浏览器。转到终端后同一条流接着读。
struct CommandOutput(std::pin::Pin<Box<dyn Stream<Item = Message>>>);

impl CommandOutput {
    /// 框里最近开始的那条命令，和浏览器的事件流。整个应用只有一条事件流：同一个浏览器上
    /// 后面的命令用 [`last_command`] 取会话号，流接着用。
    fn of_the_last_command(browser: &FileBrowser) -> (SessionId, Self) {
        (
            last_command(browser),
            Self(Box::pin(browser.terminal_panel.events.stream())),
        )
    }

    async fn deliver_until(
        &mut self,
        browser: &mut FileBrowser,
        done: impl Fn(&FileBrowser) -> bool,
    ) {
        while !done(browser) {
            let message = tokio::time::timeout(std::time::Duration::from_secs(20), self.0.next())
                .await
                .expect("the command hung")
                .expect("output keeps coming until the end");
            let task = browser.update(message);
            drive(browser, task).await;
        }
    }

    /// 读到命令结束、等它退出、结果交回去为止。
    async fn deliver_to_the_end(&mut self, browser: &mut FileBrowser, session: SessionId) {
        self.deliver_until(browser, |browser| {
            browser.hidden_terminal_session(session).is_none()
        })
        .await;
    }
}

fn last_command(browser: &FileBrowser) -> SessionId {
    browser
        .terminal_panel
        .hidden_tabs
        .last()
        .expect("a command is running")
        .session
        .id
}

async fn run_to_the_end(browser: &mut FileBrowser, text: &str) {
    type_and_run(browser, text).await;
    let (session, mut output) = CommandOutput::of_the_last_command(browser);
    output.deliver_to_the_end(browser, session).await;
}

#[tokio::test]
async fn a_command_gets_the_selection_as_arguments_and_shows_its_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let tricky = root.join("a $(touch pwned) `touch pwned2`.txt");
    std::fs::write(&tricky, b"x").unwrap();
    let mut browser = browser_listing(&root).await;
    browser.select_path(tricky.clone());

    run_to_the_end(
        &mut browser,
        "s:printf '%s' \"$1\" > got.txt; echo \"$0 has $#\"; echo careful >&2",
    )
    .await;

    assert_eq!(
        std::fs::read_to_string(root.join("got.txt")).unwrap(),
        tricky.to_str().unwrap()
    );
    assert!(!root.join("pwned").exists() && !root.join("pwned2").exists());
    // 标准输出和标准错误都在终端里，按出现的顺序。
    assert_eq!(
        exited(&browser),
        (Exit::Code(0), "bennu has 1\ncareful".to_owned())
    );
    assert_eq!(
        browser.user_config.super_box_history.entries()[0],
        "s:printf '%s' \"$1\" > got.txt; echo \"$0 has $#\"; echo careful >&2"
    );
    assert_eq!(
        browser.operation_history.undo_depth(),
        0,
        "commands are not undone"
    );
    assert!(
        browser.operation_queue.tasks().is_empty(),
        "commands do not go through the task queue"
    );
}

#[tokio::test]
async fn a_failing_command_shows_its_exit_status_and_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    run_to_the_end(&mut browser, "s:echo broken >&2; exit 3").await;
    assert_eq!(exited(&browser), (Exit::Code(3), "broken".to_owned()));
}

#[tokio::test]
async fn closing_the_box_lets_the_command_finish() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    type_and_run(&mut browser, "s:sleep 0.2; touch late").await;
    let (session, mut output) = CommandOutput::of_the_last_command(&browser);
    drop(browser.update(Message::DismissFloating));
    assert!(browser.super_box.is_none());
    output.deliver_to_the_end(&mut browser, session).await;
    assert!(root.join("late").exists());
}

#[tokio::test]
async fn stop_ends_the_command_and_kills_it_when_it_ignores_that() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    type_and_run(&mut browser, "s:echo started; sleep 30").await;
    let (session, mut output) = CommandOutput::of_the_last_command(&browser);
    output
        .deliver_until(&mut browser, |browser| {
            browser
                .hidden_terminal_session(session)
                .is_some_and(|running| running.emulator.last_lines(1) == "started")
        })
        .await;
    // 不等两秒后的强行结束：SIGTERM 就够了。
    drop(browser.update(Message::SuperBox(SuperBoxMessage::StopPressed)));
    output.deliver_to_the_end(&mut browser, session).await;
    assert!(matches!(exited(&browser), (Exit::Signal(_), output) if output == "started"));

    // 忽略 SIGTERM 的（子进程也跟着忽略），等够了就强行结束。
    type_and_run(&mut browser, "s:trap '' TERM; echo started; sleep 30").await;
    let session = last_command(&browser);
    output
        .deliver_until(&mut browser, |browser| {
            browser
                .hidden_terminal_session(session)
                .is_some_and(|running| running.emulator.last_lines(1) == "started")
        })
        .await;
    drop(browser.update(Message::SuperBox(SuperBoxMessage::StopPressed)));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(browser.hidden_terminal_session(session).is_some());
    drop(browser.update(Message::SuperBox(SuperBoxMessage::StopGraceEnded(session))));
    output.deliver_to_the_end(&mut browser, session).await;
    assert!(matches!(exited(&browser), (Exit::Signal(_), _)));
}

/// 全屏界面要用户来操作：整个移进终端面板，接着跑，不跟着文件区换目录（`cd` 不会打进程序里）。
#[tokio::test]
async fn a_full_screen_program_moves_to_the_terminal_panel_and_keeps_running() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("elsewhere")).unwrap();
    let mut browser = browser_listing(&root).await;
    // 列表视图里终端跟着当前文件夹走（多栏视图跟着鼠标指着的那栏，测试里没有鼠标）。
    browser
        .tab_mut()
        .choose_view_mode(crate::model::BrowserViewMode::List);
    type_and_run(
        &mut browser,
        "s:printf '\\033[?1049h'; read answer; printf '%s' \"$answer\" > answer.txt",
    )
    .await;
    let (session, mut output) = CommandOutput::of_the_last_command(&browser);
    output
        .deliver_until(&mut browser, |browser| {
            browser.hidden_terminal_session(session).is_none()
        })
        .await;

    assert!(browser.super_box.is_none(), "the box gets out of the way");
    assert!(browser.terminal_panel.is_expanded() && browser.terminal_panel.is_focused());
    let tab = browser
        .terminal_panel
        .active_tab()
        .expect("it is a tab now");
    assert_eq!(
        (tab.session.id, tab.program),
        (session, crate::terminal_panel::tabs::TabProgram::Command)
    );

    let navigated = browser.navigate_to(
        root.join("elsewhere"),
        crate::model::NavigationMode::RecordHistory,
    );
    drive(&mut browser, navigated).await;
    let y = iced::keyboard::Key::Character("y".into());
    let keys = browser.terminal_panel_keyboard_input(&y, &y, iced::keyboard::Modifiers::empty());
    drive(&mut browser, keys.expect("the terminal takes the keys")).await;
    browser
        .terminal_panel
        .active_tab()
        .unwrap()
        .session
        .write_input(b"es\r");
    output
        .deliver_until(&mut browser, |browser| {
            browser.terminal_panel.tabs.is_empty()
        })
        .await;
    assert_eq!(
        std::fs::read_to_string(root.join("answer.txt")).unwrap(),
        "yes"
    );
    assert!(
        browser
            .super_box_commands
            .past("printf '\\033[?1049h'; read answer; printf '%s' \"$answer\" > answer.txt")
            .is_none(),
        "what it did is in the terminal, not in the box"
    );
}

#[tokio::test]
async fn a_running_command_can_be_moved_to_the_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    type_and_run(&mut browser, "s:printf 'Password: '; read answer").await;
    let (session, mut output) = CommandOutput::of_the_last_command(&browser);
    output
        .deliver_until(&mut browser, |browser| {
            browser
                .hidden_terminal_session(session)
                .is_some_and(|running| running.emulator.last_lines(1) == "Password:")
        })
        .await;

    send(&mut browser, SuperBoxMessage::ShowInTerminalPressed).await;

    assert!(browser.super_box.is_none());
    assert_eq!(
        browser
            .terminal_panel
            .active_tab()
            .map(|tab| tab.session.id),
        Some(session)
    );
    browser
        .terminal_panel
        .active_tab()
        .unwrap()
        .session
        .write_input(b"secret\r");
    output
        .deliver_until(&mut browser, |browser| {
            browser.terminal_panel.tabs.is_empty()
        })
        .await;
    assert!(
        !browser.terminal_panel.is_expanded(),
        "it closes like any tab"
    );
}

#[tokio::test]
async fn ctrl_enter_takes_the_words_and_filters_to_the_search_page() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    send(
        &mut browser,
        SuperBoxMessage::InputChanged("f:季度报告 type:pdf".to_owned()),
    )
    .await;
    let enter = iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter);
    let task = browser
        .super_box_key(&enter, iced::keyboard::Modifiers::CTRL)
        .unwrap();
    drive(&mut browser, task).await;

    assert!(browser.super_box.is_none());
    let search = browser.active_search().expect("the search page is open");
    assert_eq!(search.input, "季度报告");
    assert_eq!(
        search.filters.selected_entry_types,
        [crate::model::SearchEntryTypePreset::Pdf]
    );
    assert_eq!(
        search.root.selected_scope(),
        crate::model::SearchDirectoryScope::AllIndexedLocations
    );
}

#[tokio::test]
async fn a_dialog_that_pops_up_over_the_box_gets_the_keys_and_closes_first() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    send(
        &mut browser,
        SuperBoxMessage::InputChanged("s:make".to_owned()),
    )
    .await;
    // 后台事件弹出的对话框（这里是打开失败后的“打开方式”）盖在框上面。
    drop(browser.update(Message::OpenFileFinished(
        root.join("a.txt"),
        Err("no application".to_owned()),
    )));
    assert!(browser.open_with.is_some());

    let tab = iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab);
    assert!(
        browser
            .super_box_key(&tab, iced::keyboard::Modifiers::empty())
            .is_none(),
        "the hidden box does not take the keys"
    );
    assert_eq!(
        browser.super_box.as_ref().map(SuperBox::mode),
        Some(BoxMode::Shell)
    );

    drop(browser.update(Message::DismissFloating));
    assert!(
        browser.open_with.is_none(),
        "the dialog on top closes first"
    );
    assert!(browser.super_box.is_some(), "the box comes back");
    drop(browser.update(Message::DismissFloating));
    assert!(browser.super_box.is_none());
}

/// Ctrl+Enter 只做一件事：有结果时也不先执行选中的结果（虚拟机里查到过先跑普通回车）。
#[tokio::test]
async fn ctrl_enter_does_not_also_run_the_selected_result() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("找到的")).unwrap();
    let mut browser = browser_listing(&root).await;
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    send(
        &mut browser,
        SuperBoxMessage::InputChanged("f:找到".to_owned()),
    )
    .await;
    let generation = browser.super_box.as_ref().unwrap().generation();
    let hit = file_search::SearchHit {
        path: root.join("找到的"),
        display_name: "找到的".to_owned(),
        kind: file_search::SearchFileKind::Directory,
        size: 0,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        rank: 1.0,
        snippet: None,
        match_source: file_search::MatchSource::Name,
    };
    drop(browser.accept_super_box_found(
        generation,
        crate::model::IndexedSearchOutcome::Batch(file_search::SearchResultBatch {
            query_id: generation,
            hits: vec![hit],
            next_cursor: None,
            finished: true,
        }),
    ));
    assert!(matches!(
        browser.super_box.as_ref().unwrap().selected_row(),
        Some(crate::super_box::BoxRow::Found(0))
    ));

    press(
        &mut browser,
        iced::keyboard::key::Named::Enter,
        iced::keyboard::Modifiers::CTRL,
    )
    .await;

    assert!(browser.active_search().is_some(), "the search page is open");
    assert_eq!(browser.user_config.super_box_history.entries(), ["f:找到"]);
}

fn file_hit(path: &Path) -> file_search::SearchHit {
    file_search::SearchHit {
        path: path.to_path_buf(),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
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

/// 在框里“找到”这个文件并按回车（不走搜索服务，结果直接交给框）。
async fn open_found(browser: &mut FileBrowser, file: &Path) {
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    send(browser, SuperBoxMessage::InputChanged("f:x".to_owned())).await;
    let generation = browser.super_box.as_ref().unwrap().generation();
    drop(browser.accept_super_box_found(
        generation,
        crate::model::IndexedSearchOutcome::Batch(file_search::SearchResultBatch {
            query_id: generation,
            hits: vec![file_hit(file)],
            next_cursor: None,
            finished: true,
        }),
    ));
    enter(browser).await;
}

fn nested_tree() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("work/proj/src")).unwrap();
    std::fs::create_dir_all(root.join("work/docs")).unwrap();
    std::fs::write(root.join("work/proj/src/main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join("work/docs/a.md"), b"# a").unwrap();
    (directory, root)
}

#[tokio::test]
async fn in_columns_a_found_file_opens_column_by_column_from_the_first_one() {
    let (_directory, root) = nested_tree();
    let mut browser = browser_listing(&root.join("work")).await;
    assert_eq!(
        browser.tab().view_mode(),
        crate::model::BrowserViewMode::Columns
    );

    open_found(&mut browser, &root.join("work/proj/src/main.rs")).await;

    assert!(browser.super_box.is_none());
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        [
            root.join("work"),
            root.join("work/proj"),
            root.join("work/proj/src")
        ]
    );
    assert!(browser.is_path_selected(&root.join("work/proj/src/main.rs")));
    assert!(browser
        .entry_for_path(&root.join("work/proj/src/main.rs"))
        .is_some());
}

#[tokio::test]
async fn in_columns_a_file_elsewhere_opens_from_the_common_folder() {
    let (_directory, root) = nested_tree();
    let mut browser = browser_listing(&root.join("work/proj")).await;

    open_found(&mut browser, &root.join("work/docs/a.md")).await;

    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        [root.join("work"), root.join("work/docs")]
    );
    assert!(browser.is_path_selected(&root.join("work/docs/a.md")));
}

#[tokio::test]
async fn the_setting_can_make_its_folder_the_first_column() {
    let (_directory, root) = nested_tree();
    let mut browser = browser_listing(&root.join("work")).await;
    browser.user_config.super_box_found_in_columns =
        crate::super_box::FoundInColumns::FolderAsFirstColumn;

    open_found(&mut browser, &root.join("work/proj/src/main.rs")).await;

    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        [root.join("work/proj/src")]
    );
    assert!(browser.is_path_selected(&root.join("work/proj/src/main.rs")));
}

#[tokio::test]
async fn other_views_open_the_folder_and_select_the_file() {
    let (_directory, root) = nested_tree();
    let mut browser = browser_listing(&root.join("work")).await;
    drop(browser.select_browser_view_mode(
        browser.active_pane_id(),
        crate::model::BrowserViewMode::List,
    ));
    // 多栏视图的设置在别的视图里不起作用。
    browser.user_config.super_box_found_in_columns = crate::super_box::FoundInColumns::OpenFile;

    open_found(&mut browser, &root.join("work/proj/src/main.rs")).await;

    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(root.join("work/proj/src"))
    );
    assert!(browser.is_path_selected(&root.join("work/proj/src/main.rs")));
}

#[tokio::test]
async fn a_command_result_is_kept_even_when_the_box_was_closed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut browser = browser_listing(&root).await;
    type_and_run(&mut browser, "s:echo kept").await;
    let (session, mut output) = CommandOutput::of_the_last_command(&browser);
    drop(browser.update(Message::DismissFloating));
    output.deliver_to_the_end(&mut browser, session).await;

    let past = browser
        .super_box_commands
        .past("echo kept")
        .expect("the result is remembered");
    assert_eq!(
        past.end,
        RunEnd::Exited {
            exit: Exit::Code(0),
            output: crate::super_box::ShownOutput::from_text("kept".to_owned()),
        }
    );

    // 框重新打开，混在一起的历史里第一条就是它（选中它，下面显示它的输出）。
    drop(browser.invoke_shortcut(ShortcutAction::OpenSuperBox));
    assert_eq!(
        browser.super_box.as_ref().unwrap().selected_row(),
        Some(&crate::super_box::BoxRow::History("s:echo kept".to_owned()))
    );
}
