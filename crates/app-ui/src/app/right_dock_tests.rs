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

/// 旧版本存下来的四格，份额合计 101018：加载时缩到 1000，画出来不能把后面的格子挤出栏外。
fn browser_with_inflated_stored_portions() -> FileBrowser {
    let (layout, _) = RightDockLayout::from_parts(
        true,
        400.0,
        true,
        [
            (vec!["preview"], 0, 20650),
            (vec!["info"], 0, 16368),
            (vec!["task_log"], 0, 32000),
            (vec!["quick_actions"], 0, 32000),
        ],
    );
    let mut user_config = config::default_user_config();
    user_config.right_dock = layout;
    let (mut browser, _) = FileBrowser::new(user_config);
    browser.main_window_width = 1400.0;
    browser.main_window_height = 1000.0;
    browser
}

/// 栏那一列在窗口里实际占的高度：窗格区顶到终端抽屉上沿，视图里栏就是这一段。
fn dock_column_height(browser: &FileBrowser) -> f32 {
    browser.main_window_height
        - crate::terminal_panel::view::terminal_panel_area_height(browser)
        - browser.main_panes_area_top()
}

fn frames_bottom(frames: &[GroupFrame]) -> f32 {
    frames
        .last()
        .map_or(0.0, |frame| frame.bounds.y + frame.bounds.height)
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
fn the_divider_keeps_both_groups_at_least_their_minimum_height() {
    for locked in [true, false] {
        let mut browser = browser_with_dock();
        if !locked {
            browser.right_dock.toggle_locked();
        }
        let frames = browser.right_dock_group_frames();
        let divider_y = frames[0].bounds.y + frames[0].bounds.height;
        press_at(
            &mut browser,
            RightDockMessage::DividerDragStarted(0),
            Point::new(1300.0, divider_y),
        );

        browser.update_right_dock_drag(Point::new(1300.0, -5000.0));
        let frames = browser.right_dock_group_frames();
        let upper_min = min_group_height(frames[0].header_height);
        // 份额的精度是 1/1000，拖到头会比最小高度多出不到 1px，但绝不会更矮。
        assert!((upper_min..upper_min + 1.0).contains(&frames[0].bounds.height));

        browser.update_right_dock_drag(Point::new(1300.0, 5000.0));
        let frames = browser.right_dock_group_frames();
        let lower_min = min_group_height(frames[1].header_height);
        assert!((lower_min..lower_min + 1.0).contains(&frames[1].bounds.height));
    }
}

#[test]
fn the_divider_boundary_follows_the_pointer_one_to_one() {
    for (divider, delta) in [(0, 40.0_f32), (1, -40.0), (2, 25.0)] {
        let mut browser = browser_with_inflated_stored_portions();
        let before = browser.right_dock_group_frames();
        let boundary =
            |frames: &[GroupFrame]| frames[divider].bounds.y + frames[divider].bounds.height;
        let start_y = boundary(&before) + DIVIDER_HEIGHT / 2.0;
        press_at(
            &mut browser,
            RightDockMessage::DividerDragStarted(divider),
            Point::new(1300.0, start_y),
        );

        browser.update_right_dock_drag(Point::new(1300.0, start_y + delta));

        let after = browser.right_dock_group_frames();
        // 份额是整数（合计 1000），所以最多差半份对应的几分之一像素。
        assert!(
            (boundary(&after) - boundary(&before) - delta).abs() < 0.5,
            "divider {divider}: {} vs {delta}",
            boundary(&after) - boundary(&before)
        );
        for (index, (old, new)) in before.iter().zip(&after).enumerate() {
            if index != divider && index != divider + 1 {
                assert!((old.bounds.height - new.bounds.height).abs() < 0.001);
            }
        }
        assert!((frames_bottom(&before) - frames_bottom(&after)).abs() < 0.001);
    }
}

#[test]
fn releasing_without_any_dock_drag_writes_nothing() {
    let mut browser = browser_with_dock();
    let before = browser.user_config.right_dock.clone();
    drop(browser.finish_right_dock_drag());
    assert_eq!(browser.user_config.right_dock, before);
}

#[test]
fn groups_share_the_height_above_their_minimums_by_portion_and_locked_lone_panels_have_no_header() {
    let browser = browser_with_dock();
    let frames = browser.right_dock_group_frames();
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| frame.header_height == 0.0));
    let spare = frames[0].bounds.height + frames[1].bounds.height
        - frames
            .iter()
            .map(|frame| min_group_height(frame.header_height))
            .sum::<f32>();
    let upper_share = (frames[0].bounds.height - min_group_height(0.0)) / spare;
    assert!((upper_share - config::DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO).abs() < 0.001);
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
fn the_frames_fill_the_dock_column_exactly_even_for_an_old_inflated_layout() {
    let browser = browser_with_inflated_stored_portions();
    let frames = browser.right_dock_group_frames();
    assert_eq!(frames.len(), 4);
    let heights: f32 = frames.iter().map(|frame| frame.bounds.height).sum();
    let dividers = DIVIDER_HEIGHT * 3.0;
    assert!((heights + dividers - dock_column_height(&browser)).abs() < 0.01);
    assert!(
        (frames_bottom(&frames)
            - (browser.main_window_height
                - crate::terminal_panel::view::terminal_panel_area_height(&browser)))
        .abs()
            < 0.01,
        "no group is pushed past the dock"
    );
    for frame in &frames {
        assert!(frame.bounds.height >= min_group_height(frame.header_height));
    }
    // 份额 20650:16368:32000:32000 的次序在高度上保持。
    assert!(frames[0].bounds.height < frames[2].bounds.height);
    assert!(frames[1].bounds.height < frames[0].bounds.height);
}

