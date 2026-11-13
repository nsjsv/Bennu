//! 端到端：按系统类型库判断类型、按类型取正文，再从索引里搜出来。需要本机装了
//! shared-mime-info（`/usr/share/mime/mime.cache`）。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use tempfile::tempdir;

use crate::config::SearchIndexConfig;
use crate::database::SearchDatabase;
use crate::model::{MimePattern, SearchEntryTypeRule, SearchQuery};
use crate::query_state::QueryState;
use crate::writer::IndexWriter;

use super::SearchIndexer;

fn file_names(database: &SearchDatabase, query: &SearchQuery) -> Vec<String> {
    let mut names = database
        .search(query, &QueryState::default())
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.display_name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[tokio::test]
async fn text_is_recognized_by_type_and_content_and_read_in_its_encoding() {
    if !Path::new("/usr/share/mime/mime.cache").exists() {
        return;
    }
    let content = tempdir().unwrap();
    let db_dir = tempdir().unwrap();
    let root = content.path();
    fs::write(root.join("data.csv"), "city,zzcsvtoken\n").unwrap();
    fs::write(root.join("layout.xml"), "<a><b>zzxmltoken</b></a>").unwrap();
    fs::write(root.join("settings.ini"), "[s]\nkey=zzinitoken\n").unwrap();
    fs::write(
        root.join("movie.srt"),
        "1\n00:00:01,000 --> 00:00:02,000\nzzsrttoken\n",
    )
    .unwrap();
    fs::write(root.join("README"), "zzreadmetoken\n").unwrap();
    fs::write(root.join("Makefile"), "all:\n\techo zzmaketoken\n").unwrap();
    fs::write(root.join("runme"), "#!/bin/sh\necho zzscripttoken\n").unwrap();
    fs::write(root.join("tool.py"), "print('zzpythontoken')\n").unwrap();
    let (gbk, _, _) = encoding_rs::GBK.encode("这份文件是用简体中文写的季度工作报告");
    fs::write(root.join("old-notes.txt"), gbk).unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend("记事本里的字".encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(root.join("notepad.txt"), utf16).unwrap();
    let mut long_log = String::from("zzloghead\n");
    long_log.push_str(&"x".repeat(3000));
    long_log.push_str("\nzzlogtail\n");
    fs::write(root.join("server.log"), long_log).unwrap();
    fs::write(
        root.join("page.html"),
        "<html><body><p>zz<b>html</b>token</p><script>var zzhiddentoken;</script></body></html>",
    )
    .unwrap();
    fs::write(root.join("chart.svg"), "<svg><text>zzsvgtoken</text></svg>").unwrap();
    fs::write(root.join("fake.txt"), b"zzbinarytoken\0\x01\x02\x03").unwrap();
    fs::write(root.join("backup.tar.gz"), b"\x1f\x8b\x08\x00").unwrap();

    let db_path = db_dir.path().join("search.sqlite");
    let writer = Arc::new(IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap()));
    let indexer = SearchIndexer::new(
        Arc::clone(&writer),
        SearchIndexConfig {
            roots: vec![root.to_path_buf()],
            excluded_paths: Vec::new(),
            unavailable_roots: Vec::new(),
            content_indexing_enabled: true,
            max_extract_bytes: 1024,
        },
    );
    indexer.rebuild().await.unwrap();
    let database = SearchDatabase::open_read_only(&db_path).unwrap();
    let found = |terms: &str| file_names(&database, &SearchQuery::global(1, terms));

    for (token, file) in [
        ("zzcsvtoken", "data.csv"),
        ("zzxmltoken", "layout.xml"),
        ("zzinitoken", "settings.ini"),
        ("zzsrttoken", "movie.srt"),
        ("zzreadmetoken", "README"),
        ("zzmaketoken", "Makefile"),
        ("zzscripttoken", "runme"),
        ("zzpythontoken", "tool.py"),
        ("季度工作报告", "old-notes.txt"),
        ("记事本", "notepad.txt"),
        ("zzloghead", "server.log"),
        ("zzhtmltoken", "page.html"),
        ("zzsvgtoken", "chart.svg"),
    ] {
        assert_eq!(found(token), [file], "{token}");
    }
    // 只读了前 1024 字节；脚本里的字、伪装成文字的二进制都不入正文。
    assert!(found("zzlogtail").is_empty());
    assert!(found("zzhiddentoken").is_empty());
    assert!(found("zzbinarytoken").is_empty());

    let mut text = SearchQuery::global(2, "");
    text.filters.entry_type_rules = vec![SearchEntryTypeRule::Mime(MimePattern::Inheriting(
        "text/plain".to_owned(),
    ))];
    let text_files = file_names(&database, &text);
    for expected in [
        "README",
        "Makefile",
        "runme",
        "tool.py",
        "layout.xml",
        "data.csv",
    ] {
        assert!(text_files.contains(&expected.to_owned()), "{expected}");
    }
    assert!(!text_files.contains(&"backup.tar.gz".to_owned()));

    let mut archives = SearchQuery::global(3, "");
    archives.filters.entry_type_rules = vec![SearchEntryTypeRule::Mime(MimePattern::Exact(
        "application/x-compressed-tar".to_owned(),
    ))];
    assert_eq!(file_names(&database, &archives), ["backup.tar.gz"]);
}
