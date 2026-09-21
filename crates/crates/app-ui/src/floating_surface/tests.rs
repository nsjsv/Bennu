use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestMessage {
    Dismiss,
}

fn dismissal(policy: OutsideDismissalPolicy) -> OutsideClickDismissal<TestMessage> {
    OutsideClickDismissal {
        message: TestMessage::Dismiss,
        policy,
    }
}

#[test]
fn modal_inside_floating_bounds_stops_background_without_dismissal() {
    let decision = decide_floating_input::<TestMessage>(
        BackgroundInputPolicy::Blocked,
        None,
        FloatingPointerTarget::FloatingBounds,
        FloatingInputEvent::PrimaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: None,
            background_update: BackgroundUpdateDecision::Stop
        }
    );
}

#[test]
fn context_menu_inside_right_click_stops_background_without_dismissal() {
    let dismissal = dismissal(OutsideDismissalPolicy::ContextMenuReplacement);

    let decision = decide_floating_input(
        BackgroundInputPolicy::Blocked,
        Some(&dismissal),
        FloatingPointerTarget::FloatingBounds,
        FloatingInputEvent::SecondaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: None,
            background_update: BackgroundUpdateDecision::Stop
        }
    );
}

#[test]
fn modal_outside_left_click_captures_without_dismissal() {
    let decision = decide_floating_input::<TestMessage>(
        BackgroundInputPolicy::Blocked,
        None,
        FloatingPointerTarget::Background,
        FloatingInputEvent::PrimaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: None,
            background_update: BackgroundUpdateDecision::Capture
        }
    );
}

#[test]
fn blocking_dismissible_outside_left_click_dismisses_and_captures() {
    let dismissal = dismissal(OutsideDismissalPolicy::CapturedPrimaryPress);

    let decision = decide_floating_input(
        BackgroundInputPolicy::Blocked,
        Some(&dismissal),
        FloatingPointerTarget::Background,
        FloatingInputEvent::PrimaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: Some(TestMessage::Dismiss),
            background_update: BackgroundUpdateDecision::Capture
        }
    );
}

#[test]
fn blocking_dismissible_outside_right_click_stays_open_and_captures() {
    let dismissal = dismissal(OutsideDismissalPolicy::CapturedPrimaryPress);

    let decision = decide_floating_input(
        BackgroundInputPolicy::Blocked,
        Some(&dismissal),
        FloatingPointerTarget::Background,
        FloatingInputEvent::SecondaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: None,
            background_update: BackgroundUpdateDecision::Capture
        }
    );
}

#[test]
fn context_menu_outside_left_click_dismisses_and_captures() {
    let dismissal = dismissal(OutsideDismissalPolicy::ContextMenuReplacement);

    let decision = decide_floating_input(
        BackgroundInputPolicy::Blocked,
        Some(&dismissal),
        FloatingPointerTarget::Background,
        FloatingInputEvent::PrimaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: Some(TestMessage::Dismiss),
            background_update: BackgroundUpdateDecision::Capture
        }
    );
}

#[test]
fn context_menu_outside_right_click_dismisses_and_updates_background() {
    let dismissal = dismissal(OutsideDismissalPolicy::ContextMenuReplacement);

    let decision = decide_floating_input(
        BackgroundInputPolicy::Blocked,
        Some(&dismissal),
        FloatingPointerTarget::Background,
        FloatingInputEvent::SecondaryPress,
    );

    assert_eq!(
        decision,
        FloatingInputDecision {
            dismiss_message: Some(TestMessage::Dismiss),
            background_update: BackgroundUpdateDecision::Update
        }
    );
}

#[test]
fn floating_overlay_captures_mouse_events_inside_bounds() {
    let event = Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
    });
    let bounds = Rectangle::new(Point::new(10.0, 10.0), Size::new(100.0, 100.0));

    assert!(should_capture_floating_overlay_event(
        &event,
        mouse::Cursor::Available(Point::new(20.0, 20.0)),
        bounds,
    ));
    assert!(!should_capture_floating_overlay_event(
        &event,
        mouse::Cursor::Available(Point::new(200.0, 200.0)),
        bounds,
    ));
}

/// 主窗口常驻 UI 高度:顶部工具栏行 72,底部终端窄条 28。
const TOOLBAR_HEIGHT: f32 = 72.0;
const BOTTOM_STRIP_HEIGHT: f32 = 28.0;

fn window_area() -> FloatingArea {
    FloatingArea {
        top: TOOLBAR_HEIGHT,
        bottom: BOTTOM_STRIP_HEIGHT,
    }
}

fn window_size() -> Size {
    Size::new(800.0, 500.0)
}

#[test]
fn center_panel_stays_inside_content_area_on_small_window() {
    let area = window_area();
    let panel = Size::new(200.0, 300.0);

    let position = floating_position(FloatingPlacement::Center, panel, window_size(), area, &[]);
    let bottom = position.y + panel.height;

    assert!(position.y >= TOOLBAR_HEIGHT + FLOATING_SURFACE_MARGIN);
    assert!(bottom <= window_size().height - BOTTOM_STRIP_HEIGHT);
}

#[test]
fn center_panel_clamps_below_toolbar_when_it_outgrows_content_area() {
    // 高度 350 的窗口扣掉工具栏与底部条只剩 250,放不下 300 高的面板:
    // 面板钉在工具栏下方,不再垂直居中压到工具栏按钮上。
    let area = window_area();
    let surface = Size::new(800.0, 350.0);
    let panel = Size::new(200.0, 300.0);

    let position = floating_position(FloatingPlacement::Center, panel, surface, area, &[]);

    assert_eq!(position.y, TOOLBAR_HEIGHT + FLOATING_SURFACE_MARGIN);
}

