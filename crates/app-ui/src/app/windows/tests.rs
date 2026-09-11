use file_operation_store::TaskQueueStore;
use iced::futures::StreamExt;
use iced::Task;
use iced_runtime::Action;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::animated_image_preview::{
    AnimatedImageFrame, AnimatedImagePlayback, AnimatedImagePreview,
};
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserViewMode, LoadedOperationStore, Message,
    PreviewContent, PreviewSize, PreviewState, PreviewWindowChromeState, SettingsCategory,
    SettingsSubpage, SplitAxis, WindowChromeLayout, WindowFrameState, WINDOW_TOP_BAR_HEIGHT,
};
use crate::operation_history::FileOperationCompletion;
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

const FLOAT_TOLERANCE: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownAction {
    WindowClosed(window::Id),
    PersistenceFinished,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedResizeDirection {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedWindowAction {
    Open(window::Id),
    Close(window::Id),
    Minimize(window::Id),
    ToggleMaximize(window::Id),
    QueryMaximized(window::Id),
    Drag(window::Id),
    Resize(window::Id, ObservedResizeDirection),
    FrameObserved(window::Id, WindowFrameState),
}

async fn observed_window_actions(task: Task<Message>) -> Vec<ObservedWindowAction> {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    while let Some(action) = stream.next().await {
        match action {
            Action::Window(iced_runtime::window::Action::Open(window, _, sender)) => {
                sender.send(window).expect("send opened window id");
                actions.push(ObservedWindowAction::Open(window));
            }
            Action::Window(iced_runtime::window::Action::Close(window)) => {
                actions.push(ObservedWindowAction::Close(window));
            }
            Action::Window(iced_runtime::window::Action::Minimize(window, true)) => {
                actions.push(ObservedWindowAction::Minimize(window));
            }
            Action::Window(iced_runtime::window::Action::ToggleMaximize(window)) => {
                actions.push(ObservedWindowAction::ToggleMaximize(window));
            }
            Action::Window(iced_runtime::window::Action::GetMaximized(window, sender)) => {
                actions.push(ObservedWindowAction::QueryMaximized(window));
                sender.send(false).expect("send maximized state");
            }
            Action::Window(iced_runtime::window::Action::Drag(window)) => {
                actions.push(ObservedWindowAction::Drag(window));
            }
            Action::Window(iced_runtime::window::Action::DragResize(window, direction)) => {
                let direction = match direction {
                    window::Direction::North => ObservedResizeDirection::North,
                    window::Direction::South => ObservedResizeDirection::South,
                    window::Direction::East => ObservedResizeDirection::East,
                    window::Direction::West => ObservedResizeDirection::West,
                    window::Direction::NorthEast => ObservedResizeDirection::NorthEast,
                    window::Direction::NorthWest => ObservedResizeDirection::NorthWest,
                    window::Direction::SouthEast => ObservedResizeDirection::SouthEast,
                    window::Direction::SouthWest => ObservedResizeDirection::SouthWest,
                };
                actions.push(ObservedWindowAction::Resize(window, direction));
            }
            Action::Output(Message::WindowMaximizedObserved(window, frame_state)) => {
                actions.push(ObservedWindowAction::FrameObserved(window, frame_state));
            }
            _ => {}
        }
    }
    actions
}

async fn widget_action_count(task: Task<Message>) -> usize {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return 0;
    };
    let mut count = 0;
    while let Some(action) = stream.next().await {
        if matches!(action, Action::Widget(_)) {
            count += 1;
        }
    }
    count
}

fn clamped_image_size(width: u32, height: u32) -> PreviewSize {
    clamp_preview_size_to_minimum(
        PreviewWindowProfile::Image,
        image_preview_size_from_dimensions(width, height),
    )
}

fn clamped_video_size(width: u32, height: u32) -> PreviewSize {
    clamp_preview_size_to_minimum(
        PreviewWindowProfile::Video,
        video_preview_size_from_frame(width, height),
    )
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= FLOAT_TOLERANCE,
        "expected {actual} to be within {FLOAT_TOLERANCE} of {expected}"
    );
}

async fn shutdown_actions(task: Task<Message>) -> Vec<ShutdownAction> {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    while let Some(action) = stream.next().await {
        match action {
            Action::Window(iced_runtime::window::Action::Close(window)) => {
                actions.push(ShutdownAction::WindowClosed(window));
            }
            Action::Output(Message::ApplicationShutdownPersisted(result)) => {
                assert_eq!(result, Ok(()));
                actions.push(ShutdownAction::PersistenceFinished);
            }
            Action::Exit => actions.push(ShutdownAction::Exit),
            _ => {}
        }
    }
    actions
}

