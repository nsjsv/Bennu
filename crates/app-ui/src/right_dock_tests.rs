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
        assert!(group.portion() >= 1, "portion at least 1: {layout:?}");
        for panel in group.panels() {
            assert!(!seen.contains(panel), "each panel once: {layout:?}");
            seen.push(*panel);
        }
    }
    if layout.groups().is_empty() {
        assert!(!layout.is_visible(), "no panels, not shown: {layout:?}");
    } else {
        let total: u32 = layout
            .groups()
            .iter()
            .map(|group| u32::from(group.portion()))
            .sum();
        assert_eq!(total, u32::from(PORTION_SCALE), "portions sum: {layout:?}");
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
    assert_eq!(portions(&dock), vec![250, 750]);
    // 拖到头也给两格各留一份。
    dock.set_split(0, 0.0);
    assert_invariants(&dock);
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
    // 份额 1:700 缩到合计 1000：第一格留 1，不会被缩没。
    assert_eq!(
        groups,
        vec![(vec![Info], Info, 1), (vec![Preview], Preview, 999)]
    );

    let (clean, dropped) =
        RightDockLayout::from_parts(true, 400.0, true, [(vec!["preview"], 0, 500)]);
    assert!(!dropped && clean.is_visible());

    let (empty, _) = RightDockLayout::from_parts(true, 400.0, true, []);
    assert!(!empty.is_visible(), "nothing to show");
}

fn portions(dock: &RightDock) -> Vec<u16> {
    dock.layout()
        .groups()
        .iter()
        .map(DockGroup::portion)
        .collect()
}

#[test]
fn a_new_group_gets_an_equal_share_and_the_others_shrink_in_proportion() {
    let mut dock = shown_dock();
    dock.set_split(0, 0.8);
    assert_eq!(portions(&dock), vec![800, 200]);

    dock.open(DockPanel::TaskLog);
    assert_invariants(&dock);
    let shares = portions(&dock);
    assert!((333..=334).contains(&shares[2]), "{shares:?}");
    assert!(
        (i32::from(shares[0]) - 4 * i32::from(shares[1])).abs() <= 4,
        "the old groups keep their 4:1 ratio: {shares:?}"
    );
}

#[test]
fn opening_and_closing_panels_over_and_over_never_inflates_the_portions() {
    let mut dock = shown_dock();
    for _ in 0..50 {
        for panel in DockPanel::ALL {
            dock.toggle_panel(panel);
            assert_invariants(&dock);
        }
    }
    for panel in DockPanel::ALL {
        dock.open(panel);
        assert_invariants(&dock);
    }
}

#[test]
fn any_sequence_of_operations_keeps_the_portions_summing_to_the_scale() {
    // 固定种子的伪随机序列：开、关、移动、拖分隔线、切标签混着来，每一步之后查不变量。
    let mut seed = 0x2545_f491_u32;
    let mut next = |bound: usize| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as usize % bound
    };
    let mut dock = shown_dock();
    for _ in 0..3000 {
        let panel = DockPanel::ALL[next(DockPanel::ALL.len())];
        let group = next(4);
        match next(7) {
            0 => dock.toggle_panel(panel),
            1 => dock.open(panel),
            2 => dock.close(panel),
            3 => dock.move_panel(panel, DropTarget::Join(group)),
            4 => dock.move_panel(panel, DropTarget::Above(group)),
            5 => dock.move_panel(panel, DropTarget::Below(group)),
            _ => dock.set_split(group, next(1001) as f32 / 1000.0),
        }
        assert_invariants(&dock);
    }
}

#[test]
fn splitting_a_group_down_to_a_single_portion_still_sums_to_the_scale() {
    let mut dock = shown_dock();
    dock.set_split(0, 0.0);
    assert_eq!(portions(&dock), vec![1, 999]);
    dock.move_panel(Info, DropTarget::Below(0));
    dock.move_panel(Info, DropTarget::Above(0));
    assert_invariants(&dock);
}

#[test]
fn a_stored_layout_whose_portions_grew_past_u16_is_scaled_down_in_proportion() {
    // 旧版本存下来的：合计 101018，渲染时会让 iced 的 u16 回绕。
    let stored = [20650_u16, 16368, 32000, 32000];
    let (layout, dropped) = RightDockLayout::from_parts(
        true,
        400.0,
        true,
        [
            (vec!["preview"], 0, stored[0]),
            (vec!["info"], 0, stored[1]),
            (vec!["task_log"], 0, stored[2]),
            (vec!["quick_actions"], 0, stored[3]),
        ],
    );
    assert!(!dropped);
    let dock = RightDock::new(layout);
    assert_invariants(&dock);
    let total: f32 = stored.iter().map(|portion| f32::from(*portion)).sum();
    for (portion, original) in portions(&dock).into_iter().zip(stored) {
        let expected = f32::from(original) / total * f32::from(PORTION_SCALE);
        assert!(
            (f32::from(portion) - expected).abs() <= 1.0,
            "{portion} vs {expected}"
        );
    }
}
