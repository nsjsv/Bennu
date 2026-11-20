use super::*;

#[test]
fn item_counts_combine_exact_numbers_with_bounds() {
    let any = ItemCount::default();
    assert!(any.allows(1) && any.allows(40));

    let pairs = ItemCount {
        exactly: vec![2, 4],
        ..ItemCount::default()
    };
    assert!(pairs.allows(2) && pairs.allows(4));
    assert!(!pairs.allows(3));

    let between = ItemCount {
        exactly: Vec::new(),
        at_least: Some(2),
        at_most: Some(3),
    };
    assert!(!between.allows(1));
    assert!(between.allows(2) && between.allows(3));
    assert!(!between.allows(4));
}

#[test]
fn service_menu_ids_round_trip_through_their_stored_key() {
    let id = ActionId::service_menu("convert.desktop", "toMp3");
    assert_eq!(id.config_key(), "servicemenu:convert.desktop/toMp3");
    assert_eq!(
        ActionId::from_config_key(&id.config_key()),
        Some(id.clone())
    );
    assert_eq!(id.fallback_label(), "convert.desktop/toMp3");
    assert_eq!(
        ActionId::from_config_key("action:extract-audio"),
        Some(ActionId::from_file_stem("extract-audio"))
    );
    for broken in [
        "servicemenu:",
        "servicemenu:a.desktop",
        "servicemenu:/x",
        "servicemenu:a/",
        "action:",
    ] {
        assert_eq!(ActionId::from_config_key(broken), None, "{broken}");
    }
}
