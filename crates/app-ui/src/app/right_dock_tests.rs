use iced::Point;

use super::*;
use crate::config;
use crate::right_dock::RightDockLayout;

fn browser_with_dock() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.main_window_width = 1400.0;
    browser.main_window_height = 900.0;
    browser.right_dock.toggle_visible();
    browser
}

fn press_at(browser: &mut FileBrowser, message: RightDockMessage, at: Point) {
    browser.cursor_position.set(Some(at));
    drop(browser.handle_right_dock_message(message));
}

fn arrangement(browser: &FileBrowser) -> Vec<Vec<DockPanel>> {
    browser
        .right_dock
        .layout()
        .groups()
        .iter()
        .map(|group| group.panels().to_vec())
        .collect()
}

#[test]
fn the_width_drag_stays_in_bounds_and_is_saved_when_released() {
    let mut browser = browser_with_dock();
    let start = browser.right_dock.layout().width();
    press_at(
        &mut browser,
        RightDockMessage::WidthDragStarted,
        Point::new(1000.0, 300.0),
    );

    // 往左拖加宽，拖过头夹在上限。
    browser.update_right_dock_drag(Point::new(-2000.0, 300.0));
    assert_eq!(
        browser.right_dock.layout().width(),
        config::MAX_RIGHT_DOCK_WIDTH
    );
    // 往右拖收窄，夹在下限。
    browser.update_right_dock_drag(Point::new(3000.0, 300.0));
    assert_eq!(
        browser.right_dock.layout().width(),
        config::MIN_RIGHT_DOCK_WIDTH
    );
    assert_eq!(
        browser.user_config.right_dock.width(),
        start,
        "nothing saved while dragging"
    );

    drop(browser.finish_right_dock_drag());
    assert_eq!(
        browser.user_config.right_dock.width(),
        config::MIN_RIGHT_DOCK_WIDTH
    );
    assert!(!browser.right_dock_width_drag_is_active());
}

#[test]
fn the_divider_keeps_both_groups_tall_enough() {
    let mut browser = browser_with_dock();
    let frames = browser.right_dock_group_frames();
    let divider_y = frames[0].bounds.y + frames[0].bounds.height;
    press_at(
        &mut browser,
        RightDockMessage::DividerDragStarted(0),
        Point::new(1300.0, divider_y),
    );

    browser.update_right_dock_drag(Point::new(1300.0, -5000.0));
    let frames = browser.right_dock_group_frames();
    assert!((frames[0].bounds.height - MIN_GROUP_HEIGHT).abs() < 2.0);

    browser.update_right_dock_drag(Point::new(1300.0, 5000.0));
    let frames = browser.right_dock_group_frames();
    assert!((frames[1].bounds.height - MIN_GROUP_HEIGHT).abs() < 2.0);
}

#[test]
fn releasing_without_any_dock_drag_writes_nothing() {
    let mut browser = browser_with_dock();
    let before = browser.user_config.right_dock.clone();
    drop(browser.finish_right_dock_drag());
    assert_eq!(browser.user_config.right_dock, before);
}

#[test]
fn groups_split_the_height_by_their_share_and_locked_lone_panels_have_no_header() {
    let browser = browser_with_dock();
    let frames = browser.right_dock_group_frames();
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| frame.header_height == 0.0));
    let ratio = frames[0].bounds.height / (frames[0].bounds.height + frames[1].bounds.height);
    assert!((ratio - config::DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO).abs() < 0.01);
    assert_eq!(
        frames[1].bounds.y,
        frames[0].bounds.y + frames[0].bounds.height + DIVIDER_HEIGHT
    );
    assert_eq!(
        frames[0].bounds.x,
        1400.0 - browser.right_dock_effective_width()
    );
}

#[test]
fn an_unlocked_panel_dragged_onto_another_group_joins_it() {
    let mut browser = browser_with_dock();
    browser.right_dock.toggle_locked();
    let frames = browser.right_dock_group_frames();
    let info = frames[1].bounds;
    let preview = frames[0].bounds;
    press_at(
        &mut browser,
        RightDockMessage::PanelPressed(DockPanel::Info),
        Point::new(info.x + 20.0, info.y + 10.0),
    );
    let middle = Point::new(preview.x + 40.0, preview.y + preview.height / 2.0);
    browser.update_right_dock_drag(middle);
    assert_eq!(
        browser.right_dock_panel_drop(),
        Some((DockPanel::Info, DropTarget::Join(0)))
    );

    drop(browser.finish_right_dock_drag());

    assert_eq!(
        arrangement(&browser),
        vec![vec![DockPanel::Preview, DockPanel::Info]]
    );
    assert_eq!(browser.user_config.right_dock, *browser.right_dock.layout());
}

