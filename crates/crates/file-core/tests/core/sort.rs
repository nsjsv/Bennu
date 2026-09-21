use super::*;

#[test]
fn hidden_filter_removes_dot_entries_when_disabled() {
    let root = Path::new("/tmp");
    let mut entries = vec![
        entry(root.join(".hidden"), FileKind::File, 0, true),
        entry(root.join("shown"), FileKind::File, 0, false),
    ];

    filter_hidden(&mut entries, false);

    assert_eq!(names(&entries), vec!["shown"]);
}

#[test]
fn sort_entries_keeps_directories_first_then_sorts_by_name() {
    let root = Path::new("/tmp");
    let mut entries = vec![
        entry(root.join("z.txt"), FileKind::File, 2, false),
        entry(root.join("a-dir"), FileKind::Directory, 0, false),
        entry(root.join("a.txt"), FileKind::File, 1, false),
    ];
    let options = ScanOptions {
        include_hidden: true,
        sort_field: SortField::Name,
        sort_direction: SortDirection::Ascending,
        directories_first: true,
    };

    sort_entries(&mut entries, &options);

    assert_eq!(names(&entries), vec!["a-dir", "a.txt", "z.txt"]);
}

#[test]
fn sort_entries_can_sort_by_size_descending() {
    let root = Path::new("/tmp");
    let mut entries = vec![
        entry(root.join("small"), FileKind::File, 1, false),
        entry(root.join("large"), FileKind::File, 10, false),
    ];
    let options = ScanOptions {
        include_hidden: true,
        sort_field: SortField::Size,
        sort_direction: SortDirection::Descending,
        directories_first: false,
    };

    sort_entries(&mut entries, &options);

    assert_eq!(names(&entries), vec!["large", "small"]);
}
