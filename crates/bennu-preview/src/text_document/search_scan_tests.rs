use std::ops::ControlFlow;

use encoding_rs::{Encoding, GBK, UTF_16LE, UTF_8};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::text_document::rows::RowReader;
use crate::text_document::TextDecoding;

/// 编码后的字节（UTF-16 带 BOM）与解码设置。
fn encoded(encoding: &'static Encoding, content: &str) -> (Vec<u8>, TextDecoding) {
    if encoding == UTF_16LE {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(content.encode_utf16().flat_map(u16::to_le_bytes));
        return (bytes, TextDecoding::new(encoding, 2));
    }
    (
        encoding.encode(content).0.into_owned(),
        TextDecoding::new(encoding, 0),
    )
}

fn query(text: &str, case_sensitive: bool) -> SearchMatcher {
    SearchMatcher::new(&SearchQuery {
        text: text.to_owned(),
        case_sensitive,
    })
    .expect("matcher")
}

fn with_scanner<R>(
    bytes: &[u8],
    decoding: TextDecoding,
    matcher: &SearchMatcher,
    run: impl FnOnce(&MatchScanner<'_>) -> R,
) -> R {
    let source: &dyn ByteSource = &bytes.to_vec();
    let scanner = MatchScanner {
        reader: RowReader {
            source,
            decoding,
            len: bytes.len() as u64,
        },
        matcher,
    };
    run(&scanner)
}

/// 扫整篇：所有匹配的显示位置，以及报告过的行首。
fn scan_all(scanner: &MatchScanner<'_>) -> (Vec<TextMatch>, Vec<u64>) {
    let mut found = Vec::new();
    let mut line_starts = Vec::new();
    let end = scanner
        .scan(
            scanner.reader.decoding.data_start(),
            scanner.reader.len,
            &CancellationToken::new(),
            &mut |event| {
                match event {
                    ScanEvent::Found(candidate) => {
                        found.push(scanner.locate(&candidate).expect("locate"))
                    }
                    ScanEvent::LinesScanned { next_line } => line_starts.push(next_line),
                }
                ControlFlow::Continue(())
            },
        )
        .expect("scan");
    assert_eq!(end, ScanEnd::Completed);
    (found, line_starts)
}

fn matched_text(scanner: &MatchScanner<'_>, found: &TextMatch) -> String {
    scanner
        .reader
        .text_between(found.start, found.end)
        .expect("text")
}

#[test]
fn matches_are_counted_across_blocks_and_located_on_their_rows() {
    let content: String = (0..200_000)
        .map(|index| {
            format!(
                "row {index} {}\n",
                if index % 7 == 0 { "Needle" } else { "hay" }
            )
        })
        .collect();
    let (bytes, decoding) = encoded(UTF_8, &content);
    assert!(bytes.len() > 2 * SCAN_BLOCK_BYTES);
    let matcher = query("needle", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let (found, line_starts) = scan_all(scanner);
        assert_eq!(found.len(), 200_000_usize.div_ceil(7));
        for found in &found {
            assert_eq!(matched_text(scanner, found), "Needle");
            assert_eq!(found.start.row, found.line_start);
        }
        // 检查点都在行首：前一个字节是换行。
        assert!(line_starts
            .iter()
            .all(|start| bytes[*start as usize - 1] == b'\n'));
    });
}

#[test]
fn case_sensitivity_is_respected() {
    let (bytes, decoding) = encoded(UTF_8, "Alpha alpha ALPHA\n");
    let insensitive = query("alpha", false);
    let sensitive = query("alpha", true);
    with_scanner(&bytes, decoding, &insensitive, |scanner| {
        assert_eq!(scan_all(scanner).0.len(), 3);
    });
    with_scanner(&bytes, decoding, &sensitive, |scanner| {
        assert_eq!(scan_all(scanner).0.len(), 1);
    });
}

#[test]
fn chinese_is_found_in_legacy_and_utf16_encodings() {
    let content = "第一行\n这里有中文搜索词\r\n又一个中文\n";
    for encoding in [GBK, UTF_16LE, UTF_8] {
        let (bytes, decoding) = encoded(encoding, content);
        let matcher = query("中文", false);
        with_scanner(&bytes, decoding, &matcher, |scanner| {
            let (found, _) = scan_all(scanner);
            assert_eq!(found.len(), 2, "{}", encoding.name());
            for found in &found {
                assert_eq!(matched_text(scanner, found), "中文", "{}", encoding.name());
            }
        });
    }
}

#[test]
fn a_query_only_matches_within_one_line() {
    let (bytes, decoding) = encoded(UTF_8, "foo\nbar\n");
    // 换行之后的部分不要：「foo\nbar」只按「foo」搜。
    let matcher = query("foo\nbar", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        assert_eq!(scan_all(scanner).0.len(), 1);
    });
    assert!(SearchMatcher::new(&SearchQuery::default()).is_none());
}

#[test]
fn matches_across_segment_and_batch_boundaries_are_found_once() {
    // 一行 3 MB 没有换行：超过一块，按显示段分批读。
    let unit = "abcdefghij".repeat(99) + "NEEDLE";
    let line = unit.repeat(3 * SCAN_BLOCK_BYTES / unit.len());
    let content = format!("first\n{line}\nlast NEEDLE\n");
    let (bytes, decoding) = encoded(UTF_8, &content);
    let matcher = query("needle", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let (found, line_starts) = scan_all(scanner);
        assert_eq!(found.len(), line.matches("NEEDLE").count() + 1);
        for found in &found {
            assert_eq!(matched_text(scanner, found), "NEEDLE");
        }
        // 有的匹配跨了段：终点在下一段。
        assert!(found.iter().any(|found| found.start.row != found.end.row));
        let mut sorted = found.clone();
        sorted.sort_by_key(|found| found.start);
        sorted.dedup();
        assert_eq!(sorted.len(), found.len());
        assert!(line_starts.contains(&(6 + line.len() as u64 + 1)));
    });
}

