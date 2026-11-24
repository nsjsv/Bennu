use super::*;

/// 模块注释里那几条：每个操作之后都要成立。
fn assert_invariants(dock: &RightDock) {
    let layout = dock.layout();
    let mut seen = Vec::new();
    for group in layout.groups() {
        assert!(!group.panels().is_empty(), "no empty group: {layout:?}");
        assert!(
            group.active < group.panels().len(),
            "active in range: {layout:?}"
        );
        assert!(group.portion() > 0, "positive portion: {layout:?}");
        for panel in group.panels() {
            assert!(!seen.contains(panel), "each panel once: {layout:?}");
            seen.push(*panel);
        }
    }
    if layout.groups().is_empty() {
        assert!(!layout.is_visible(), "no panels, not shown: {layout:?}");
    }
}

fn shown_dock() -> RightDock {
    let mut dock = RightDock::new(RightDockLayout::default());
    dock.toggle_visible();
    assert_invariants(&dock);
    dock
}

fn arrangement(dock: &RightDock) -> Vec<(Vec<DockPanel>, DockPanel)> {
    dock.layout()
        .groups()
        .iter()
        .map(|group| (group.panels().to_vec(), group.active_panel()))
        .collect()
}

use DockPanel::{Info, Preview};

#[test]
fn the_default_is_preview_above_info_like_the_old_panel() {
    let dock = shown_dock();
    assert_eq!(
        arrangement(&dock),
        vec![(vec![Preview], Preview), (vec![Info], Info)]
    );
    assert!(dock.layout().is_locked());
    assert!(dock.layout().shows(Preview) && dock.layout().shows(Info));
}

#[test]
fn the_old_panel_settings_become_the_same_layout() {
    let layout = RightDockLayout::from_previous_panel(true, 480.0, 0.8);
    assert!(layout.is_visible());
    assert_eq!(layout.width(), 480.0);
    let portions: Vec<u16> = layout.groups().iter().map(DockGroup::portion).collect();
    assert_eq!(portions, vec![800, 200]);
    // 旧版能拉到 100%：信息那格也留一份，不出现份额为 0 的格子。
    let full = RightDockLayout::from_previous_panel(true, 320.0, 1.0);
    assert!(full.groups().iter().all(|group| group.portion() > 0));
}

#[test]
fn dropping_onto_a_group_stacks_tabs_and_dropping_on_an_edge_splits_again() {
    let mut dock = shown_dock();
    dock.move_panel(Info, DropTarget::Join(0));
    assert_invariants(&dock);
    assert_eq!(arrangement(&dock), vec![(vec![Preview, Info], Info)]);

    dock.move_panel(Info, DropTarget::Above(0));
    assert_invariants(&dock);
    assert_eq!(
        arrangement(&dock),
        vec![(vec![Info], Info), (vec![Preview], Preview)]
    );

    dock.move_panel(Info, DropTarget::Below(1));
    assert_invariants(&dock);
    assert_eq!(
        arrangement(&dock),
        vec![(vec![Preview], Preview), (vec![Info], Info)]
    );
}

#[test]
fn dropping_a_lone_panel_onto_its_own_group_changes_nothing() {
    let mut dock = shown_dock();
    let before = dock.layout().clone();
    for target in [
        DropTarget::Join(1),
        DropTarget::Above(1),
        DropTarget::Below(1),
    ] {
        dock.move_panel(Info, target);
        assert_eq!(dock.layout(), &before);
    }
}

#[test]
fn a_tab_shows_only_while_it_is_the_current_one() {
    let mut dock = shown_dock();
    dock.move_panel(Info, DropTarget::Join(0));
    assert!(dock.layout().shows(Info) && !dock.layout().shows(Preview));

    dock.select(Preview);
    assert!(dock.layout().shows(Preview) && !dock.layout().shows(Info));

    dock.toggle_visible();
    assert!(!dock.layout().shows(Preview), "a hidden dock shows nothing");
}