#[test]
fn every_application_window_disables_native_decorations() {
    let main = main_window_settings();
    let settings = settings_window_settings();
    let properties = properties_window_settings();
    let preview = preview_window_settings(
        PreviewWindowProfile::Regular,
        default_preview_size(PreviewWindowProfile::Regular),
    );

    assert!(!main.decorations);
    assert!(!settings.decorations);
    assert!(!properties.decorations);
    assert!(!preview.decorations);
    assert!(main.exit_on_close_request);
    assert!(settings.exit_on_close_request);
    assert!(properties.exit_on_close_request);
    assert!(preview.exit_on_close_request);
}

#[test]
fn auxiliary_windows_keep_one_content_size_across_global_chrome_layouts() {
    for layout in WindowChromeLayout::ALL {
        let mut config = config::default_user_config();
        let _ = config.window_controls.select_layout(layout);
        assert_eq!(config.window_controls.layout(), layout);
        let settings = settings_window_settings();
        assert_close(
            settings.size.height,
            DEFAULT_SETTINGS_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        );
        let properties = properties_window_settings();
        assert_close(
            properties.size.height,
            DEFAULT_PROPERTIES_HEIGHT + WINDOW_TOP_BAR_HEIGHT,
        );

        let content_size = default_preview_size(PreviewWindowProfile::Regular);
        let preview = preview_window_settings(PreviewWindowProfile::Regular, content_size);
        assert_close(preview.size.width, content_size.width);
        assert_close(preview.size.height, content_size.height);
    }
}

#[test]
fn preview_resize_uses_the_full_client_area_without_chrome_offset() {
    let profile = PreviewWindowProfile::Image;
    let expected = PreviewSize {
        width: 900.0,
        height: 700.0,
    };

    let actual = preview_content_size_from_window(profile, expected.width, expected.height);

    assert_close(actual.width, expected.width);
    assert_close(actual.height, expected.height);
}

#[test]
fn preview_controls_follow_only_the_preview_window_top_region() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, PreviewWindowChromeState::REVEAL_HEIGHT),
    }));
    assert!(browser.preview_window_chrome.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: browser.main_window,
        position: iced::Point::new(8.0, 8.0),
    }));
    assert!(browser.preview_window_chrome.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, PreviewWindowChromeState::REVEAL_HEIGHT + 1.0),
    }));
    assert!(!browser.preview_window_chrome.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, PreviewWindowChromeState::REVEAL_HEIGHT),
    }));
    assert!(browser.preview_window_chrome.target_is_visible());
    drop(browser.update(Message::CursorLeft {
        window: browser.main_window,
    }));
    assert!(browser.preview_window_chrome.target_is_visible());

    drop(browser.update(Message::CursorLeft {
        window: preview_window,
    }));
    assert!(!browser.preview_window_chrome.target_is_visible());
}
#[test]
fn preview_top_and_bottom_controls_have_independent_pointer_regions() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_profile = PreviewWindowProfile::Video;
    browser.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: PathBuf::from("clip.mp4"),
        frame: None,
        width: 320,
        height: 180,
        duration: Some(Duration::from_secs(10)),
    }));
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 700.0,
    };

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 64.0),
    }));
    assert!(browser.preview_window_chrome.target_is_visible());
    assert!(!browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 636.0),
    }));
    assert!(!browser.preview_window_chrome.target_is_visible());
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 635.9),
    }));
    assert!(!browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.update(Message::CursorLeft {
        window: preview_window,
    }));
    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

#[test]
fn preview_bottom_controls_reset_when_preview_closes() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_profile = PreviewWindowProfile::Video;
    browser.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: PathBuf::from("clip.mp4"),
        frame: None,
        width: 320,
        height: 180,
        duration: Some(Duration::from_secs(10)),
    }));
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 700.0,
    };

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 636.0),
    }));
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.close_preview_window());
    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

#[test]
fn preview_bottom_controls_retarget_after_client_height_changes() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_profile = PreviewWindowProfile::Video;
    browser.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: PathBuf::from("clip.mp4"),
        frame: None,
        width: 320,
        height: 180,
        duration: Some(Duration::from_secs(10)),
    }));
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 700.0,
    };

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 636.0),
    }));
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    browser.pending_preview_resize = None;
    drop(browser.handle_auxiliary_window_resized(preview_window, 640.0, 500.0));
    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

#[test]
fn initial_preview_chrome_hides_only_without_pointer_interaction() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();

    browser.preview_window = Some(preview_window);
    browser.preview_window_profile = PreviewWindowProfile::Video;
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 700.0,
    };
    browser.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: PathBuf::from("clip.mp4"),
        frame: None,
        width: 320,
        height: 180,
        duration: Some(Duration::from_secs(10)),
    }));

    drop(browser.start_preview_window_initial_chrome());
    assert!(browser.preview_window_chrome.target_is_visible());
    assert!(browser.preview_window_bottom_controls.target_is_visible());
    let initial_generation = browser.preview_window_initial_chrome_generation;
    browser.hide_preview_window_initial_chrome(initial_generation);
    assert!(!browser.preview_window_chrome.target_is_visible());
    assert!(!browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.start_preview_window_initial_chrome());
    let stale_generation = browser.preview_window_initial_chrome_generation;
    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 636.0),
    }));
    assert!(browser.preview_window_bottom_controls.target_is_visible());
    browser.hide_preview_window_initial_chrome(stale_generation);
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    let current_generation = browser.preview_window_initial_chrome_generation;
    browser.hide_preview_window_initial_chrome(current_generation);
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 635.9),
    }));
    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

