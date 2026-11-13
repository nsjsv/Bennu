// 区间数组就是“要加粗的片段”，只有一个片段的写法是本意，不是误写的序列。
#![allow(clippy::single_range_in_vec_init)]

use std::path::Path;

use super::{emphasized_spans, location_text};

#[test]
fn location_shortens_the_home_folder() {
    let home = Path::new("/home/user");
    assert_eq!(location_text(Path::new("/home/user/a.txt"), home), "~");
    assert_eq!(
        location_text(Path::new("/home/user/docs/notes/a.txt"), home),
        "~/docs/notes"
    );
    assert_eq!(
        location_text(Path::new("/srv/data/a.txt"), home),
        "/srv/data"
    );
}

#[test]
fn location_keeps_full_paths_when_home_is_the_root() {
    assert_eq!(
        location_text(Path::new("/srv/data/a.txt"), Path::new("/")),
        "/srv/data"
    );
}

#[test]
fn emphasized_spans_alternate_plain_and_bold() {
    let spans = emphasized_spans("one two\nthree", &[4..7]);
    let texts = spans
        .iter()
        .map(|span| span.text.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(texts, vec!["one ", "two", " three"]);
    assert!(spans[0].font.is_none());
    assert!(spans[1].font.is_some());
}