#[test]
fn closing_the_last_panel_hides_the_dock_and_showing_it_brings_the_default_back() {
    let mut dock = shown_dock();
    dock.close(Preview);
    dock.close(Info);
    assert_invariants(&dock);
    assert!(!dock.layout().is_visible());

    dock.toggle_visible();
    assert_invariants(&dock);
    assert_eq!(
        arrangement(&dock),
        vec![(vec![Preview], Preview), (vec![Info], Info)]
    );
}

#[test]
fn a_closed_panel_comes_back_where_it_was() {
    // 自己一格：回到原来的格子序号。
    let mut dock = shown_dock();
    dock.close(Preview);
    assert_eq!(arrangement(&dock), vec![(vec![Info], Info)]);
    dock.open(Preview);
    assert_invariants(&dock);
    assert_eq!(
        arrangement(&dock),
        vec![(vec![Preview], Preview), (vec![Info], Info)]
    );

    // 跟别的面板叠着：回到那个面板的格子里，成为当前标签。
    dock.move_panel(Info, DropTarget::Join(0));
    dock.close(Info);
    dock.move_panel(Preview, DropTarget::Below(0));
    dock.open(Info);
    assert_invariants(&dock);
    assert_eq!(arrangement(&dock), vec![(vec![Preview, Info], Info)]);
}

#[test]
fn opening_a_panel_while_the_dock_is_hidden_shows_the_dock() {
    let mut dock = RightDock::new(RightDockLayout::default());
    dock.toggle_panel(Info);
    assert!(dock.layout().is_visible() && dock.layout().shows(Info));
    // 再按一次是关掉它（显示着的面板）。
    dock.toggle_panel(Info);
    assert!(!dock.layout().contains(Info));
    assert_invariants(&dock);
}

#[test]
fn closing_the_current_tab_shows_its_neighbour() {
    let mut dock = shown_dock();
    dock.move_panel(Info, DropTarget::Join(0));
    dock.close(Info);
    assert_invariants(&dock);
    assert_eq!(arrangement(&dock), vec![(vec![Preview], Preview)]);
}

#[test]
fn a_split_moves_share_between_two_neighbours_only() {
    let mut dock = shown_dock();
    dock.set_split(0, 0.25);
    let portions: Vec<u16> = dock
        .layout()
        .groups()
        .iter()
        .map(DockGroup::portion)
        .collect();
    assert_eq!(portions, vec![250, 750]);
    // 拖到头也给两格各留一份。
    dock.set_split(0, 0.0);
    assert!(dock
        .layout()
        .groups()
        .iter()
        .all(|group| group.portion() > 0));
    // 最下面一格下面没有分隔线。
    let before = dock.layout().clone();
    dock.set_split(1, 0.5);
    assert_eq!(dock.layout(), &before);
}

#[test]
fn stored_parts_are_tidied_and_report_what_was_dropped() {
    let (layout, dropped) = RightDockLayout::from_parts(
        true,
        f32::NAN,
        false,
        [
            (vec!["info", "newer-panel"], 1, 0),
            (vec!["info"], 0, 300),
            (vec![], 0, 300),
            (vec!["preview"], 5, 700),
        ],
    );
    assert!(dropped);
    assert_eq!(layout.width(), DEFAULT_RIGHT_DOCK_WIDTH);
    assert!(!layout.is_locked());
    let groups: Vec<(Vec<DockPanel>, DockPanel, u16)> = layout
        .groups()
        .iter()
        .map(|group| {
            (
                group.panels().to_vec(),
                group.active_panel(),
                group.portion(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        vec![(vec![Info], Info, 1), (vec![Preview], Preview, 700)]
    );

    let (clean, dropped) =
        RightDockLayout::from_parts(true, 400.0, true, [(vec!["preview"], 0, 500)]);
    assert!(!dropped && clean.is_visible());

    let (empty, _) = RightDockLayout::from_parts(true, 400.0, true, []);
    assert!(!empty.is_visible(), "nothing to show");
}
