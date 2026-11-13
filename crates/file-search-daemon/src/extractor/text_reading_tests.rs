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

/// 游戏资源的样子：开头一整片 0x9D（判二进制只看的那 8 KB 里没有 NUL），后面是夹着 NUL
/// 和控制字节的数据。
fn game_asset() -> Vec<u8> {
    let mut bytes = vec![0x9d; 9 * 1024];
    bytes.extend(b"\x9d\x00\x01\x9d\x10\x9d\x9d\x1f\xff\x9d".repeat(2 * 1024));
    bytes
}

#[test]
fn binary_without_nul_bytes_is_not_text() {
    assert_eq!(decode_text(&game_asset(), false), None);
    // 数据文件：没有 NUL，但夹满控制字节。
    let record = b"id=\x01\x02name\x1f\x03\x7f\x80\x81value\x14\x15";
    assert_eq!(decode_text(&record.repeat(1000), false), None);
}

#[test]
fn text_with_a_few_stray_control_characters_is_still_text() {
    let bundled_script = format!("{}\x01", "const 名称 = \"值\";\n".repeat(20));
    assert!(decode_text(bundled_script.as_bytes(), false).is_some());
    let log = format!(
        "{}\x1b[0m\x08",
        "游戏日志：加载场景完成，耗时 12 毫秒。\n".repeat(30)
    );
    let (gbk_log, _, _) = encoding_rs::GBK.encode(&log);
    assert_eq!(decode_text(&gbk_log, false), Some(log));
}

#[test]
fn more_than_one_percent_of_non_text_characters_means_binary() {
    let at_limit = format!("{}\x01", "a".repeat(99));
    assert!(decode_text(at_limit.as_bytes(), false).is_some());
    let over_limit = format!("{}\x01\x01", "a".repeat(98));
    assert_eq!(decode_text(over_limit.as_bytes(), false), None);
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
async fn text_that_grows_when_decoded_stays_within_the_limit() {
    let directory = tempdir().unwrap();
    let chinese = "这是一份用简体中文写的季度工作报告，内容包括销售数据和市场分析。".repeat(40);
    let french = "Le café crème coûte très cher à Noël, déjà réservé. ".repeat(40);
    let cases = [
        (
            "report.txt",
            encoding_rs::GBK.encode(&chinese).0.into_owned(),
            TextForm::Plain,
            &chinese,
        ),
        (
            "menu.txt",
            encoding_rs::WINDOWS_1252.encode(&french).0.into_owned(),
            TextForm::Plain,
            &french,
        ),
        (
            "page.html",
            encoding_rs::GBK
                .encode(&format!("<p>{chinese}</p>"))
                .0
                .into_owned(),
            TextForm::Markup,
            &chinese,
        ),
    ];
    let limit = 1000;
    for (name, encoded, form, original) in cases {
        let path = directory.path().join(name);
        fs::write(&path, encoded).unwrap();

        let outcome = extract_text(&path, limit, form).await.unwrap();

        let text = outcome.text.unwrap();
        assert!(text.len() as u64 <= limit, "{name}: {} bytes", text.len());
        assert!(
            text.len() as u64 > limit - 4,
            "{name}: {} bytes",
            text.len()
        );
        assert!(original.starts_with(&text), "{name}: {text}");
    }
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
async fn binary_that_decodes_into_characters_is_not_indexed() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("sharedassets.resS");
    fs::write(&path, game_asset()).unwrap();

    let outcome = extract_text(&path, 64 * 1024, TextForm::Plain)
        .await
        .unwrap();

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