#[test]
fn a_dock_too_short_for_every_minimum_splits_its_height_equally() {
    let mut browser = browser_with_dock();
    for panel in [
        DockPanel::TaskLog,
        DockPanel::QuickActions,
        DockPanel::FolderTree,
    ] {
        browser.right_dock.open(panel);
    }
    browser.main_window_height = browser.main_panes_area_top()
        + crate::terminal_panel::view::terminal_panel_area_height(&browser)
        + 300.0;
    let frames = browser.right_dock_group_frames();
    assert_eq!(frames.len(), 5);
    let share = (300.0 - DIVIDER_HEIGHT * 4.0) / 5.0;
    for frame in &frames {
        assert!((frame.bounds.height - share).abs() < 0.01, "{frames:?}");
    }
    // 份额不影响这时画出来的高度，所以分隔线没有可拖的。
    press_at(
        &mut browser,
        RightDockMessage::DividerDragStarted(0),
        Point::new(1300.0, 200.0),
    );
    assert!(!matches!(
        browser.right_dock_drag,
        Some(RightDockDrag::Divider { .. })
    ));

    // 六格也一样：平分、没有一格是 0。
    let six = group_heights(
        260.0,
        &[
            (1, 0.0),
            (1, 0.0),
            (500, 28.0),
            (100, 28.0),
            (200, 0.0),
            (198, 28.0),
        ],
    );
    assert_eq!(six.len(), 6);
    assert!(six
        .iter()
        .all(|height| (height - 260.0 / 6.0).abs() < 0.001));
}

#[test]
fn the_preview_viewport_height_is_the_preview_group_content_height() {
    let mut browser = browser_with_dock();
    // 解锁后预览那格有标题栏，内容区比整格矮一个标题栏。
    browser.right_dock.toggle_locked();
    let frame = browser.right_dock_group_frames()[0];
    let content_height = frame.bounds.height - frame.header_height;
    assert!(
        content_height > 200.0,
        "tall enough to not hit the viewport floor"
    );
    assert_eq!(
        browser.right_preview_panel_viewport().height,
        content_height - 2.0 * crate::app::right_preview_panel::PANEL_CONTENT_PADDING
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

#[test]
fn showing_the_tasks_panel_closes_the_floating_task_list() {
    let mut browser = browser_with_dock();
    browser.operation_queue.open_panel();

    drop(browser.toggle_dock_panel(DockPanel::TaskLog));

    assert!(browser.right_dock.layout().shows(DockPanel::TaskLog));
    assert!(!browser.operation_queue.is_panel_open());
}

#[test]
fn the_menu_from_anywhere_in_the_dock_carries_the_hover_setting() {
    use crate::app::dock_hover::DockHoverMessage;
    use crate::app::folder_tree::FolderTreeMessage;
    use crate::app::info_panel::InfoPanelMessage;

    fn menu_shows_hovered_item(browser: &FileBrowser) -> bool {
        match &browser.context_menu {
            Some(ContextMenuState::RightDock(menu)) => menu.shows_hovered_item,
            other => panic!("expected the dock menu, got {other:?}"),
        }
    }

    let mut browser = browser_with_dock();
    drop(browser.handle_right_dock_message(RightDockMessage::MenuRequested));
    assert!(!menu_shows_hovered_item(&browser));

    drop(browser.handle_dock_hover_message(DockHoverMessage::ShowHoveredItemToggled));
    assert!(browser.context_menu.is_none(), "a click closes the menu");
    assert!(browser.user_config.dock_shows_hovered_item);

    drop(browser.handle_right_dock_message(RightDockMessage::MenuRequested));
    assert!(menu_shows_hovered_item(&browser));
    drop(browser.handle_info_panel_message(InfoPanelMessage::MenuRequested));
    assert!(menu_shows_hovered_item(&browser));
    drop(browser.handle_folder_tree_message(FolderTreeMessage::MenuRequested));
    assert!(menu_shows_hovered_item(&browser));
}
