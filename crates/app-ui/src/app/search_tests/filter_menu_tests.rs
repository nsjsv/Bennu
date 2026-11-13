use std::path::PathBuf;

use iced::event;
use iced::keyboard::{self, key};

use super::{browser_for_search_tests, submit_search};
use crate::model::{
    BrowserLocation, SearchDatePreset, SearchFilterMenu, SearchFilterPick, SearchSizePreset,
};

fn press_escape(browser: &mut super::FileBrowser) {
    drop(browser.handle_keyboard_key_pressed(
        keyboard::Key::Named(key::Named::Escape),
        keyboard::Modifiers::default(),
        event::Status::Ignored,
    ));
}

#[test]
fn filter_menus_only_open_inside_a_search() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.open_search_filter_menu(SearchFilterMenu::Types));
    assert!(browser.search_filter_menu.is_none());

    drop(submit_search(&mut browser));
    drop(browser.open_search_filter_menu(SearchFilterMenu::Types));
    assert_eq!(browser.search_filter_menu, Some(SearchFilterMenu::Types));
}

#[test]
fn picking_a_value_applies_it_and_closes_the_menu() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    let generation = browser.active_search().unwrap().run.generation;

    drop(browser.open_search_filter_menu(SearchFilterMenu::Time));
    drop(browser.pick_search_filter(SearchFilterPick::DatePreset(
        SearchDatePreset::PastSevenDays,
    )));

    assert!(browser.search_filter_menu.is_none());
    let search = browser.active_search().unwrap();
    assert_eq!(search.filters.date_preset, SearchDatePreset::PastSevenDays);
    assert!(search.run.generation > generation);
}

#[test]
fn choosing_custom_size_keeps_the_menu_open_for_its_bounds() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));

    drop(browser.open_search_filter_menu(SearchFilterMenu::Size));
    drop(browser.pick_search_filter(SearchFilterPick::Size(SearchSizePreset::Custom)));
    assert_eq!(browser.search_filter_menu, Some(SearchFilterMenu::Size));
    assert_eq!(
        browser.active_search().unwrap().filters.size_preset,
        SearchSizePreset::Custom
    );

    drop(browser.pick_search_filter(SearchFilterPick::Size(SearchSizePreset::Large)));
    assert!(browser.search_filter_menu.is_none());
}

#[test]
fn escape_closes_the_open_menu_before_leaving_the_search() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    drop(browser.open_search_filter_menu(SearchFilterMenu::Scope));

    press_escape(&mut browser);
    assert!(browser.search_filter_menu.is_none());
    assert!(browser.active_search().is_some());

    press_escape(&mut browser);
    assert!(browser.active_search().is_none());
}

#[test]
fn leaving_the_search_by_navigation_closes_the_menu() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    drop(browser.open_search_filter_menu(SearchFilterMenu::TextScope));

    drop(browser.navigate_to(
        PathBuf::from("/elsewhere"),
        crate::model::NavigationMode::RecordHistory,
    ));

    assert!(matches!(
        browser.tab().location,
        BrowserLocation::Directory(_)
    ));
    assert!(browser.search_filter_menu.is_none());
}
