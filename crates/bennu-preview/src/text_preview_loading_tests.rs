use tempfile::tempdir;

use super::*;
use crate::text_document::MARKDOWN_RENDER_LIMIT_BYTES;

/// 窗口里的行按原始行拼回文字。
fn joined(opened: &TextOpened) -> String {
    let mut text = String::new();
    for (index, row) in opened.batch.rows.iter().enumerate() {
        if index > 0 && row.is_line_start() {
            text.push('\n');
        }
        text.push_str(&row.text);
    }
    text
}

async fn opened(path: PathBuf, rules: &PreviewExtensionRules) -> Result<Arc<TextOpened>, String> {
    match load_initial_text_preview(path, rules).await? {
        PreviewContent::Text { opened, .. } => Ok(opened),
        _ => panic!("expected text preview"),
    }
}

fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

#[tokio::test]
async fn legacy_encodings_and_utf16_are_decoded() {
    let temp_dir = tempdir().expect("temp dir");
    let rules = PreviewExtensionRules::default_rules();
    let cases = [
        (
            encoding_rs::GBK,
            "这是一个用简体中文写的季度工作报告，内容包括销售数据和市场分析。",
        ),
        (
            encoding_rs::BIG5,
            "這是一份用繁體中文撰寫的年度報告，包含財務資料與市場分析。",
        ),
        (
            encoding_rs::SHIFT_JIS,
            "これは日本語で書かれた四半期の業務報告書です。売上データと市場分析を含みます。",
        ),
    ];
    for (encoding, text) in cases {
        let path = temp_dir.path().join(format!("{}.txt", encoding.name()));
        std::fs::write(&path, encoding.encode(text).0).expect("write text file");
        let opened = opened(path, &rules).await.expect("text preview");
        assert_eq!(opened.decoding.encoding(), encoding);
        assert_eq!(joined(&opened), text);
    }

    // 带 BOM 的 UTF-16，有后缀和没后缀（类型库认不出）都当文本。
    for name in ["notepad.txt", "notepad"] {
        let path = temp_dir.path().join(name);
        std::fs::write(&path, utf16le_with_bom("记事本\r\n第二行")).expect("write text file");
        let opened = opened(path, &rules).await.expect(name);
        assert_eq!(opened.decoding.encoding(), encoding_rs::UTF_16LE);
        assert_eq!(joined(&opened), "记事本\n第二行");
    }
}

#[tokio::test]
async fn binary_content_is_rejected_whatever_the_name() {
    let temp_dir = tempdir().expect("temp dir");
    let rules = PreviewExtensionRules::default_rules();
    let mut binary = b"\x7fELF\x02\x01\x01".to_vec();
    binary.extend((0..4096_u32).map(|value| (value % 251) as u8));
    for name in ["looks-like.txt", "no-extension"] {
        let path = temp_dir.path().join(name);
        std::fs::write(&path, &binary).expect("write binary file");
        assert_eq!(opened(path, &rules).await.err(), Some(not_text()), "{name}");
    }
}

#[tokio::test]
async fn typescript_is_text_but_mpeg_ts_is_not() {
    let temp_dir = tempdir().expect("temp dir");
    let rules = PreviewExtensionRules::default_rules();
    let source = temp_dir.path().join("index.ts");
    std::fs::write(&source, "const answer: number = 42;\nexport { answer };\n")
        .expect("write typescript");
    assert!(opened(source, &rules).await.is_ok());

    let mut transport_stream = Vec::new();
    for _ in 0..5 {
        let mut packet = vec![0xff; 188];
        packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        transport_stream.extend(packet);
    }
    let video = temp_dir.path().join("clip.ts");
    std::fs::write(&video, transport_stream).expect("write transport stream");
    assert_eq!(opened(video, &rules).await.err(), Some(not_text()));
}

#[tokio::test]
async fn markdown_is_decided_by_type_and_rendered_only_up_to_two_megabytes() {
    let temp_dir = tempdir().expect("temp dir");
    let rules = PreviewExtensionRules::default_rules();
    let markdown = temp_dir.path().join("README.md");
    std::fs::write(&markdown, "# Title\n").expect("write markdown");
    let plain = temp_dir.path().join("README");
    std::fs::write(&plain, "# Title\n").expect("write readme");

    let markdown = opened(markdown, &rules).await.expect("md");
    assert_eq!(markdown.format, TextPreviewFormat::Markdown);
    assert_eq!(markdown.markdown.as_deref(), Some("# Title\n"));
    let plain = opened(plain, &rules).await.expect("readme");
    assert_eq!(plain.format, TextPreviewFormat::Plain);
    assert!(plain.markdown.is_none());

    let large = temp_dir.path().join("large.md");
    let line = "- item with some words\n";
    let repeat = (MARKDOWN_RENDER_LIMIT_BYTES as usize / line.len()) + 10;
    std::fs::write(&large, line.repeat(repeat)).expect("write large markdown");
    let large = opened(large, &rules).await.expect("large md");
    assert_eq!(large.format, TextPreviewFormat::Markdown);
    assert!(large.markdown.is_none());
}

