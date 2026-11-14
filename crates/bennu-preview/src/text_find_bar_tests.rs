use iced::keyboard::{self, key, Key};

use super::*;

#[test]
fn the_summary_reads_ordinal_total_and_whether_counting_goes_on() {
    let summary = |ordinal, total, finished| {
        summary_label(Some(SearchSummary {
            ordinal,
            total,
            finished,
        }))
    };
    assert_eq!(summary(Some(3), 17, true), "3/17");
    assert_eq!(summary(Some(3), 17, false), "3/17+");
    assert_eq!(summary(None, 17, false), "–/17+");
    assert_eq!(summary(None, 0, true), tr!("No results"));
    assert_eq!(summary(None, 0, false), tr!("Searching…"));
    assert_eq!(summary_label(None), "");
}

fn keys() -> FindKeys<&'static str> {
    FindKeys {
        escape: "escape",
        enter: "enter",
        shift_enter: "shift_enter",
        next: "next",
        previous: "previous",
        open_search: "open_search",
        open_goto: "open_goto",
    }
}

#[test]
fn bar_keys_map_to_their_messages() {
    let keys = keys();
    let none = keyboard::Modifiers::empty();
    let shift = keyboard::Modifiers::SHIFT;
    let ctrl = keyboard::Modifiers::CTRL;
    let named = |named| Key::Named(named);
    assert_eq!(
        keys.message_for(&named(key::Named::Escape), none),
        Some("escape")
    );
    assert_eq!(
        keys.message_for(&named(key::Named::Enter), none),
        Some("enter")
    );
    assert_eq!(
        keys.message_for(&named(key::Named::Enter), shift),
        Some("shift_enter")
    );
    assert_eq!(keys.message_for(&named(key::Named::F3), none), Some("next"));
    assert_eq!(
        keys.message_for(&named(key::Named::F3), shift),
        Some("previous")
    );
    assert_eq!(
        keys.message_for(&Key::Character("F".into()), ctrl),
        Some("open_search")
    );
    assert_eq!(
        keys.message_for(&Key::Character("g".into()), ctrl),
        Some("open_goto")
    );
    // 普通字符归输入框。
    assert_eq!(keys.message_for(&Key::Character("f".into()), none), None);
    assert_eq!(keys.message_for(&named(key::Named::Space), none), None);
}

struct FakeFocus(bool);

impl Focusable for FakeFocus {
    fn is_focused(&self) -> bool {
        self.0
    }

    fn focus(&mut self) {
        self.0 = true;
    }

    fn unfocus(&mut self) {
        self.0 = false;
    }
}

#[test]
fn the_space_shortcut_passes_only_when_the_find_input_is_not_typing() {
    let check = |focused| {
        let mut check = FindInputFocusCheck {
            target: find_input_id(),
            focused: false,
            message: "close",
        };
        let other = widget::Id::new("other-input");
        Operation::<&str>::focusable(
            &mut check,
            Some(&other),
            Rectangle::default(),
            &mut FakeFocus(true),
        );
        Operation::<&str>::focusable(
            &mut check,
            Some(&find_input_id()),
            Rectangle::default(),
            &mut FakeFocus(focused),
        );
        matches!(Operation::<&str>::finish(&check), Outcome::Some("close"))
    };
    assert!(!check(true), "typing a space in the search field");
    assert!(check(false), "a space anywhere else closes the preview");
}
