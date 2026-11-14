use crate::text_document::test_support::settle;
use crate::text_document::*;

fn lines_with_marks(count: usize, marked: &[usize]) -> Vec<u8> {
    (0..count)
        .map(|index| {
            let mark = if marked.contains(&index) { " mark" } else { "" };
            format!("line {index}{mark}\n")
        })
        .collect::<String>()
        .into_bytes()
}

fn line_start_of(bytes: &[u8], line: usize) -> u64 {
    if line == 0 {
        return 0;
    }
    bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(line - 1)
        .map(|(index, _)| index as u64 + 1)
        .expect("line exists")
}

fn search(document: &mut TextPreviewDocument, text: &str) {
    let reads = document.open_search(None);
    settle(document, reads);
    let reads = document.set_search_text(text.to_owned());
    settle(document, reads);
}

fn step(document: &mut TextPreviewDocument, direction: SearchDirection) -> TextMatch {
    let reads = document.step_search(direction);
    settle(document, reads);
    document.current_match().expect("current match")
}

fn current_line(document: &TextPreviewDocument, bytes: &[u8]) -> usize {
    let current = document.current_match().expect("current match");
    bytes[..current.line_start as usize]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

#[test]
fn typing_finds_the_first_match_and_counts_them_all() {
    let bytes = lines_with_marks(100, &[10, 50, 90]);
    let mut document = document_from_bytes(bytes.clone(), TextJump::Start);
    search(&mut document, "MARK");
    assert_eq!(document.search_matches().len(), 3);
    assert_eq!(current_line(&document, &bytes), 10);
    let jump = document.jump().expect("jump");
    assert_eq!(jump.place, JumpPlace::Reveal);
    assert_eq!(
        jump.cursor,
        document.current_match().map(|found| found.start)
    );
    assert_eq!(
        document.search_summary(),
        Some(SearchSummary {
            ordinal: Some(1),
            total: 3,
            finished: true,
        })
    );
}

#[test]
fn stepping_wraps_around_when_the_whole_file_is_in_the_window() {
    let bytes = lines_with_marks(100, &[10, 50, 90]);
    let mut document = document_from_bytes(bytes.clone(), TextJump::Start);
    search(&mut document, "mark");
    step(&mut document, SearchDirection::Forward);
    assert_eq!(current_line(&document, &bytes), 50);
    step(&mut document, SearchDirection::Forward);
    step(&mut document, SearchDirection::Forward);
    assert_eq!(current_line(&document, &bytes), 10);
    step(&mut document, SearchDirection::Backward);
    assert_eq!(current_line(&document, &bytes), 90);
    assert_eq!(
        document
            .search_summary()
            .and_then(|summary| summary.ordinal),
        Some(3)
    );
}

#[test]
fn a_match_outside_the_window_is_found_in_the_background_and_jumped_to() {
    let bytes = lines_with_marks(200_000, &[150_000]);
    let mut document = document_from_bytes(bytes.clone(), TextJump::Start);
    search(&mut document, "mark");
    let current = document.current_match().expect("found");
    assert_eq!(current.line_start, line_start_of(&bytes, 150_000));
    assert!(
        document.row_index(current.start.row).is_ok(),
        "window moved there"
    );
    assert_eq!(
        document.jump().and_then(|jump| jump.cursor),
        Some(current.start)
    );
    // 往前找绕回到同一处（整篇只有一个）。
    let again = step(&mut document, SearchDirection::Backward);
    assert_eq!(again, current);
    assert_eq!(
        document
            .search_summary()
            .and_then(|summary| summary.ordinal),
        Some(1)
    );
}

#[test]
fn window_matches_follow_rows_read_in_both_directions() {
    let bytes = lines_with_marks(300_000, &[]);
    let middle = bytes.len() as u64 / 2;
    let mut document = document_from_bytes(bytes, TextJump::Offset(middle));
    search(&mut document, "line");
    // 视口贴着窗口上沿（往前补读），再贴着下沿（往后补读）。
    for top_edge in [true, false] {
        let first = if top_edge {
            0
        } else {
            document.rows().len() - 31
        };
        let viewport = TextViewport {
            first_row: document.rows()[first].start,
            last_row: document.rows()[first + 30].start,
            at_bottom: false,
            cursor: None,
        };
        let reads = document.accept_viewport(viewport);
        settle(&mut document, reads);
        // 每行一个「line」：窗口里的匹配与窗口行一一对应。
        let rows: Vec<u64> = document.rows().iter().map(|row| row.start).collect();
        let matched: Vec<u64> = document
            .search_matches()
            .iter()
            .map(|found| found.start.row)
            .collect();
        assert_eq!(matched, rows);
    }
}

#[test]
fn closing_hides_matches_and_reopening_keeps_the_query() {
    let bytes = lines_with_marks(100, &[10]);
    let mut document = document_from_bytes(bytes, TextJump::Start);
    search(&mut document, "mark");
    document.close_find();
    assert!(document.find_bar().is_none());
    assert!(document.search_matches().is_empty());
    assert_eq!(document.search_summary(), None);
    let reads = document.open_search(None);
    settle(&mut document, reads);
    assert_eq!(document.search_query().text, "mark");
    assert_eq!(document.search_matches().len(), 1);
}

#[test]
fn match_case_restarts_the_search() {
    let bytes = b"Mark mark MARK\n".to_vec();
    let mut document = document_from_bytes(bytes, TextJump::Start);
    search(&mut document, "mark");
    assert_eq!(document.search_matches().len(), 3);
    let reads = document.toggle_search_case();
    settle(&mut document, reads);
    assert_eq!(document.search_matches().len(), 1);
    assert_eq!(
        document.current_match().map(|found| found.start.column),
        Some(5)
    );
}

#[test]
fn a_selected_word_seeds_the_query() {
    let mut document = document_from_bytes(lines_with_marks(10, &[3]), TextJump::Start);
    let reads = document.open_search(Some("line 3".to_owned()));
    settle(&mut document, reads);
    assert_eq!(document.search_query().text, "line 3");
    assert_eq!(document.search_matches().len(), 1);
}

#[test]
fn going_to_a_line_clamps_and_works_while_lines_are_still_counted() {
    let bytes = lines_with_marks(50_000, &[]);
    let mut document = document_from_bytes(bytes.clone(), TextJump::Start);
    assert!(matches!(
        document.line_count_status(),
        LineCountStatus::Counting { .. }
    ));
    for (input, line) in [("12", 11), ("0", 0), ("9x9999999", 49_999)] {
        document.open_goto();
        document.set_goto_input(input.to_owned());
        let reads = document.submit_goto();
        settle(&mut document, reads);
        assert!(
            document.find_bar().is_none(),
            "the bar closes after the jump"
        );
        let jump = document.jump().expect("jump");
        assert_eq!(jump.place, JumpPlace::Reveal);
        let target = line_start_of(&bytes, line);
        assert_eq!(
            jump.cursor,
            Some(TextPosition {
                row: target,
                column: 0
            })
        );
        assert!(document.row_index(target).is_ok());
    }
}

#[test]
fn goto_input_keeps_only_digits() {
    let mut document = document_from_bytes(lines_with_marks(10, &[]), TextJump::Start);
    document.open_goto();
    document.set_goto_input("1a2 b3".to_owned());
    assert_eq!(
        document.find_bar(),
        Some(&FindBar::GotoLine {
            input: "123".to_owned()
        })
    );
}

fn markdown_document(markdown: &str) -> TextPreviewDocument {
    let mut document = document_from_bytes(markdown.as_bytes().to_vec(), TextJump::Start);
    document.markdown = Some(std::sync::Arc::new(
        crate::markdown_render::RenderedMarkdown::parse(markdown),
    ));
    document.format = crate::text_preview::TextPreviewFormat::Markdown;
    document.markdown_preview_mode = crate::text_preview::MarkdownPreviewMode::Rendered;
    document
}

#[test]
fn rendered_markdown_is_searched_without_markup_and_steps_through_blocks() {
    use crate::text_preview::MarkdownPreviewMode;

    let source = "# Needle title\n\nplain **nee**dle\n\n| a | b |\n| - | - |\n| x | needle |\n";
    let mut document = markdown_document(source);
    search(&mut document, "needle");
    // 「**nee**dle」去掉标记后也是 needle；标题、正文、表格单元格各一处。
    let (matches, current) = document.rendered_search().expect("rendered search");
    assert_eq!(matches.len(), 3);
    assert_eq!(current, Some(0));
    assert_eq!(
        document.search_summary(),
        Some(SearchSummary {
            ordinal: Some(1),
            total: 3,
            finished: true,
        })
    );
    assert_eq!(document.rendered_current_block(), Some(0));
    let reads = document.step_search(SearchDirection::Backward);
    settle(&mut document, reads);
    assert_eq!(
        document.rendered_current_block(),
        Some(2),
        "wrapped to the table"
    );
    // 原文模式里同一个词按文件搜：标记符号里的「**nee**dle」不算。
    let reads = document.select_markdown_preview_mode(MarkdownPreviewMode::Raw);
    settle(&mut document, reads);
    assert!(document.rendered_search().is_none());
    assert_eq!(
        document.search_summary().map(|summary| summary.total),
        Some(2)
    );
}

#[test]
fn going_to_a_line_is_only_for_the_raw_text() {
    use crate::text_preview::MarkdownPreviewMode;

    let mut document = markdown_document("# a\n\nb\n");
    document.open_goto();
    assert!(document.find_bar().is_none());
    let reads = document.select_markdown_preview_mode(MarkdownPreviewMode::Raw);
    settle(&mut document, reads);
    document.open_goto();
    assert!(matches!(
        document.find_bar(),
        Some(FindBar::GotoLine { .. })
    ));
    // 切回渲染模式时跳行条关掉。
    let reads = document.select_markdown_preview_mode(MarkdownPreviewMode::Rendered);
    settle(&mut document, reads);
    assert!(document.find_bar().is_none());
}
