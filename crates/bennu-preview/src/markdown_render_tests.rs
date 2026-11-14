use super::*;

fn text_blocks(rendered: &RenderedMarkdown) -> Vec<(TextBlockKind, Option<&str>, &str)> {
    rendered
        .blocks()
        .iter()
        .filter_map(|block| match block {
            MarkdownBlock::Text {
                kind,
                marker,
                content,
            } => Some((*kind, marker.as_deref(), content.inline.text.as_str())),
            _ => None,
        })
        .collect()
}

#[test]
fn headings_lists_and_quotes_keep_their_kind_and_markers() {
    let rendered =
        RenderedMarkdown::parse("# Title\n\n- [x] done\n- next\n\n3. third\n\n> quoted\n");
    assert_eq!(
        text_blocks(&rendered),
        vec![
            (TextBlockKind::Heading(HeadingLevel::H1), None, "Title"),
            (TextBlockKind::ListItem { depth: 0 }, Some("☑"), "done"),
            (TextBlockKind::ListItem { depth: 0 }, Some("•"), "next"),
            (TextBlockKind::ListItem { depth: 0 }, Some("3."), "third"),
            (TextBlockKind::Quote { depth: 1 }, None, "quoted"),
        ]
    );
}

#[test]
fn inline_formatting_is_kept_without_markup_characters() {
    let rendered =
        RenderedMarkdown::parse("Plain **bold *both*** `code` ~~gone~~ [link](https://x.y)\n");
    let MarkdownBlock::Text { content, .. } = &rendered.blocks()[0] else {
        panic!("a paragraph");
    };
    assert_eq!(content.inline.text, "Plain bold both code gone link");
    let styles: Vec<(&str, InlineStyle)> = content
        .inline
        .styled_ranges()
        .map(|(range, style)| (&content.inline.text[range], style))
        .collect();
    let style_of = |needle: &str| {
        styles
            .iter()
            .find(|(text, _)| text.contains(needle))
            .map(|(_, style)| *style)
            .expect("styled")
    };
    assert!(style_of("bold").bold && !style_of("bold").italic);
    assert!(style_of("both").bold && style_of("both").italic);
    assert!(style_of("code").code);
    assert!(style_of("gone").strike);
    assert!(style_of("link").link);
    assert_eq!(style_of("Plain"), InlineStyle::default());
}

#[test]
fn tables_keep_alignment_header_and_cells_as_units() {
    let rendered =
        RenderedMarkdown::parse("| Name | **Size** |\n| :- | -: |\n| a | 1 |\n| b `c` | 22 |\n");
    let MarkdownBlock::Table {
        alignments,
        header,
        rows,
    } = &rendered.blocks()[0]
    else {
        panic!("a table");
    };
    assert_eq!(alignments, &vec![Alignment::Left, Alignment::Right]);
    let texts = |cells: &[MarkdownUnit]| -> Vec<String> {
        cells.iter().map(|cell| cell.inline.text.clone()).collect()
    };
    assert_eq!(texts(header), ["Name", "Size"]);
    assert!(header[1].inline.runs.iter().all(|(_, style)| style.bold));
    assert_eq!(texts(&rows[0]), ["a", "1"]);
    assert_eq!(texts(&rows[1]), ["b c", "22"]);
    // 每个单元格是一个能搜的单元，都在这张表这一块里。
    let units: Vec<usize> = rendered.units().map(|unit| unit.unit).collect();
    assert_eq!(units, (0..6).collect::<Vec<_>>());
    assert!((0..6).all(|unit| rendered.block_of_unit(unit) == Some(0)));
}

#[test]
fn fenced_code_is_highlighted_only_when_the_language_is_known() {
    let rendered = RenderedMarkdown::parse(
        "```rust\nfn main() {\n    // hi\n}\n```\n\n```\nplain text\n```\n\n```nonsense-lang\nx\n```\n",
    );
    let code: Vec<(&Option<String>, &Vec<CodeLine>, &str)> = rendered
        .blocks()
        .iter()
        .filter_map(|block| match block {
            MarkdownBlock::Code {
                language,
                content,
                lines,
            } => Some((language, lines, content.inline.text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(code.len(), 3);
    let (language, lines, text) = code[0];
    assert_eq!(language.as_deref(), Some("rust"));
    assert_eq!(lines.len(), 3);
    assert_eq!(&text[lines[1].range.clone()], "    // hi");
    assert!(lines[0]
        .tokens
        .iter()
        .any(|token| token.kind == crate::text_highlight::TokenKind::Keyword));
    assert!(lines[1]
        .tokens
        .iter()
        .any(|token| token.kind == crate::text_highlight::TokenKind::Comment));
    assert!(code[1].1.iter().all(|line| line.tokens.is_empty()));
    assert!(code[2].1.iter().all(|line| line.tokens.is_empty()));
}

#[test]
fn units_follow_document_order() {
    let rendered = RenderedMarkdown::parse("one\n\n---\n\n```\ntwo\n```\n\n- three\n");
    let texts: Vec<&str> = rendered
        .units()
        .map(|unit| unit.inline.text.as_str())
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);
    assert_eq!(rendered.block_of_unit(1), Some(2));
}

/// 手动跑的测量：`... measure_markdown_parse -- --ignored --nocapture`。约 2 MB 的
/// Markdown（正文、列表、表格、Rust 代码块混排）解析加代码块高亮的耗时。
#[test]
#[ignore]
fn measure_markdown_parse() {
    let unit = "## Section\n\nSome **bold** text with `code` and a [link](https://example.com).\n\n- item one\n- item two\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n```rust\nfn main() {\n    let x = 42; // answer\n}\n```\n\n";
    let source = unit.repeat(2 * 1024 * 1024 / unit.len());
    let started = std::time::Instant::now();
    let rendered = RenderedMarkdown::parse(&source);
    println!(
        "{} bytes, {} blocks: {:?}",
        source.len(),
        rendered.blocks().len(),
        started.elapsed()
    );
}