fn animated_preview_for_window(duration: Option<Duration>) -> AnimatedImagePreview {
    let path = PathBuf::from("animation.gif");
    AnimatedImagePreview::new(
        path.clone(),
        AnimatedImageFrame {
            path,
            generation: 1,
            position: Duration::ZERO,
            delay: Duration::from_millis(20),
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        },
        1,
        duration,
        AnimatedImagePlayback::Animated,
    )
    .expect("animated image preview")
}

#[test]
fn animated_image_bottom_controls_follow_video_visibility_rules() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();

    browser.preview_window = Some(preview_window);
    browser.preview_window_profile = PreviewWindowProfile::Image;
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 700.0,
    };
    browser.preview = Some(PreviewState::Ready(PreviewContent::AnimatedImage(
        animated_preview_for_window(Some(Duration::from_secs(10))),
    )));

    drop(browser.start_preview_window_initial_chrome());
    assert!(browser.preview_window_chrome.target_is_visible());
    assert!(browser.preview_window_bottom_controls.target_is_visible());
    let initial_generation = browser.preview_window_initial_chrome_generation;
    browser.hide_preview_window_initial_chrome(initial_generation);
    assert!(!browser.preview_window_bottom_controls.target_is_visible());

    drop(browser.start_preview_window_initial_chrome());
    let stale_generation = browser.preview_window_initial_chrome_generation;
    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 636.0),
    }));
    browser.hide_preview_window_initial_chrome(stale_generation);
    assert!(browser.preview_window_bottom_controls.target_is_visible());

    let current_generation = browser.preview_window_initial_chrome_generation;
    browser.hide_preview_window_initial_chrome(current_generation);
    assert!(browser.preview_window_bottom_controls.target_is_visible());
    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, 635.9),
    }));
    assert!(!browser.preview_window_bottom_controls.target_is_visible());

    browser.preview = Some(PreviewState::Ready(PreviewContent::AnimatedImage(
        animated_preview_for_window(None),
    )));
    drop(browser.start_preview_window_initial_chrome());
    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

#[test]
fn main_window_left_click_clears_global_error_for_captured_and_ignored_events() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let main_window = browser.main_window;

    browser.show_global_error("captured failure");
    drop(browser.update(Message::WindowPointerPressed {
        window: main_window,
        button: iced::mouse::Button::Left,
        status: iced::event::Status::Captured,
    }));
    assert_eq!(browser.current_error(), None);

    browser.show_global_error("ignored failure");
    drop(browser.update(Message::WindowPointerPressed {
        window: main_window,
        button: iced::mouse::Button::Left,
        status: iced::event::Status::Ignored,
    }));
    assert_eq!(browser.current_error(), None);
}

#[test]
fn preview_controls_stay_visible_while_the_window_is_dragged() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_chrome.start_reveal();

    drop(browser.update(Message::WindowDragRequested(preview_window)));
    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, PreviewWindowChromeState::REVEAL_HEIGHT + 1.0),
    }));
    drop(browser.update(Message::CursorLeft {
        window: preview_window,
    }));
    assert!(browser.preview_window_chrome.target_is_visible());

    drop(browser.update(Message::WindowPointerReleased {
        window: preview_window,
        status: iced::event::Status::Captured,
    }));
    drop(browser.update(Message::CursorMoved {
        window: preview_window,
        position: iced::Point::new(8.0, PreviewWindowChromeState::REVEAL_HEIGHT + 1.0),
    }));
    assert!(!browser.preview_window_chrome.target_is_visible());
}

#[test]
fn preview_bottom_controls_hide_after_drag_without_pointer_position() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview = Some(PreviewState::Ready(PreviewContent::Video {
        path: PathBuf::from("clip.mp4"),
        frame: None,
        width: 320,
        height: 180,
        duration: Some(Duration::from_secs(10)),
    }));

    drop(browser.start_preview_window_initial_chrome());
    drop(browser.update(Message::WindowDragRequested(preview_window)));
    drop(browser.update(Message::WindowPointerReleased {
        window: preview_window,
        status: iced::event::Status::Captured,
    }));

    assert!(!browser.preview_window_bottom_controls.target_is_visible());
}

