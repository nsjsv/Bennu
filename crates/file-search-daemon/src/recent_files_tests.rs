use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use tempfile::tempdir;

use super::*;

fn xbel(bookmarks: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
{bookmarks}
</xbel>"#
    )
}

#[test]
fn bookmarks_map_local_paths_to_their_latest_use() {
    let parsed = parse_recently_used(&xbel(
        r#"<bookmark href="file:///home/me/%E6%8A%A5%E5%91%8A%20final.docx" added="2026-11-01T08:00:00Z" modified="2026-11-02T08:00:00.250Z" visited="2026-11-03T09:30:00.5Z"><info/></bookmark>
<bookmark href="file://localhost/home/me/notes.txt" added="2026-10-01T00:00:00Z" modified="2026-10-01T00:00:00Z" visited="1969-12-31T23:59:59Z"/>
<bookmark href="https://example.com/page" added="2026-11-01T00:00:00Z" modified="2026-11-01T00:00:00Z" visited="2026-11-01T00:00:00Z"/>"#,
    ));

    assert_eq!(parsed.len(), 2);
    assert_eq!(
        parsed[Path::new("/home/me/报告 final.docx")],
        utc_timestamp_ms("2026-11-03T09:30:00.5Z").unwrap()
    );
    // 1969 年的占位访问时间不算，取真实的修改时间。
    assert_eq!(
        parsed[Path::new("/home/me/notes.txt")],
        utc_timestamp_ms("2026-10-01T00:00:00Z").unwrap()
    );
}

#[test]
fn percent_encoded_bytes_keep_non_utf8_paths() {
    let path = file_uri_path("file:///tmp/%FF%FE.bin").unwrap();
    assert_eq!(path.as_os_str().as_bytes(), b"/tmp/\xff\xfe.bin");
    assert!(file_uri_path("file:///tmp/%G1").is_none());
    assert!(file_uri_path("file:relative").is_none());
}

#[test]
fn timestamps_follow_the_utc_calendar() {
    assert_eq!(utc_timestamp_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
    assert_eq!(
        utc_timestamp_ms("2000-03-01T00:00:00Z"),
        Some(951_868_800_000)
    );
    assert_eq!(
        utc_timestamp_ms("2026-11-12T10:20:30.123456Z"),
        Some(1_794_478_830_123)
    );
    assert_eq!(utc_timestamp_ms("2026-11-12T10:20:30+08:00"), None);
    assert_eq!(utc_timestamp_ms("2026-13-01T00:00:00Z"), None);
    assert_eq!(utc_timestamp_ms("2026-11-12T10:20:30.１Z"), None);
}

#[test]
fn a_broken_file_keeps_what_was_read_before_the_error() {
    let parsed = parse_recently_used(
        r#"<xbel><bookmark href="file:///a.txt" modified="2026-11-01T00:00:00Z"/><bookmark href="file:///b.txt" modified=</xbel>"#,
    );
    assert!(parsed.contains_key(Path::new("/a.txt")));
    assert!(!parsed.contains_key(Path::new("/b.txt")));
}

#[test]
fn entries_beyond_the_cap_are_not_loaded() {
    let bookmarks = (0..MAX_RECENT_ENTRIES + 5)
        .map(|index| {
            format!(r#"<bookmark href="file:///f{index}" modified="2026-11-01T00:00:00Z"/>"#)
        })
        .collect::<String>();
    assert_eq!(
        parse_recently_used(&xbel(&bookmarks)).len(),
        MAX_RECENT_ENTRIES
    );
}

#[test]
fn the_list_is_reloaded_only_when_the_file_changes() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("recently-used.xbel");
    let recent_files = RecentFiles::at(source.clone());
    assert!(recent_files.last_used().is_empty());

    std::fs::write(
        &source,
        xbel(r#"<bookmark href="file:///a.txt" modified="2026-11-01T00:00:00Z"/>"#),
    )
    .unwrap();
    let first = recent_files.last_used();
    assert!(first.contains_key(Path::new("/a.txt")));
    assert!(Arc::ptr_eq(&first, &recent_files.last_used()));

    std::fs::write(
        &source,
        xbel(r#"<bookmark href="file:///bb.txt" modified="2026-11-01T00:00:00Z"/>"#),
    )
    .unwrap();
    let reloaded = recent_files.last_used();
    assert!(reloaded.contains_key(Path::new("/bb.txt")));
    assert!(!reloaded.contains_key(Path::new("/a.txt")));
}

#[test]
fn a_missing_source_or_an_oversized_file_means_no_recent_files() {
    assert!(RecentFiles::default().last_used().is_empty());
    let directory = tempdir().unwrap();
    let source = directory.path().join("recently-used.xbel");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(MAX_RECENT_FILE_BYTES + 1)
        .unwrap();
    assert!(RecentFiles::at(source).last_used().is_empty());
}
