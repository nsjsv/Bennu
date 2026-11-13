use super::{time_label, types_label};
use crate::model::search::SearchFilterPresetState;
use crate::model::{SearchDateField, SearchDatePreset, SearchEntryTypePreset};

fn all_types() -> Vec<SearchEntryTypePreset> {
    SearchEntryTypePreset::ALL.to_vec()
}

#[test]
fn the_type_button_names_up_to_two_types_then_counts() {
    let mut filters = SearchFilterPresetState::default();
    assert_eq!(types_label(&filters, &all_types()), "All types");

    filters.toggle_entry_type(SearchEntryTypePreset::Pdf);
    assert_eq!(types_label(&filters, &all_types()), "PDF");

    // 名字按下拉里的顺序写，不按点的先后。
    filters.toggle_entry_type(SearchEntryTypePreset::Images);
    assert_eq!(types_label(&filters, &all_types()), "Images, PDF");

    filters.custom_extensions = "psd".to_owned();
    assert_eq!(types_label(&filters, &all_types()), "3 types");
}

#[test]
fn custom_extensions_count_as_one_type() {
    let filters = SearchFilterPresetState {
        custom_extensions: "psd, ai".to_owned(),
        ..SearchFilterPresetState::default()
    };
    assert_eq!(types_label(&filters, &all_types()), "Custom");
}

#[test]
fn a_selected_type_hidden_in_settings_still_shows_on_the_button() {
    let mut filters = SearchFilterPresetState::default();
    filters.toggle_entry_type(SearchEntryTypePreset::Links);
    assert_eq!(
        types_label(&filters, &[SearchEntryTypePreset::Images]),
        "Links"
    );
}

#[test]
fn the_time_button_names_the_field_and_the_range() {
    let mut filters = SearchFilterPresetState::default();
    assert_eq!(time_label(&filters), "Any time");
    filters.date_preset = SearchDatePreset::PastSevenDays;
    assert_eq!(time_label(&filters), "Modified · Past 7 days");
    filters.date_field = SearchDateField::Created;
    assert_eq!(time_label(&filters), "Created · Past 7 days");
}