#[tokio::test]
async fn image_dimension_update_reuses_existing_preview_window() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);

    let actions =
        observed_window_actions(browser.open_image_preview_window_for_dimensions(1600, 900)).await;

    assert_eq!(browser.preview_window, Some(preview_window));
    assert_eq!(browser.focused_window, preview_window);
    assert_eq!(browser.preview_window_profile, PreviewWindowProfile::Image);
    assert!(actions.iter().all(|action| {
        !matches!(
            action,
            ObservedWindowAction::Open(_) | ObservedWindowAction::Close(_)
        )
    }));
}

#[test]
fn preview_window_opens_with_controls_visible() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.open_image_preview_window_for_dimensions(748, 499));

    assert!(browser.preview_window_chrome.target_is_visible());
    assert_close(browser.preview_size.width, 748.0);
    assert_close(browser.preview_size.height, 499.0);

    drop(browser.close_preview_window());
    assert!(!browser.preview_window_chrome.target_is_visible());
}

#[test]
fn maximized_observation_is_isolated_and_removed_when_window_closes() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let settings_window = window::Id::unique();
    browser.settings_window = Some(settings_window);
    browser.expanded_color_scheme_family =
        Some(crate::matugen_theme::ColorSchemeFamily::Everforest);

    drop(browser.accept_window_maximized_observation(settings_window, WindowFrameState::Maximized));
    assert!(browser.maximized_windows.contains(&settings_window));
    assert!(!browser.maximized_windows.contains(&browser.main_window));

    drop(browser.close_settings_window());
    assert!(!browser.maximized_windows.contains(&settings_window));
    assert_eq!(browser.expanded_color_scheme_family, None);

    drop(browser.accept_window_maximized_observation(settings_window, WindowFrameState::Maximized));
    assert!(!browser.maximized_windows.contains(&settings_window));
}

#[tokio::test]
async fn window_control_messages_target_the_source_window() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source_window = browser.main_window;

    let minimize =
        observed_window_actions(browser.update(Message::WindowMinimizeRequested(source_window)))
            .await;
    assert_eq!(
        minimize,
        vec![ObservedWindowAction::Minimize(source_window)]
    );

    let maximize =
        observed_window_actions(browser.update(Message::WindowMaximizeToggled(source_window)))
            .await;
    assert_eq!(
        maximize,
        vec![
            ObservedWindowAction::ToggleMaximize(source_window),
            ObservedWindowAction::QueryMaximized(source_window),
            ObservedWindowAction::FrameObserved(source_window, WindowFrameState::Restored),
        ]
    );

    let drag =
        observed_window_actions(browser.update(Message::WindowDragRequested(source_window))).await;
    assert_eq!(drag, vec![ObservedWindowAction::Drag(source_window)]);

    let resize_directions = [
        (window::Direction::North, ObservedResizeDirection::North),
        (window::Direction::South, ObservedResizeDirection::South),
        (window::Direction::East, ObservedResizeDirection::East),
        (window::Direction::West, ObservedResizeDirection::West),
        (
            window::Direction::NorthEast,
            ObservedResizeDirection::NorthEast,
        ),
        (
            window::Direction::NorthWest,
            ObservedResizeDirection::NorthWest,
        ),
        (
            window::Direction::SouthEast,
            ObservedResizeDirection::SouthEast,
        ),
        (
            window::Direction::SouthWest,
            ObservedResizeDirection::SouthWest,
        ),
    ];
    for (direction, observed_direction) in resize_directions {
        let resize = observed_window_actions(
            browser.update(Message::WindowResizeRequested(source_window, direction)),
        )
        .await;
        assert_eq!(
            resize,
            vec![ObservedWindowAction::Resize(
                source_window,
                observed_direction,
            )]
        );
    }
}

#[tokio::test]
async fn maximized_window_does_not_start_edge_resize() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source_window = browser.main_window;
    drop(browser.accept_window_maximized_observation(source_window, WindowFrameState::Maximized));

    let actions = observed_window_actions(browser.update(Message::WindowResizeRequested(
        source_window,
        window::Direction::North,
    )))
    .await;

    assert!(actions.is_empty());
}

