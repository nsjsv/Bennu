//! 预览窗里的预览键关窗口，与事件有没有被窗内控件捕获无关。内容路由在辅助窗口里被启用门挡掉，
//! 没被捕获的 Space 若交给它，会被静默丢弃（2026-11-19 键位重写后的回归）。

use iced::event::Status;
use iced::futures::StreamExt;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::{window, Task};
use iced_runtime::Action;

use super::FileBrowser;
use crate::config;
use crate::model::{Message, PreviewState};

fn browser_with_preview_window() -> (FileBrowser, window::Id) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let path = std::path::PathBuf::from("/workspace/report.txt");
    let preview_window = window::Id::unique();
    browser.preview_engine.preview_shown_path = Some(path.clone());
    browser.preview_engine.preview = Some(PreviewState::Loading(path));
    browser.preview_engine.preview_window = Some(preview_window);
    (browser, preview_window)
}

fn press_space(browser: &mut FileBrowser, status: Status) -> Task<Message> {
    browser.update(Message::KeyboardKeyPressed {
        key: Key::Named(Named::Space),
        modified_key: Key::Named(Named::Space),
        modifiers: Modifiers::default(),
        status,
    })
}

/// 把“搜索条输入框没在打字”的检查跑一遍并取回它确认后发出的消息；任务没做检查时返回 None。
/// 预览窗里此时没有搜索条，检查必然通过。
async fn find_input_check_outcome(task: Task<Message>) -> Option<Message> {
    let mut stream = iced_runtime::task::into_stream(task)?;
    let Some(Action::Widget(check)) = stream.next().await else {
        return None;
    };
    check.finish();
    // 丢掉检查才会放掉消息通道的发送端，流才能在读完消息后结束。
    drop(check);
    match stream.next().await {
        Some(Action::Output(message)) => Some(message),
        _ => None,
    }
}

#[tokio::test]
async fn preview_key_closes_the_focused_preview_window_whether_or_not_it_was_captured() {
    for status in [Status::Ignored, Status::Captured] {
        let (mut browser, preview_window) = browser_with_preview_window();
        browser.focused_window = preview_window;

        let task = press_space(&mut browser, status);
        let confirmed = find_input_check_outcome(task)
            .await
            .expect("preview window asks whether the find input has focus");
        assert!(matches!(confirmed, Message::PreviewWindowCloseKeyConfirmed));
        // 检查只是查焦点，窗口要等确认消息回来才关。
        assert_eq!(browser.preview_engine.preview_window, Some(preview_window));

        drop(browser.update(confirmed));

        assert!(
            browser.preview_engine.preview_window.is_none(),
            "{status:?}"
        );
        assert!(browser.preview_engine.preview.is_none(), "{status:?}");
        assert_eq!(browser.focused_window, browser.main_window);
    }
}

#[tokio::test]
async fn close_confirmation_does_nothing_once_focus_left_the_preview_window() {
    let (mut browser, preview_window) = browser_with_preview_window();
    browser.focused_window = preview_window;
    let task = press_space(&mut browser, Status::Ignored);
    let confirmed = find_input_check_outcome(task)
        .await
        .expect("find input check");

    browser.focused_window = browser.main_window;
    drop(browser.update(confirmed));

    assert_eq!(browser.preview_engine.preview_window, Some(preview_window));
    assert!(browser.preview_engine.preview.is_some());
}

#[tokio::test]
async fn captured_space_in_the_main_window_leaves_an_open_preview_alone() {
    let (mut browser, preview_window) = browser_with_preview_window();
    browser.focused_window = browser.main_window;

    let task = press_space(&mut browser, Status::Captured);

    assert!(find_input_check_outcome(task).await.is_none());
    assert_eq!(browser.preview_engine.preview_window, Some(preview_window));
    assert!(browser.preview_engine.preview.is_some());
}
