//! 测试里替引擎同步执行文档要的读取（行号统计与高亮除外），把结果交回文档，直到没有新请求。

use std::ops::ControlFlow;

use super::*;

pub(crate) fn settle(document: &mut TextPreviewDocument, mut reads: Vec<TextRead>) {
    for _ in 0..50 {
        let mut next = Vec::new();
        for read in reads {
            next.extend(execute(document, read));
        }
        if next.is_empty() {
            return;
        }
        reads = next;
    }
    panic!("reads never settled");
}

fn execute(document: &mut TextPreviewDocument, read: TextRead) -> Vec<TextRead> {
    let source = document.source();
    let reader = RowReader {
        source: source.as_ref(),
        decoding: document.decoding(),
        len: document.len(),
    };
    match read {
        TextRead::Rows {
            generation,
            request,
        } => {
            let outcome = read_rows(
                source.as_ref(),
                document.decoding(),
                document.len(),
                request,
            )
            .map_err(|error| error.to_string());
            document.accept_rows(generation, request, outcome)
        }
        TextRead::Number {
            generation,
            line_start,
            known,
        } => {
            let outcome = reader
                .lines_before(line_start, known)
                .map_err(|error| error.to_string());
            document.accept_numbering(generation, line_start, outcome)
        }
        TextRead::SearchCount {
            generation,
            matcher,
            start,
            cancel,
        } => {
            let scanner = MatchScanner {
                reader,
                matcher: &matcher,
            };
            let mut progress = Vec::new();
            search_scan::count_from(&scanner, start, &cancel, &mut |step| {
                progress.push(step);
                ControlFlow::Continue(())
            });
            progress
                .into_iter()
                .flat_map(|step| document.accept_search_counted(generation, step))
                .collect()
        }
        TextRead::SearchFind {
            generation,
            matcher,
            line_start,
            from,
            direction,
            cancel,
        } => {
            let scanner = MatchScanner {
                reader,
                matcher: &matcher,
            };
            let outcome = find_match(&scanner, line_start, from, direction, &cancel)
                .map_err(|error| error.to_string());
            document.accept_search_found(generation, outcome)
        }
        TextRead::SearchOrdinal {
            generation,
            matcher,
            at,
            from,
            cancel,
        } => {
            let scanner = MatchScanner {
                reader,
                matcher: &matcher,
            };
            let outcome =
                match_ordinal(&scanner, at, from, &cancel).map_err(|error| error.to_string());
            document.accept_search_ordinal(generation, at, outcome);
            Vec::new()
        }
        TextRead::LineStart { line, seek } => {
            let outcome =
                line_start_for_number(&reader, line, seek).map_err(|error| error.to_string());
            document.accept_line_start(line, outcome)
        }
        TextRead::Count { .. } | TextRead::Highlight(_) | TextRead::HighlightRows(_) => Vec::new(),
    }
}