#[tokio::test]
async fn idle_shutdown_closes_every_window_and_exits_after_one_outcome() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let settings_window = window::Id::unique();
    let properties_window = window::Id::unique();
    let preview_window = window::Id::unique();
    browser.settings_window = Some(settings_window);
    browser.properties_window = Some(properties_window);
    browser.preview_window = Some(preview_window);

    let shutdown = shutdown_actions(browser.close_auxiliary_window(browser.main_window)).await;
    let closed = shutdown
        .iter()
        .filter_map(|action| match action {
            ShutdownAction::WindowClosed(window) => Some(*window),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        closed,
        std::collections::HashSet::from([
            browser.main_window,
            settings_window,
            properties_window,
            preview_window,
        ])
    );
    assert!(!shutdown.contains(&ShutdownAction::PersistenceFinished));
    assert!(!shutdown.contains(&ShutdownAction::Exit));
    for window in [settings_window, properties_window, preview_window] {
        assert!(
            shutdown_actions(browser.update(Message::ApplicationWindowClosed(window)))
                .await
                .is_empty()
        );
    }
    let persisted =
        shutdown_actions(browser.update(Message::ApplicationWindowClosed(browser.main_window)))
            .await;
    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);
    assert!(
        shutdown_actions(browser.close_auxiliary_window(browser.main_window))
            .await
            .is_empty()
    );
    assert!(
        observed_window_actions(browser.update(Message::AuxiliaryWindowResized(
            browser.main_window,
            900.0,
            600.0,
        )))
        .await
        .is_empty()
    );

    assert_eq!(
        shutdown_actions(browser.update(Message::ApplicationShutdownPersisted(Ok(())))).await,
        vec![ShutdownAction::Exit]
    );
    assert!(
        shutdown_actions(browser.update(Message::ApplicationShutdownPersisted(Ok(()))))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn observed_main_window_close_starts_persistence_without_a_second_close_action() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    let shutdown =
        shutdown_actions(browser.update(Message::ApplicationWindowClosed(browser.main_window)))
            .await;

    assert_eq!(shutdown, vec![ShutdownAction::PersistenceFinished]);
    assert_eq!(
        shutdown_actions(browser.update(Message::ApplicationShutdownPersisted(Ok(())))).await,
        vec![ShutdownAction::Exit]
    );
}

