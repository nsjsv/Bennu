use super::*;
use crate::model::BrowserPaneId;

#[test]
fn view_density_wheel_direction_prefers_y_and_maps_to_step() {
    assert_eq!(
        view_density_step_from_wheel(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
        Some(crate::config::ViewDensityStep::Increase)
    );
    assert_eq!(
        view_density_step_from_wheel(mouse::ScrollDelta::Pixels { x: 0.0, y: -2.0 }),
        Some(crate::config::ViewDensityStep::Decrease)
    );
    // 竖轴为零时回退横轴。
    assert_eq!(
        view_density_step_from_wheel(mouse::ScrollDelta::Lines { x: -3.0, y: 0.0 }),
        Some(crate::config::ViewDensityStep::Decrease)
    );
    assert_eq!(
        view_density_step_from_wheel(mouse::ScrollDelta::Lines { x: 0.0, y: 0.0 }),
        None
    );
}

#[test]
fn view_density_target_covers_only_browsing_content_regions() {
    let pane_id = BrowserPaneId::PRIMARY;
    assert_eq!(
        view_density_target(&ScrollbarRegion::PaneList(pane_id)),
        Some(ViewDensityTarget::List)
    );
    assert_eq!(
        view_density_target(&ScrollbarRegion::PaneIcons(pane_id)),
        Some(ViewDensityTarget::Icons)
    );
    assert_eq!(
        view_density_target(&ScrollbarRegion::Column {
            pane_id,
            directory: std::path::PathBuf::from("/tmp")
        }),
        Some(ViewDensityTarget::Columns)
    );
    assert_eq!(
        view_density_target(&ScrollbarRegion::ColumnBrowser(pane_id)),
        Some(ViewDensityTarget::Columns)
    );
    // 侧栏、地址栏、预览和设置不参与密度缩放。
    assert_eq!(view_density_target(&ScrollbarRegion::Sidebar), None);
    assert_eq!(
        view_density_target(&ScrollbarRegion::AddressBar(pane_id)),
        None
    );
    assert_eq!(view_density_target(&ScrollbarRegion::Settings), None);
    assert_eq!(view_density_target(&ScrollbarRegion::TextPreview), None);
}

#[test]
fn ctrl_wheel_on_icon_grid_changes_level_without_starting_scroll() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let region = ScrollbarRegion::PaneIcons(BrowserPaneId::PRIMARY);
    drop(browser.handle_smooth_scroll_wheel(
        region.clone(),
        mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
    ));
    assert!(browser.smooth_scroll.is_active());

    browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;
    drop(browser.handle_smooth_scroll_wheel(region, mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }));

    assert_eq!(browser.user_config.icons_view_density.index(), 3);
    assert_eq!(browser.user_config.icons_icon_edge(), 112);
    assert_eq!(browser.user_config.icon_grid_size, 112);
    assert!(!browser.smooth_scroll.is_active());
}

#[test]
fn ctrl_wheel_adjusts_list_and_columns_levels_independently() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;

    drop(browser.handle_smooth_scroll_wheel(
        ScrollbarRegion::PaneList(BrowserPaneId::PRIMARY),
        mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
    ));
    drop(browser.handle_smooth_scroll_wheel(
        ScrollbarRegion::PaneList(BrowserPaneId::PRIMARY),
        mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
    ));
    drop(browser.handle_smooth_scroll_wheel(
        ScrollbarRegion::Column {
            pane_id: BrowserPaneId::PRIMARY,
            directory: std::path::PathBuf::from("/tmp"),
        },
        mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
    ));
    drop(browser.handle_smooth_scroll_wheel(
        ScrollbarRegion::ColumnBrowser(BrowserPaneId::PRIMARY),
        mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
    ));

    assert_eq!(browser.user_config.list_view_density.index(), 4);
    assert_eq!(browser.user_config.columns_view_density.index(), 0);
    assert_eq!(browser.user_config.icons_view_density.index(), 2);
    assert!(!browser.smooth_scroll.is_active());
}

#[test]
fn ctrl_wheel_stops_silently_at_density_boundaries() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;
    browser.user_config.list_view_density = crate::config::ViewDensityLevel::from_index(8);

    let list_region = ScrollbarRegion::PaneList(BrowserPaneId::PRIMARY);
    drop(browser.handle_smooth_scroll_wheel(
        list_region.clone(),
        mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
    ));
    assert_eq!(browser.user_config.list_view_density.index(), 8);

    browser.user_config.list_view_density = crate::config::ViewDensityLevel::from_index(0);
    drop(
        browser
            .handle_smooth_scroll_wheel(list_region, mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 }),
    );
    assert_eq!(browser.user_config.list_view_density.index(), 0);
}

#[test]
fn plain_wheel_without_ctrl_keeps_normal_scroll_semantics() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let list_region = ScrollbarRegion::PaneList(BrowserPaneId::PRIMARY);
    drop(
        browser
            .handle_smooth_scroll_wheel(list_region, mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
    );

    assert_eq!(browser.user_config.list_view_density.index(), 2);
    assert!(browser.smooth_scroll.is_active());
}
