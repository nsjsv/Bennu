use std::fs;

use tempfile::tempdir;

use super::{decode_text, extract_text, TextForm};
use crate::extractor::ExtractionStatus;

#[test]
fn utf8_is_used_as_is() {
    assert_eq!(
        decode_text("hello 搜索".as_bytes(), false).as_deref(),
        Some("hello 搜索")
    );
}

#[test]
fn a_character_cut_at_the_read_limit_is_dropped() {
    let mut bytes = "abc搜".as_bytes().to_vec();
    bytes.pop();
    assert_eq!(decode_text(&bytes, true).as_deref(), Some("abc"));
}

#[test]
fn gbk_and_big5_are_recognized() {
    let (gbk, _, _) =
        encoding_rs::GBK.encode("这是一个用简体中文写的季度工作报告，内容包括销售数据和市场分析。");
    assert_eq!(
        decode_text(&gbk, false).as_deref(),
        Some("这是一个用简体中文写的季度工作报告，内容包括销售数据和市场分析。")
    );
    let (big5, _, _) =
        encoding_rs::BIG5.encode("這是一份用繁體中文撰寫的年度報告，包含財務資料與市場分析。");
    assert_eq!(
        decode_text(&big5, false).as_deref(),
        Some("這是一份用繁體中文撰寫的年度報告，包含財務資料與市場分析。")
    );
}

#[test]
fn byte_order_marks_decide_the_encoding() {
    let mut utf16le = vec![0xff, 0xfe];
    utf16le.extend("记事本".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(decode_text(&utf16le, false).as_deref(), Some("记事本"));

    let mut utf16be = vec![0xfe, 0xff];
    utf16be.extend("记事本".encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(decode_text(&utf16be, false).as_deref(), Some("记事本"));

    // 只读了开头时，UTF-16 被截断的半个码元丢掉。
    let mut cut = utf16le.clone();
    cut.push(0x41);
    assert_eq!(decode_text(&cut, true).as_deref(), Some("记事本"));

    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend_from_slice("带 BOM".as_bytes());
    assert_eq!(decode_text(&utf8_bom, false).as_deref(), Some("带 BOM"));
}

#[test]
fn nul_bytes_without_a_byte_order_mark_mean_binary() {
    assert_eq!(decode_text(b"text\0\x01\x02 more", false), None);
    let mut late_nul = vec![b'a'; 9 * 1024];
    late_nul.push(0);
    assert_eq!(
        decode_text(&late_nul, false).map(|text| text.ends_with(' ')),
        Some(true)
    );
}

#[tokio::test]
async fn long_text_is_read_up_to_the_limit() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("long.log");
    fs::write(&path, "first line\n".repeat(100)).unwrap();

    let outcome = extract_text(&path, 20, TextForm::Plain).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::Indexed);
    assert_eq!(outcome.text.as_deref(), Some("first line\nfirst lin"));
}

#[tokio::test]
async fn markup_is_reduced_to_its_words() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("page.html");
    fs::write(&path, "<h1>标题</h1><p>正文</p>").unwrap();

    let outcome = extract_text(&path, 1024, TextForm::Markup).await.unwrap();

    assert_eq!(outcome.text.as_deref(), Some("标题 正文"));
}

#[tokio::test]
async fn binary_content_with_a_text_name_is_not_indexed() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("really-binary.txt");
    fs::write(&path, [0u8; 4096]).unwrap();

    let outcome = extract_text(&path, 1024, TextForm::Plain).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::Unsupported);
    assert!(outcome.text.is_none());
}

#[tokio::test]
async fn a_missing_file_is_skipped_instead_of_aborting_the_crawl() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("gone.txt");

    let outcome = extract_text(&path, 1024, TextForm::Plain).await.unwrap();

    assert!(matches!(
        outcome.status,
        ExtractionStatus::ReadFailed { .. }
    ));
}
