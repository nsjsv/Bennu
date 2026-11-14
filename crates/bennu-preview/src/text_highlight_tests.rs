use std::path::Path;

use super::*;

fn language(name: &str, first_line: &str) -> Option<String> {
    detect_language(Path::new(name), first_line).map(|language| language.name().to_owned())
}

#[test]
fn common_languages_are_recognised_by_name_extension_or_first_line() {
    for (file, first_line) in [
        ("main.rs", ""),
        ("script.py", ""),
        ("app.js", ""),
        ("app.ts", ""),
        ("main.c", ""),
        ("main.cpp", ""),
        ("main.go", ""),
        ("build.sh", ""),
        ("data.json", ""),
        ("config.yaml", ""),
        ("Cargo.toml", ""),
        ("index.html", ""),
        ("style.css", ""),
        ("query.sql", ""),
        ("README.md", ""),
        ("Dockerfile", ""),
        ("Makefile", ""),
        ("run", "#!/bin/bash"),
    ] {
        assert!(
            language(file, first_line).is_some(),
            "{file} not recognised"
        );
    }
    assert_eq!(language("notes.txt", "hello"), None);
    // 日志也有语法（时间、级别上色），≤ 10 MB 的日志照样高亮。
    assert!(language("server.log", "2026-11-13 INFO start").is_some());
}

fn kinds(language_name: &str, lines: &[&str]) -> Vec<Vec<(String, TokenKind)>> {
    let language = detect_language(Path::new(language_name), "").expect("language");
    let checkpoint = HighlightCheckpoint::at_start(&language, 0);
    let mut highlighter = LineHighlighter::resume(&checkpoint);
    lines
        .iter()
        .map(|line| {
            highlighter
                .line(line)
                .into_iter()
                .map(|token| (line[token.range].to_owned(), token.kind))
                .collect()
        })
        .collect()
}

#[test]
fn rust_tokens_get_their_kinds() {
    let tokens = kinds(
        "main.rs",
        &["fn main() { let answer = 42; // done", "    \"text\""],
    );
    assert!(tokens[0].contains(&("fn".to_owned(), TokenKind::Keyword)));
    assert!(tokens[0].contains(&("main".to_owned(), TokenKind::Function)));
    assert!(tokens[0].contains(&("42".to_owned(), TokenKind::Number)));
    assert!(tokens[0]
        .iter()
        .any(|(text, kind)| text.starts_with("//") && *kind == TokenKind::Comment));
    assert!(tokens[1]
        .iter()
        .any(|(text, kind)| text.contains("text") && *kind == TokenKind::String));
    // 运算符不按关键字上色。
    assert!(!tokens[0].iter().any(|(text, _)| text == "="));
}

#[test]
fn a_block_comment_colours_the_lines_in_between() {
    let tokens = kinds(
        "main.c",
        &["int a; /* start", "still a comment", "end */ int b;"],
    );
    assert_eq!(
        tokens[1],
        vec![("still a comment".to_owned(), TokenKind::Comment)]
    );
    assert!(tokens[2].contains(&("int".to_owned(), TokenKind::Keyword)));
}

#[test]
fn resuming_from_a_checkpoint_matches_parsing_from_the_start() {
    let language = detect_language(Path::new("main.c"), "").expect("language");
    let lines = ["int a;", "/* open", "inside", "close */", "int b;"];
    let mut from_start = LineHighlighter::resume(&HighlightCheckpoint::at_start(&language, 0));
    let mut expected = Vec::new();
    let mut saved = None;
    for (index, line) in lines.iter().enumerate() {
        if index == 2 {
            saved = Some(from_start.checkpoint(0));
        }
        expected.push(from_start.line(line));
    }
    let mut resumed = LineHighlighter::resume(&saved.expect("checkpoint"));
    for (index, line) in lines.iter().enumerate().skip(2) {
        assert_eq!(resumed.line(line), expected[index]);
    }
}

#[test]
fn overlong_lines_are_left_plain() {
    let language = detect_language(Path::new("app.js"), "").expect("language");
    let mut highlighter = LineHighlighter::resume(&HighlightCheckpoint::at_start(&language, 0));
    let long = "var a = 1;".repeat(MAX_HIGHLIGHT_LINE_BYTES / 10 + 1);
    assert!(highlighter.line(&long).is_empty());
    assert!(!highlighter.line("var a = 1;").is_empty());
}

#[test]
fn both_palettes_cover_every_kind_with_distinct_keyword_and_string_colours() {
    for palette in [HighlightPalette::Light, HighlightPalette::Dark] {
        assert_ne!(
            palette.color(TokenKind::Keyword),
            palette.color(TokenKind::String)
        );
    }
}
