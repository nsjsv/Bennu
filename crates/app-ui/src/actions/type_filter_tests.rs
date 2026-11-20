use std::sync::Arc;

use super::*;

fn types() -> Arc<MimeDatabase> {
    MimeDatabase::system()
}

fn filter(patterns: &[&str]) -> TypeFilter {
    let types = types();
    TypeFilter::Patterns(
        patterns
            .iter()
            .map(|pattern| TypePattern::parse(pattern, &types).expect(pattern))
            .collect(),
    )
}

fn targets(items: &[(&str, bool)]) -> TargetTypes {
    let types = types();
    TargetTypes::new(
        items
            .iter()
            .map(|(path, is_directory)| ItemType::of(Path::new(path), *is_directory, &types)),
        &types,
    )
}

#[test]
fn groups_types_and_subclasses_match_like_kio() {
    assert!(filter(&["video/*"]).matches_all(&targets(&[("/v/a.mp4", false)])));
    assert!(!filter(&["video/*"]).matches_all(&targets(&[("/v/a.png", false)])));
    assert!(filter(&["image/png"]).matches_all(&targets(&[("/p/a.png", false)])));
    // .tar.gz 是 application/gzip 的子类。
    assert!(filter(&["application/gzip"]).matches_all(&targets(&[("/a/b.tar.gz", false)])));
    // 所有 text/* 都算 text/plain。
    assert!(filter(&["text/plain"]).matches_all(&targets(&[("/a/notes.md", false)])));
}

#[test]
fn folders_and_all_files_are_told_apart() {
    let folder = targets(&[("/home/u/docs", true)]);
    let file = targets(&[("/home/u/docs/a.txt", false)]);
    assert!(filter(&["inode/directory"]).matches_all(&folder));
    assert!(!filter(&["inode/directory"]).matches_all(&file));
    assert!(filter(&["application/octet-stream"]).matches_all(&file));
    assert!(!filter(&["application/octet-stream"]).matches_all(&folder));
    assert!(filter(&["allfiles"]).matches_all(&file));
    assert!(filter(&["all/all"]).matches_all(&folder));
    // 认不出名字的文件当作 application/octet-stream。
    let unknown = targets(&[("/a/blob.qqqqzz", false)]);
    assert!(filter(&["application/octet-stream"]).matches_all(&unknown));
    assert!(!filter(&["text/plain"]).matches_all(&unknown));
}

#[test]
fn every_selected_item_has_to_match() {
    let mixed = targets(&[("/v/a.mp4", false), ("/v/b.png", false)]);
    assert!(!filter(&["video/*"]).matches_all(&mixed));
    assert!(filter(&["video/*", "image/*"]).matches_all(&mixed));
    assert!(TypeFilter::Anything.matches_all(&mixed));
}

#[test]
fn malformed_patterns_are_rejected() {
    let types = types();
    for wrong in ["video", "video/", "/png", "a/b/c", "vid eo/*", ""] {
        assert!(TypePattern::parse(wrong, &types).is_none(), "{wrong}");
    }
}

/// 打开菜单时 50 个动作、选中 1000 项也要很快（PRD R10：额外耗时不到 1 毫秒）。
#[test]
fn matching_fifty_actions_against_a_thousand_items_is_fast() {
    let types = types();
    let filters = (0..50)
        .map(|index| match index % 3 {
            0 => filter(&["video/*"]),
            1 => filter(&["image/png", "image/jpeg"]),
            _ => filter(&["application/octet-stream"]),
        })
        .collect::<Vec<_>>();
    let names = (0..1000)
        .map(|index| format!("/photos/IMG_{index:04}.png"))
        .collect::<Vec<_>>();
    // 先暖一次缓存：系统库第一次要从磁盘读。
    TargetTypes::new([ItemType::of(Path::new("/a.png"), false, &types)], &types);

    let started = std::time::Instant::now();
    let selection = TargetTypes::new(
        names
            .iter()
            .map(|name| ItemType::of(Path::new(name), false, &types)),
        &types,
    );
    let matching = filters
        .iter()
        .filter(|filter| filter.matches_all(&selection))
        .count();
    let elapsed = started.elapsed();

    assert_eq!(matching, 33);
    // 调试构建比发布构建慢得多，这里给 20 毫秒；发布构建远低于 1 毫秒。
    assert!(
        elapsed < std::time::Duration::from_millis(20),
        "{elapsed:?}"
    );
}