#[test]
fn a_line_longer_than_a_segment_inside_a_block_is_located_by_segment() {
    let line = "x".repeat(25_000) + "target" + &"y".repeat(100);
    let content = format!("{line}\n");
    let (bytes, decoding) = encoded(UTF_8, &content);
    let matcher = query("target", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let (found, _) = scan_all(scanner);
        assert_eq!(found.len(), 1);
        assert_ne!(found[0].start.row, 0, "the match is in a later segment");
        assert_eq!(matched_text(scanner, &found[0]), "target");
    });
}

#[test]
fn the_unterminated_last_line_is_not_a_checkpoint() {
    let (bytes, decoding) = encoded(UTF_8, "one\ntwo needle");
    let matcher = query("needle", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let (found, line_starts) = scan_all(scanner);
        assert_eq!(found.len(), 1);
        assert!(line_starts.is_empty());
    });
}

fn position(found: &TextMatch) -> (u64, usize) {
    (found.start.row, found.start.column)
}

#[test]
fn finding_forward_and_backward_wraps_around() {
    let content: String = (0..40_000)
        .map(|index| {
            format!(
                "line {index} {}\n",
                if index % 10_000 == 5 { "mark" } else { "" }
            )
        })
        .collect();
    let (bytes, decoding) = encoded(UTF_8, &content);
    let matcher = query("mark", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let (all, _) = scan_all(scanner);
        assert_eq!(all.len(), 4);
        let cancel = CancellationToken::new();
        let next = |found: &TextMatch| {
            let from = TextPosition {
                column: found.start.column + 1,
                ..found.start
            };
            find_match(
                scanner,
                found.line_start,
                from,
                SearchDirection::Forward,
                &cancel,
            )
            .expect("find")
            .expect("a match")
        };
        let previous = |found: &TextMatch| {
            find_match(
                scanner,
                found.line_start,
                found.start,
                SearchDirection::Backward,
                &cancel,
            )
            .expect("find")
            .expect("a match")
        };
        for index in 0..all.len() {
            assert_eq!(
                position(&next(&all[index])),
                position(&all[(index + 1) % 4])
            );
            assert_eq!(
                position(&previous(&all[index])),
                position(&all[(index + 3) % 4])
            );
        }
        let none = query("absent", false);
        let scanner = MatchScanner {
            reader: RowReader { ..scanner.reader },
            matcher: &none,
        };
        let start = TextPosition { row: 0, column: 0 };
        assert_eq!(
            find_match(&scanner, 0, start, SearchDirection::Forward, &cancel).expect("find"),
            None
        );
        assert_eq!(
            find_match(&scanner, 0, start, SearchDirection::Backward, &cancel).expect("find"),
            None
        );
    });
}

#[test]
fn several_matches_on_one_line_step_in_order() {
    let (bytes, decoding) = encoded(UTF_8, "ab ab ab\n");
    let matcher = query("ab", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let cancel = CancellationToken::new();
        let from = TextPosition { row: 0, column: 1 };
        let next = find_match(scanner, 0, from, SearchDirection::Forward, &cancel)
            .expect("find")
            .expect("match");
        assert_eq!(next.start.column, 3);
        let previous = find_match(scanner, 0, next.start, SearchDirection::Backward, &cancel)
            .expect("find")
            .expect("match");
        assert_eq!(previous.start.column, 0);
    });
}

#[test]
fn the_ordinal_counts_from_the_nearest_checkpoint() {
    let content: String = (0..50_000)
        .map(|index| {
            format!(
                "{index} {}\n",
                if index % 3 == 0 { "hit hit" } else { "miss" }
            )
        })
        .collect();
    let (bytes, decoding) = encoded(UTF_8, &content);
    let matcher = query("hit", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let mut checkpoints = vec![SearchCheckpoint {
            offset: 0,
            matches_before: 0,
        }];
        let mut found = Vec::new();
        scanner
            .scan(
                0,
                scanner.reader.len,
                &CancellationToken::new(),
                &mut |event| {
                    match event {
                        ScanEvent::Found(candidate) => {
                            found.push(scanner.locate(&candidate).expect("locate"))
                        }
                        ScanEvent::LinesScanned { next_line } => {
                            checkpoints.push(SearchCheckpoint {
                                offset: next_line,
                                matches_before: found.len() as u64,
                            })
                        }
                    }
                    ControlFlow::Continue(())
                },
            )
            .expect("scan");
        for (index, at) in found.iter().enumerate().step_by(997) {
            let from = checkpoints
                [checkpoints.partition_point(|checkpoint| checkpoint.offset <= at.line_start) - 1];
            let ordinal =
                match_ordinal(scanner, *at, from, &CancellationToken::new()).expect("ordinal");
            assert_eq!(ordinal, index as u64 + 1);
        }
    });
}

#[test]
fn a_cancelled_scan_stops() {
    let content = "needle\n".repeat(400_000);
    let (bytes, decoding) = encoded(UTF_8, &content);
    let matcher = query("needle", false);
    with_scanner(&bytes, decoding, &matcher, |scanner| {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let end = scanner
            .scan(0, scanner.reader.len, &cancel, &mut |_| {
                ControlFlow::Continue(())
            })
            .expect("scan");
        assert_eq!(end, ScanEnd::Cancelled);
    });
}