#[tokio::test]
async fn close_command_completion_is_a_bounded_missing_event_fallback() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.close_auxiliary_window(browser.main_window));

    let persisted =
        shutdown_actions(browser.update(Message::ApplicationWindowCloseCommandsFinished)).await;

    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_waits_for_old_preferences_save_then_commits_latest_search_history() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let store =
        TaskQueueStore::new(temp_dir.path().join("state.sqlite")).expect("create state store");
    let mut old_preferences = config::default_user_config().user_preferences().to_stored();
    old_preferences.search_history = vec!["old".to_owned()];
    store
        .replace_user_preferences(&old_preferences)
        .expect("store old preferences");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_operation_store(Ok(LoadedOperationStore {
        task_queue_store: store.clone(),
        column_width_overrides: HashMap::new(),
        column_width_reference_content_width: None,
        classified_startup_session: None,
    })));
    browser.user_preferences_save_in_flight = true;
    browser
        .user_config
        .search_history
        .record_submission("latest");

    drop(browser.close_auxiliary_window(browser.main_window));
    assert!(shutdown_actions(
        browser.update(Message::ApplicationWindowClosed(browser.main_window)),
    )
    .await
    .is_empty());

    assert_eq!(
        shutdown_actions(browser.update(Message::UserPreferencesSaved(Ok(())))).await,
        vec![ShutdownAction::PersistenceFinished]
    );
    assert_eq!(
        store
            .read_user_preferences()
            .unwrap()
            .unwrap()
            .search_history,
        ["latest"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn final_shutdown_session_waits_for_an_older_save_outcome() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let store =
        TaskQueueStore::new(temp_dir.path().join("state.sqlite")).expect("create state store");
    let mut user_config = config::default_user_config();
    user_config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
    user_config.save_view_state = true;
    let (mut browser, _) = FileBrowser::new(user_config);
    drop(browser.accept_operation_store(Ok(LoadedOperationStore {
        task_queue_store: store.clone(),
        column_width_overrides: HashMap::new(),
        column_width_reference_content_width: None,
        classified_startup_session: None,
    })));
    let final_directory = temp_dir.path().join("final-directory");
    fs::create_dir_all(&final_directory).unwrap();
    browser.current_dir = final_directory.clone();
    browser.sync_active_tab_state();
    browser.browser_session_saves_in_flight = 1;

    let initial = shutdown_actions(browser.close_auxiliary_window(browser.main_window)).await;
    assert!(initial.contains(&ShutdownAction::WindowClosed(browser.main_window)));
    assert!(!initial.contains(&ShutdownAction::PersistenceFinished));
    assert!(shutdown_actions(
        browser.update(Message::ApplicationWindowClosed(browser.main_window)),
    )
    .await
    .is_empty());

    let persisted = shutdown_actions(browser.update(Message::BrowserSessionSaved(Ok(())))).await;
    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);
    let stored = store.read_browser_session().unwrap().unwrap();
    assert_eq!(
        stored.panes[0].tabs[0].directory.to_path_buf(),
        final_directory
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recoverable_runner_ack_precedes_one_shutdown_transaction_and_exit() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let store =
        TaskQueueStore::new(temp_dir.path().join("state.sqlite")).expect("create state store");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_operation_store(Ok(LoadedOperationStore {
        task_queue_store: store.clone(),
        column_width_overrides: HashMap::new(),
        column_width_reference_content_width: None,
        classified_startup_session: None,
    })));
    let enqueue = browser.operation_queue.enqueue(QueuedFileOperation::Copy {
        transfers: vec![QueuedTransfer::new(
            temp_dir.path().join("source"),
            temp_dir.path().join("target"),
        )],
        verification: file_core::FileOperationVerification::BasicMetadata,
    });
    assert!(enqueue.error().is_none());
    let task_id = browser.operation_queue.tasks()[0].id;

    let initial = shutdown_actions(browser.close_auxiliary_window(browser.main_window)).await;
    let stored_task_id = browser.operation_queue.tasks()[0].stored_id.unwrap();
    assert!(initial.contains(&ShutdownAction::WindowClosed(browser.main_window)));
    assert!(!initial.contains(&ShutdownAction::PersistenceFinished));
    assert!(shutdown_actions(
        browser.update(Message::ApplicationWindowClosed(browser.main_window)),
    )
    .await
    .is_empty());
    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_none());

    let persisted = shutdown_actions(browser.update(Message::FileOperationFinished(
        task_id,
        FileOperationCompletion::RecoveryInterrupted("application stopping".to_owned(), Vec::new()),
    )))
    .await;
    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        file_operation_store::StoredTaskStatus::RecoveryPending
    );
    assert!(!store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_none());

    assert_eq!(
        shutdown_actions(browser.update(Message::ApplicationShutdownPersisted(Ok(())))).await,
        vec![ShutdownAction::Exit]
    );
    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_recoverable_completion_is_not_rewritten_as_recovery_pending() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let store =
        TaskQueueStore::new(temp_dir.path().join("state.sqlite")).expect("create state store");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_operation_store(Ok(LoadedOperationStore {
        task_queue_store: store.clone(),
        column_width_overrides: HashMap::new(),
        column_width_reference_content_width: None,
        classified_startup_session: None,
    })));
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Copy {
            transfers: vec![QueuedTransfer::new(
                temp_dir.path().join("source"),
                temp_dir.path().join("target"),
            )],
            verification: file_core::FileOperationVerification::BasicMetadata,
        })
        .error()
        .is_none());
    let task_id = browser.operation_queue.tasks()[0].id;

    drop(browser.close_auxiliary_window(browser.main_window));
    drop(browser.update(Message::ApplicationWindowClosed(browser.main_window)));
    let persisted = shutdown_actions(browser.update(Message::FileOperationFinished(
        task_id,
        FileOperationCompletion::RecoveryBlocked {
            error: "manual recovery required".to_owned(),
            completed_move_transfers: Vec::new(),
        },
    )))
    .await;

    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);
    let stored_task_id = browser.operation_queue.tasks()[0].stored_id.unwrap();
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        file_operation_store::StoredTaskStatus::Failed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn close_all_windows_saves_browser_session_before_exit() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let root = temp_dir.path().join("home");
    let deepest = root.join("project").join("src");
    fs::create_dir_all(&deepest).expect("create column directory chain");
    let store =
        TaskQueueStore::new(temp_dir.path().join("state.sqlite")).expect("create state store");

    let mut user_config = config::default_user_config();
    user_config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
    user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
    user_config.browser_view_mode = BrowserViewMode::Columns;
    let (mut browser, _) = FileBrowser::new(user_config);
    drop(browser.accept_operation_store(Ok(LoadedOperationStore {
        task_queue_store: store.clone(),
        column_width_overrides: HashMap::new(),
        column_width_reference_content_width: None,
        classified_startup_session: None,
    })));

    browser.current_dir = root.clone();
    browser.view_mode = BrowserViewMode::Columns;
    browser.deepest_open_column_directory = Some(deepest.clone());
    browser.sync_active_tab_state();

    let shutdown = shutdown_actions(browser.close_auxiliary_window(browser.main_window)).await;
    assert!(shutdown.contains(&ShutdownAction::WindowClosed(browser.main_window)));
    assert!(!shutdown.contains(&ShutdownAction::PersistenceFinished));
    assert!(!shutdown.contains(&ShutdownAction::Exit));

    let persisted =
        shutdown_actions(browser.update(Message::ApplicationWindowClosed(browser.main_window)))
            .await;
    assert_eq!(persisted, vec![ShutdownAction::PersistenceFinished]);

    let exit =
        shutdown_actions(browser.update(Message::ApplicationShutdownPersisted(Ok(())))).await;
    assert_eq!(exit, vec![ShutdownAction::Exit]);

    let stored_session = store
        .read_browser_session()
        .expect("read browser session")
        .expect("browser session should be stored");
    let stored_deepest_directory = stored_session.panes[0].tabs[0]
        .deepest_open_column_directory
        .as_ref()
        .map(|path| path.to_path_buf());
    assert_eq!(stored_deepest_directory.as_deref(), Some(deepest.as_path()));
}