#[test]
fn bottom_left_panel_sits_above_bottom_strip() {
    let area = window_area();
    let panel = Size::new(360.0, 100.0);

    let position = floating_position(
        FloatingPlacement::BottomLeft {
            left: 120.0,
            bottom: 18.0,
        },
        panel,
        window_size(),
        area,
        &[],
    );

    assert_eq!(
        position.y + panel.height,
        window_size().height - BOTTOM_STRIP_HEIGHT - 18.0
    );
}

#[test]
fn at_placement_notification_shifts_below_toolbar() {
    // 错误通知固定 y=18,落在工具栏行内;安全区把它推到工具栏下方。
    let area = window_area();
    let panel = Size::new(560.0, 80.0);

    let position = floating_position(
        FloatingPlacement::At(Point::new(120.0, 18.0)),
        panel,
        window_size(),
        area,
        &[],
    );

    assert_eq!(position.y, TOOLBAR_HEIGHT + FLOATING_SURFACE_MARGIN);
}

#[test]
fn at_placement_inside_content_area_keeps_requested_position() {
    let area = window_area();
    let panel = Size::new(190.0, 200.0);

    let position = floating_position(
        FloatingPlacement::At(Point::new(300.0, 200.0)),
        panel,
        window_size(),
        area,
        &[],
    );

    assert_eq!(position, Point::new(300.0, 200.0));
}

#[test]
fn free_placement_still_follows_pointer_anywhere() {
    let area = window_area();
    let panel = Size::new(80.0, 40.0);

    let position = floating_position(
        FloatingPlacement::Free(Point::new(30.0, 10.0)),
        panel,
        window_size(),
        area,
        &[],
    );

    assert_eq!(position, Point::new(30.0, 10.0));
}

#[test]
fn anchor_bottom_right_still_expands_from_cursor() {
    let area = window_area();
    let panel = Size::new(80.0, 40.0);

    let position = floating_position(
        FloatingPlacement::AnchorBottomRight {
            anchor: Point::new(100.0, 60.0),
        },
        panel,
        window_size(),
        area,
        &[],
    );

    assert_eq!(position, Point::new(20.0, 20.0));
}

#[test]
fn center_max_height_excludes_reserved_chrome() {
    let max_size = floating_max_size(FloatingPlacement::Center, window_size(), window_area());

    assert_eq!(
        max_size.width,
        window_size().width - FLOATING_SURFACE_MARGIN * 2.0
    );
    assert_eq!(
        max_size.height,
        window_size().height - TOOLBAR_HEIGHT - BOTTOM_STRIP_HEIGHT - FLOATING_SURFACE_MARGIN * 2.0
    );
}

/// BesideParent 测试基准父浮层:已是钳制后矩形(resolved 表存的就是它)。
fn resolved_parent(x: f32, y: f32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(190.0, 200.0))
}

#[test]
fn beside_parent_sits_right_of_parent_when_it_fits() {
    let area = window_area();
    let resolved = vec![Some(resolved_parent(100.0, 80.0))];
    let panel = Size::new(170.0, 84.0);

    let position = floating_position(
        FloatingPlacement::BesideParent {
            parent: 0,
            top: 32.0,
        },
        panel,
        window_size(),
        area,
        &resolved,
    );

    assert_eq!(
        position,
        Point::new(100.0 + 190.0 + BESIDE_PARENT_GAP, 80.0 + 32.0)
    );
}

#[test]
fn beside_parent_flips_left_of_parent_when_right_overflows_safety_margin() {
    // 父浮层右缘 790,右侧 gap 4 + 子菜单宽 170 溢出安全区(782),翻到父左侧。
    let area = window_area();
    let resolved = vec![Some(resolved_parent(600.0, 80.0))];
    let panel = Size::new(170.0, 84.0);

    let position = floating_position(
        FloatingPlacement::BesideParent {
            parent: 0,
            top: 32.0,
        },
        panel,
        window_size(),
        area,
        &resolved,
    );

    assert_eq!(
        position,
        Point::new(600.0 - 170.0 - BESIDE_PARENT_GAP, 80.0 + 32.0)
    );
}

#[test]
fn beside_parent_clamps_bottom_into_safe_area() {
    // 父浮层靠下且触发行偏移大:子菜单底缘钳在安全区底(472)上方 18。
    let area = window_area();
    let resolved = vec![Some(resolved_parent(100.0, 400.0))];
    let panel = Size::new(170.0, 84.0);

    let position = floating_position(
        FloatingPlacement::BesideParent {
            parent: 0,
            top: 50.0,
        },
        panel,
        window_size(),
        area,
        &resolved,
    );

    assert_eq!(
        position,
        Point::new(100.0 + 190.0 + BESIDE_PARENT_GAP, 370.0)
    );
    assert_eq!(position.y + panel.height, 472.0 - FLOATING_SURFACE_MARGIN);
}

#[test]
fn beside_parent_clamps_top_into_safe_area() {
    // 父浮层高于工具栏触发行:子菜单顶缘钳在安全区顶(72)下方 18。
    let area = window_area();
    let resolved = vec![Some(resolved_parent(100.0, 40.0))];
    let panel = Size::new(170.0, 84.0);

    let position = floating_position(
        FloatingPlacement::BesideParent {
            parent: 0,
            top: 5.0,
        },
        panel,
        window_size(),
        area,
        &resolved,
    );

    assert_eq!(
        position,
        Point::new(
            100.0 + 190.0 + BESIDE_PARENT_GAP,
            TOOLBAR_HEIGHT + FLOATING_SURFACE_MARGIN
        )
    );
}
