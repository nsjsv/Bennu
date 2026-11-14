use encoding_rs::{BIG5, GBK, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8};

use super::{reads_as_text, sniff_text_encoding, SniffedEncoding};

fn sniffed(sample: &[u8]) -> Option<SniffedEncoding> {
    sniff_text_encoding(sample, true)
}

#[test]
fn byte_order_marks_decide_the_encoding_and_body_start() {
    assert_eq!(
        sniffed(b"\xef\xbb\xbfabc"),
        Some(SniffedEncoding {
            encoding: UTF_8,
            bom_length: 3
        })
    );
    // UTF-16 正文满是 NUL，BOM 必须先于 NUL 判定。
    let mut utf16le = vec![0xff, 0xfe];
    utf16le.extend("记事本".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(
        sniffed(&utf16le).map(|found| found.encoding),
        Some(UTF_16LE)
    );
    let mut utf16be = vec![0xfe, 0xff];
    utf16be.extend("abc".encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(
        sniffed(&utf16be),
        Some(SniffedEncoding {
            encoding: UTF_16BE,
            bom_length: 2
        })
    );
}

#[test]
fn nul_near_the_start_without_bom_means_binary() {
    assert_eq!(sniffed(b"abc\0def"), None);
    // 8 KB 以后才出现的 NUL 不改变判断。
    let mut late_nul = vec![b'a'; 9 * 1024];
    late_nul.push(0);
    assert_eq!(sniffed(&late_nul).map(|found| found.encoding), Some(UTF_8));
}

#[test]
fn legacy_chinese_and_japanese_encodings_are_recognized() {
    let cases = [
        (
            GBK,
            "这是一个用简体中文写的季度工作报告，内容包括销售数据和市场分析。",
        ),
        (
            BIG5,
            "這是一份用繁體中文撰寫的年度報告，包含財務資料與市場分析。",
        ),
        (
            SHIFT_JIS,
            "これは日本語で書かれた四半期の業務報告書です。売上データと市場分析を含みます。",
        ),
    ];
    for (encoding, text) in cases {
        let (bytes, _, _) = encoding.encode(text);
        assert_eq!(
            sniffed(&bytes).map(|found| found.encoding),
            Some(encoding),
            "{}",
            encoding.name()
        );
    }
}

#[test]
fn utf8_cut_mid_character_is_still_utf8_when_only_the_head_was_read() {
    let mut head = "abc搜".as_bytes().to_vec();
    head.pop();
    assert_eq!(
        sniff_text_encoding(&head, false).map(|found| found.encoding),
        Some(UTF_8)
    );
    // 整篇读完还断在半个字上就不是合法 UTF-8。
    assert_ne!(
        sniff_text_encoding(&head, true).map(|found| found.encoding),
        Some(UTF_8)
    );
}

#[test]
fn more_than_one_percent_non_text_characters_is_binary() {
    let mostly_text = format!("{}\u{fffd}", "a".repeat(99));
    assert!(reads_as_text(&mostly_text));
    let garbled = format!("{}\u{fffd}\u{fffd}", "a".repeat(99));
    assert!(!reads_as_text(&garbled));
    // 日志颜色码与退格是正常内容。
    assert!(reads_as_text("\x1b[31merror\x1b[0m\x08\t\r\n"));
}
