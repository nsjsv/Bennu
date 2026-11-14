use super::*;

fn spans_text(spans: &[Span<'_, (), Font>]) -> Vec<(String, bool)> {
    spans
        .iter()
        .map(|span| (span.text.to_string(), span.highlight.is_some()))
        .collect()
}

#[test]
fn search_marks_split_styled_spans_and_get_a_background() {
    let palette = HighlightPalette::Light;
    let looks = [
        (
            0..6,
            SpanLook {
                font: Font::DEFAULT,
                color: None,
                background: None,
                strike: false,
            },
        ),
        (
            6..11,
            SpanLook {
                font: Font::MONOSPACE,
                color: None,
                background: None,
                strike: false,
            },
        ),
    ];
    let spans = marked_spans("hello world", looks.into_iter(), &[(4..8, true)], palette);
    assert_eq!(
        spans_text(&spans),
        vec![
            ("hell".to_owned(), false),
            ("o ".to_owned(), true),
            ("wo".to_owned(), true),
            ("rld".to_owned(), false),
        ]
    );
}

#[test]
fn each_block_has_its_own_container_id() {
    assert_ne!(markdown_block_id(1), markdown_block_id(2));
    assert_eq!(markdown_block_id(3), markdown_block_id(3));
}

#[test]
fn chinese_inside_code_uses_the_interface_font() {
    let code = SpanLook {
        font: Font::MONOSPACE,
        color: None,
        background: None,
        strike: false,
    };
    let text = "// 注释 ok";
    let spans = marked_spans(
        text,
        [(0..text.len(), code)].into_iter(),
        &[],
        HighlightPalette::Dark,
    );
    let fonts: Vec<(String, Option<Font>)> = spans
        .iter()
        .map(|span| (span.text.to_string(), span.font))
        .collect();
    assert_eq!(
        fonts,
        vec![
            ("// ".to_owned(), Some(Font::MONOSPACE)),
            ("注释".to_owned(), Some(Font::DEFAULT)),
            (" ok".to_owned(), Some(Font::MONOSPACE)),
        ]
    );
}