#[test]
fn drop_zones_are_the_edges_and_the_middle_of_a_group_and_nothing_outside() {
    let mut browser = browser_with_dock();
    browser.right_dock.toggle_locked();
    let preview = browser.right_dock_group_frames()[0].bounds;
    press_at(
        &mut browser,
        RightDockMessage::PanelPressed(DockPanel::Info),
        Point::new(preview.x + 10.0, preview.y + preview.height + 50.0),
    );
    let x = preview.x + 30.0;
    let cases = [
        (Point::new(x, preview.y + 5.0), Some(DropTarget::Above(0))),
        (
            Point::new(x, preview.y + preview.height / 2.0),
            Some(DropTarget::Join(0)),
        ),
        (
            Point::new(x, preview.y + preview.height - 5.0),
            Some(DropTarget::Below(0)),
        ),
        (Point::new(preview.x - 20.0, preview.y + 50.0), None),
    ];
    for (at, expected) in cases {
        browser.update_right_dock_drag(at);
        assert_eq!(
            browser.right_dock_panel_drop().map(|(_, target)| target),
            expected,
            "{at:?}"
        );
    }
    // 拖出停靠区松开：取消，布局不变。
    let before = browser.right_dock.layout().clone();
    drop(browser.finish_right_dock_drag());
    assert_eq!(browser.right_dock.layout(), &before);
}

#[test]
fn a_press_without_moving_just_selects_the_tab_and_a_locked_dock_never_drags() {
    let mut browser = browser_with_dock();
    browser
        .right_dock
        .move_panel(DockPanel::Info, DropTarget::Join(0));
    browser.right_dock.select(DockPanel::Preview);

    // 锁定：按下就切过去，没有拖动。
    press_at(
        &mut browser,
        RightDockMessage::PanelPressed(DockPanel::Info),
        Point::new(1300.0, 100.0),
    );
    assert!(browser.right_dock.layout().shows(DockPanel::Info));
    browser.update_right_dock_drag(Point::new(1300.0, 600.0));
    assert_eq!(browser.right_dock_panel_drop(), None);

    // 解锁：按下、原地松开 = 点了这个标签。
    browser.right_dock.toggle_locked();
    press_at(
        &mut browser,
        RightDockMessage::PanelPressed(DockPanel::Preview),
        Point::new(1300.0, 100.0),
    );
    drop(browser.finish_right_dock_drag());
    assert!(browser.right_dock.layout().shows(DockPanel::Preview));
    assert_eq!(
        arrangement(&browser),
        vec![vec![DockPanel::Preview, DockPanel::Info]]
    );
}

#[test]
fn the_panes_give_up_the_dock_width_only_while_it_is_shown() {
    let mut browser = browser_with_dock();
    let shown = browser.split_content_width();
    drop(browser.toggle_right_dock());
    let hidden = browser.split_content_width();
    assert_eq!(hidden - shown, browser.right_dock_effective_width());
    assert!(
        !browser.user_config.right_dock.is_visible(),
        "hiding is saved"
    );
}

#[test]
fn the_menu_unticks_a_panel_hidden_behind_another_tab_by_closing_it() {
    let mut browser = browser_with_dock();
    browser
        .right_dock
        .move_panel(DockPanel::Info, DropTarget::Join(0));
    browser.right_dock.select(DockPanel::Preview);

    drop(browser.handle_right_dock_message(RightDockMessage::PanelToggled(DockPanel::Info)));

    assert!(!browser.right_dock.layout().contains(DockPanel::Info));
    // 快捷键不一样：藏在后面的切到前面来。
    drop(browser.toggle_dock_panel(DockPanel::Info));
    drop(browser.toggle_dock_panel(DockPanel::Preview));
    assert!(browser.right_dock.layout().shows(DockPanel::Preview));
}

#[test]
fn a_saved_layout_is_what_a_new_window_starts_with() {
    let mut user_config = config::default_user_config();
    user_config.right_dock = RightDockLayout::from_previous_panel(true, 500.0, 0.5);
    let (browser, _) = FileBrowser::new(user_config);
    assert!(browser.right_dock.layout().is_visible());
    assert_eq!(browser.right_dock.layout().width(), 500.0);
}
