//! 真的启动服务菜单：进程在第一项的文件夹里跑，文件名到它那里还是原样，嵌套命令行的写法被拦下。

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::actions::{ActionId, ActionLibrary};
use crate::app::actions::ActionSubject;
use crate::app::task_driving_tests::drive;
use crate::model::BrowserLocation;

fn service_menu(action: &str, exec: &str) -> ServiceMenuAction {
    ServiceMenuAction::for_tests(action, action, exec)
}

fn browser_in(directory: &Path, menus: Vec<ServiceMenuAction>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.system_focused_window = Some(iced::window::Id::unique());
    drop(browser.handle_action_message(ActionMessage::LibraryLoaded(
        ActionLibrary::from_definitions(Vec::new()).with_service_menus(menus),
    )));
    browser
}

async fn invoke(browser: &mut FileBrowser, action: &str) {
    let task = browser.handle_action_message(ActionMessage::Invoked {
        action: ActionId::service_menu("test.desktop", action),
        subject: ActionSubject::Selection,
    });
    drive(browser, task).await;
}

/// 进程启动以后就不管了，结果要等一会儿才出现在磁盘上。
fn appears(path: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[tokio::test]
async fn a_service_menu_runs_in_the_folder_of_the_item_and_names_stay_data() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let hostile = root.join("$(touch pwned) `touch pwned2`; touch pwned3.txt");
    std::fs::write(&hostile, b"x").unwrap();
    let mut browser = browser_in(
        root,
        vec![service_menu("mark", "touch %f.done here-it-ran")],
    );
    browser.select_path(hostile.clone());

    invoke(&mut browser, "mark").await;

    let mut done = hostile.clone().into_os_string();
    done.push(".done");
    assert!(appears(Path::new(&done)), "the menu ran on the item");
    assert!(appears(&root.join("here-it-ran")), "in the item's folder");
    for injected in ["pwned", "pwned2", "pwned3.txt"] {
        assert!(!root.join(injected).exists(), "{injected} was created");
    }
    assert_eq!(browser.current_error(), None);
}

#[tokio::test]
async fn a_name_handed_on_to_a_second_shell_is_refused_but_ordinary_names_go_through() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let exec = r#"sh -c "touch \"%f.done\"""#;
    let mut browser = browser_in(root, vec![service_menu("nested", exec)]);

    let hostile = root.join("a$(touch pwned).txt");
    std::fs::write(&hostile, b"x").unwrap();
    browser.select_path(hostile.clone());
    invoke(&mut browser, "nested").await;
    let error = browser.current_error().expect("the refusal is reported");
    assert!(error.contains("nested") && error.contains('$'), "{error}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!root.join("pwned").exists());
    assert!(!root.join("a$(touch pwned).txt.done").exists());

    let ordinary = root.join("my clip (1) it's.txt");
    std::fs::write(&ordinary, b"x").unwrap();
    browser.select_path(ordinary);
    invoke(&mut browser, "nested").await;
    assert!(appears(&root.join("my clip (1) it's.txt.done")));
}

#[tokio::test]
async fn a_missing_program_is_reported_instead_of_failing_silently() {
    let directory = tempfile::tempdir().unwrap();
    let item = directory.path().join("note.txt");
    std::fs::write(&item, b"x").unwrap();
    let mut browser = browser_in(
        directory.path(),
        vec![service_menu("ghost", "bennu-no-such-program-here %f")],
    );
    browser.select_path(item);

    invoke(&mut browser, "ghost").await;

    let error = browser.current_error().expect("an error is shown");
    assert!(error.contains("bennu-no-such-program-here"), "{error}");
}

#[test]
fn programs_are_found_on_the_path_or_at_their_absolute_location() {
    assert!(check_program("sh").is_ok());
    assert!(check_program("/bin/sh").is_ok());
    assert!(check_program("./relative-is-not-checked").is_ok());
    assert!(check_program("bennu-no-such-program-here").is_err());
    let directory = tempfile::tempdir().unwrap();
    let plain = directory.path().join("plain");
    std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
    let error = check_program(plain.to_str().unwrap()).unwrap_err();
    assert!(error.contains("plain"), "{error}");
}