#[tokio::test]
async fn extra_text_extension_overrides_a_non_text_name() {
    let temp_dir = tempdir().expect("temp dir");
    let path = temp_dir.path().join("notes.so");
    std::fs::write(&path, "plain words\n").expect("write text file");

    let rules = PreviewExtensionRules::default_rules();
    assert_eq!(opened(path.clone(), &rules).await.err(), Some(not_text()));
    let mut rules = PreviewExtensionRules::default_rules();
    rules.text.push("so".to_owned());
    assert!(opened(path, &rules).await.is_ok());
}

#[tokio::test]
async fn a_final_newline_does_not_add_an_empty_row() {
    let temp_dir = tempdir().expect("temp dir");
    let path = temp_dir.path().join("Dockerfile");
    std::fs::write(&path, "FROM alpine\nRUN true\nCMD [\"sh\"]\n").expect("write file");
    let opened = opened(path, &PreviewExtensionRules::default_rules())
        .await
        .expect("text preview");
    assert_eq!(opened.batch.rows.len(), 3);
    assert!(opened.batch.at_start && opened.batch.at_end);
    assert_eq!(opened.batch.rows[2].line_number, Some(2));
}

#[tokio::test]
async fn reopening_decodes_the_same_bytes_with_the_chosen_encoding() {
    let temp_dir = tempdir().expect("temp dir");
    let path = temp_dir.path().join("report.txt");
    let text = "这是一个用简体中文写的季度工作报告。";
    std::fs::write(&path, encoding_rs::GBK.encode(text).0).expect("write text file");

    let as_utf8 = reopen_text_preview(
        path.clone(),
        TextEncodingChoice::Chosen(encoding_rs::UTF_8),
        TextPreviewFormat::Plain,
        TextJump::Start,
    )
    .await
    .expect("utf-8");
    assert!(joined(&as_utf8).contains('\u{fffd}'));
    assert!(as_utf8.encoding_chosen);

    let as_gbk = reopen_text_preview(
        path,
        TextEncodingChoice::Chosen(encoding_rs::GBK),
        TextPreviewFormat::Plain,
        TextJump::Start,
    )
    .await
    .expect("gbk");
    assert_eq!(as_gbk.decoding.encoding(), encoding_rs::GBK);
    assert_eq!(joined(&as_gbk), text);
}

#[tokio::test]
async fn reopening_skips_only_a_matching_byte_order_mark() {
    let temp_dir = tempdir().expect("temp dir");
    let path = temp_dir.path().join("bom.txt");
    std::fs::write(&path, b"\xef\xbb\xbfabc").expect("write text file");

    let reopen = |encoding| {
        reopen_text_preview(
            path.clone(),
            TextEncodingChoice::Chosen(encoding),
            TextPreviewFormat::Plain,
            TextJump::Start,
        )
    };
    assert_eq!(
        joined(&reopen(encoding_rs::UTF_8).await.expect("utf-8")),
        "abc"
    );
    // 换成别的编码时 BOM 也是要看的字节。
    assert_eq!(
        joined(
            &reopen(encoding_rs::WINDOWS_1252)
                .await
                .expect("windows-1252")
        ),
        "ï»¿abc"
    );
}

#[tokio::test]
async fn reopening_near_an_offset_keeps_that_row_in_focus() {
    let temp_dir = tempdir().expect("temp dir");
    let path = temp_dir.path().join("log.txt");
    let content: String = (0..50_000).map(|index| format!("line {index}\n")).collect();
    std::fs::write(&path, &content).expect("write log");
    let offset = content.find("line 30000\n").expect("line") as u64 + 3;

    let opened = reopen_text_preview(
        path,
        TextEncodingChoice::Detect,
        TextPreviewFormat::Plain,
        TextJump::Offset(offset),
    )
    .await
    .expect("reopen");
    let (focus, place) = opened.batch.focus.expect("focus");
    assert_eq!(place, crate::text_document::JumpPlace::Top);
    let row = opened
        .batch
        .rows
        .iter()
        .find(|row| row.start == focus)
        .expect("focus row");
    assert_eq!(row.text, "line 30000");
    assert!(!opened.batch.at_start && !opened.batch.at_end);
}