#[test]
fn system_window_focus_ignores_late_unfocus_from_an_older_window() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let main_window = browser.main_window;
    let settings_window = window::Id::unique();

    assert_eq!(browser.system_focused_window, None);
    drop(browser.handle_window_focused(main_window));
    assert_eq!(browser.system_focused_window, Some(main_window));

    drop(browser.handle_window_focused(settings_window));
    drop(browser.handle_window_unfocused(main_window));
    assert_eq!(browser.system_focused_window, Some(settings_window));
    assert_eq!(browser.focused_window, settings_window);

    drop(browser.handle_window_unfocused(settings_window));
    assert_eq!(browser.system_focused_window, None);
}

#[tokio::test]
async fn main_window_refocus_rechecks_search_input_focus() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let main_window = browser.main_window;

    assert_eq!(
        widget_action_count(browser.handle_window_focused(main_window)).await,
        1
    );
    assert_eq!(
        widget_action_count(browser.handle_window_focused(window::Id::unique())).await,
        0
    );
}

#[tokio::test]
async fn only_tab_key_rechecks_search_input_focus() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    let enter = browser.update(Message::KeyboardKeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        modifiers: iced::keyboard::Modifiers::default(),
        status: iced::event::Status::Captured,
    });
    assert_eq!(widget_action_count(enter).await, 0);

    for (modifiers, expected_actions) in [
        (iced::keyboard::Modifiers::default(), 1),
        (iced::keyboard::Modifiers::SHIFT, 1),
        (iced::keyboard::Modifiers::CTRL, 0),
        (iced::keyboard::Modifiers::ALT, 0),
    ] {
        let tab = browser.update(Message::KeyboardKeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
            modifiers,
            status: iced::event::Status::Captured,
        });
        assert_eq!(widget_action_count(tab).await, expected_actions);
    }
}

#[test]
fn closing_focused_auxiliary_window_clears_system_focus() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let settings_window = window::Id::unique();
    browser.settings_window = Some(settings_window);
    browser.focused_window = settings_window;
    browser.system_focused_window = Some(settings_window);

    drop(browser.close_settings_window());

    assert_eq!(browser.system_focused_window, None);
    assert_eq!(browser.focused_window, browser.main_window);
}

#[test]
fn image_preview_size_fits_large_landscape_to_max_width() {
    let size = clamped_image_size(3_000, 1_000);
    let max_size = image_preview_initial_fit_max_size();

    assert_close(size.width, max_size.width);
    assert!(size.height < max_size.height);
    assert_close(size.width / size.height, 3.0);
}

#[test]
fn image_preview_size_fits_large_portrait_to_max_height() {
    let size = clamped_image_size(1_000, 2_000);
    let max_size = image_preview_initial_fit_max_size();

    assert!(size.width < max_size.width);
    assert_close(size.height, max_size.height);
    assert_close(size.height / size.width, 2.0);
}

#[test]
fn image_preview_size_clamps_small_images_to_minimum_window() {
    let size = clamped_image_size(64, 32);
    let min_size = preview_min_size(PreviewWindowProfile::Image);

    assert_close(size.width, min_size.width);
    assert_close(size.height, min_size.height);
}

#[test]
fn image_preview_size_keeps_medium_landscape_tight() {
    let size = clamped_image_size(748, 499);

    assert_close(size.width, 748.0);
    assert_close(size.height, 499.0);
}

#[test]
fn image_preview_size_keeps_medium_portrait_tight() {
    let size = clamped_image_size(400, 600);

    assert_close(size.width, 400.0);
    assert_close(size.height, 600.0);
}

#[test]
fn animated_image_preview_size_preserves_ratio_at_image_minimum() {
    let size = animated_image_preview_size_from_dimensions(320, 180);
    let min_size = preview_min_size(PreviewWindowProfile::Image);

    assert!(size.width > min_size.width);
    assert_close(size.height, min_size.height);
    assert_close(size.width / size.height, 320.0 / 180.0);
}

#[test]
fn animated_image_preview_size_bounds_extreme_aspect_ratio() {
    let size = animated_image_preview_size_from_dimensions(1, 65_535);
    let min_size = preview_min_size(PreviewWindowProfile::Image);

    assert_close(size.width, min_size.width);
    assert_close(size.height, min_size.height);
}

#[test]
fn video_preview_size_keeps_medium_frame_tight() {
    let size = clamped_video_size(640, 360);

    assert_close(size.width, 640.0);
    assert_close(size.height, 360.0);
}

#[test]
fn video_preview_size_fits_large_portrait_frame_to_max_height() {
    let size = clamped_video_size(720, 1280);
    let max_size = video_preview_initial_fit_max_size();

    assert!(size.width < max_size.width);
    assert_close(size.height, max_size.height);
    assert_close(size.height / size.width, 1280.0 / 720.0);
}

#[test]
fn pinned_preview_survives_main_window_pointer_press() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_pinned = true;

    drop(browser.update(Message::WindowPointerPressed {
        window: browser.main_window,
        button: iced::mouse::Button::Left,
        status: iced::event::Status::Ignored,
    }));

    assert_eq!(browser.preview_window, Some(preview_window));
    assert!(browser.preview_window_pinned);
}

#[test]
fn unpinned_preview_closes_on_main_window_pointer_press() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());

    drop(browser.update(Message::WindowPointerPressed {
        window: browser.main_window,
        button: iced::mouse::Button::Left,
        status: iced::event::Status::Ignored,
    }));

    assert!(browser.preview_window.is_none());
}

#[test]
fn pinned_preview_content_survives_interaction_clears_until_closed() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());
    browser.preview_window_pinned = true;
    browser.preview = Some(PreviewState::Loading(PathBuf::from("photo.png")));

    browser.clear_preview();
    assert!(browser.preview.is_some());

    drop(browser.close_preview_window());
    assert!(browser.preview.is_none());
    assert!(!browser.preview_window_pinned);
}

#[test]
fn request_preview_without_hover_or_preview_is_noop() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.request_preview());

    assert!(browser.preview.is_none());
    assert!(browser.preview_window.is_none());
}

#[test]
fn request_preview_without_hover_closes_open_preview() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());
    browser.preview_window_pinned = true;
    browser.preview_shown_path = Some(PathBuf::from("photo.png"));
    browser.preview = Some(PreviewState::Loading(PathBuf::from("photo.png")));

    drop(browser.request_preview());

    assert!(browser.preview.is_none());
    assert!(browser.preview_window.is_none());
    assert!(!browser.preview_window_pinned);
}

#[test]
fn request_preview_with_hover_but_no_selection_is_noop() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.hovered_entry = Some(PathBuf::from("note.txt"));

    drop(browser.request_preview());

    assert!(browser.preview.is_none());
    assert!(browser.preview_window.is_none());
}

#[test]
fn request_preview_toggles_closed_for_same_target() {
    let path = PathBuf::from("clip.mp4");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());
    browser.hovered_entry = Some(path.clone());
    browser.selected = Some(path.clone());
    browser.preview_shown_path = Some(path);
    browser.preview = Some(PreviewState::Loading(PathBuf::from("clip.mp4")));

    drop(browser.request_preview());

    assert!(browser.preview.is_none());
    assert!(browser.preview_window.is_none());
    assert!(!browser.preview_window_pinned);
}

#[test]
fn pinned_space_switches_to_newly_selected_file_and_keeps_pin() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());
    browser.preview_window_pinned = true;
    browser.preview_shown_path = Some(PathBuf::from("first.png"));
    browser.preview = Some(PreviewState::Loading(PathBuf::from("first.png")));

    let next = PathBuf::from("second.png");
    browser.hovered_entry = Some(next.clone());
    browser.selected = Some(next.clone());

    drop(browser.request_preview());

    assert_eq!(browser.preview_shown_path.as_deref(), Some(next.as_path()));
    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(path)) if path == &next
    ));
    assert!(browser.preview_window.is_some());
    assert!(browser.preview_window_pinned);
}

#[test]
fn pinned_preview_survives_preview_window_unfocus() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.preview_window_pinned = true;

    drop(browser.update(Message::WindowUnfocused(preview_window)));

    assert_eq!(browser.preview_window, Some(preview_window));
    assert!(browser.preview_window_pinned);
}

#[test]
fn unpinned_preview_closes_when_preview_window_unfocuses() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_window = Some(window::Id::unique());

    drop(browser.update(Message::WindowUnfocused(browser.preview_window.unwrap())));

    assert!(browser.preview_window.is_none());
}

#[test]
fn selecting_settings_category_clears_open_subpage() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.selected_settings_category = SettingsCategory::Files;
    drop(browser.select_settings_subpage(SettingsSubpage::Preview));
    assert_eq!(browser.settings_subpage, Some(SettingsSubpage::Preview));

    drop(browser.select_settings_category(SettingsCategory::Appearance));

    assert_eq!(browser.settings_subpage, None);
    assert_eq!(
        browser.selected_settings_category,
        SettingsCategory::Appearance
    );
}

#[test]
fn escape_returns_from_settings_subpage_before_closing_settings_window() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let settings_window = window::Id::unique();
    browser.settings_window = Some(settings_window);
    browser.focused_window = settings_window;
    browser.selected_settings_category = SettingsCategory::Files;
    drop(browser.select_settings_subpage(SettingsSubpage::Preview));

    drop(browser.handle_focused_window_escape_pressed());
    assert_eq!(browser.settings_subpage, None);
    assert_eq!(browser.settings_window, Some(settings_window));

    drop(browser.handle_focused_window_escape_pressed());
    assert_eq!(browser.settings_window, None);
}
