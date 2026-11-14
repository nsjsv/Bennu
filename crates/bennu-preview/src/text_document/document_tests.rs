use super::*;

/// 测试里替引擎执行文档要的读取（行号统计除外），把结果交回文档，直到没有新请求。
fn settle(document: &mut TextPreviewDocument, mut reads: Vec<TextRead>) {
    for _ in 0..50 {
        let mut next = Vec::new();
        for read in reads {
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
                    next.extend(document.accept_rows(generation, request, outcome));
                }
                TextRead::Number {
                    generation,
                    line_start,
                    known,
                } => {
                    let outcome = reader
                        .lines_before(line_start, known)
                        .map_err(|error| error.to_string());
                    next.extend(document.accept_numbering(generation, line_start, outcome));
                }
                TextRead::Count { .. } => {}
            }
        }
        if next.is_empty() {
            return;
        }
        reads = next;
    }
    panic!("reads never settled");
}

fn numbered_lines(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("line {index}\n"))
        .collect::<String>()
        .into_bytes()
}

fn viewport_at(document: &TextPreviewDocument, first: usize, rows: usize) -> TextViewport {
    let window = document.rows();
    TextViewport {
        first_row: window[first].start,
        last_row: window[(first + rows - 1).min(window.len() - 1)].start,
        at_bottom: false,
        cursor: None,
    }
}

fn assert_contiguous(document: &TextPreviewDocument) {
    for pair in document.rows().iter().collect::<Vec<_>>().windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
}

#[test]
fn a_small_file_is_read_whole_and_asks_for_nothing_more() {
    let mut document = document_from_bytes(numbered_lines(100), TextJump::Start);
    assert!(document.at_start() && document.at_end());
    assert_eq!(document.rows().len(), 100);
    let viewport = viewport_at(&document, 90, 10);
    assert!(document.accept_viewport(viewport).is_empty());
}

#[test]
fn reading_near_the_window_edges_extends_the_window_in_both_directions() {
    let mut document = document_from_bytes(numbered_lines(200_000), TextJump::Start);
    assert!(!document.at_end());
    let first_len = document.rows().len();

    // 视口靠近窗口底部：往后补，行号接着编。
    let viewport = viewport_at(&document, first_len - 20, 20);
    let reads = document.accept_viewport(viewport);
    assert_eq!(reads.len(), 1);
    settle(&mut document, reads);
    assert!(document.rows().len() > first_len);
    assert_contiguous(&document);
    let last = document.rows().back().expect("rows");
    assert_eq!(
        last.text,
        format!("line {}", last.line_number.expect("numbered"))
    );

    // 跳到中间：窗口整个换掉，前后都能补。
    let middle = document.len() / 2;
    let reads = document.jump_to(TextJump::Offset(middle));
    settle(&mut document, reads);
    let jump = document.jump().expect("jump");
    let focus = document.row_index(jump.row).expect("focus row");
    assert!(document.rows()[focus].start <= middle && middle < document.rows()[focus].end);
    let viewport = viewport_at(&document, 0, 20);
    let reads = document.accept_viewport(viewport);
    assert!(reads.iter().any(|read| matches!(
        read,
        TextRead::Rows {
            request: RowsRequest::Backward { .. },
            ..
        }
    )));
    settle(&mut document, reads);
    assert_contiguous(&document);
}

#[test]
fn only_the_latest_of_several_quick_jumps_is_applied() {
    let mut document = document_from_bytes(numbered_lines(200_000), TextJump::Start);
    let first = document.jump_to(TextJump::Offset(100_000));
    assert_eq!(first.len(), 1);
    // 拖滑块：第一次还在读，后面的只记最新的。
    assert!(document.jump_to(TextJump::Offset(500_000)).is_empty());
    assert!(document.jump_to(TextJump::End).is_empty());

    settle(&mut document, first);
    let jump = document.jump().expect("jump");
    assert_eq!(jump.place, JumpPlace::Bottom);
    assert!(document.at_end());
    assert_eq!(
        document.rows().back().map(|row| row.text.as_str()),
        Some("line 199999")
    );
}

#[test]
fn stale_reads_for_a_replaced_window_are_dropped() {
    let mut document = document_from_bytes(numbered_lines(200_000), TextJump::Start);
    let first_len = document.rows().len();
    let viewport = viewport_at(&document, first_len - 20, 20);
    let stale = document.accept_viewport(viewport);
    let jump = document.jump_to(TextJump::End);
    settle(&mut document, jump);
    let rows_after_jump = document.rows().clone();
    settle(&mut document, stale);
    assert_eq!(*document.rows(), rows_after_jump);
}

#[test]
fn the_window_stays_bounded_while_scrolling_through_a_large_file() {
    let mut document = document_from_bytes(numbered_lines(400_000), TextJump::Start);
    for _ in 0..40 {
        let len = document.rows().len();
        let viewport = viewport_at(&document, len.saturating_sub(30), 20);
        let reads = document.accept_viewport(viewport);
        settle(&mut document, reads);
        assert!(document.rows().len() <= WINDOW_MAX_ROWS + EXTEND_BUDGET.rows);
        assert_contiguous(&document);
    }
    assert!(!document.at_start());
}

#[test]
fn numbering_fills_line_numbers_once_the_count_covers_the_window() {
    let bytes = numbered_lines(200_000);
    let len = bytes.len() as u64;
    let mut document = document_from_bytes(bytes, TextJump::Start);
    let reads = document.jump_to(TextJump::Offset(len / 2));
    settle(&mut document, reads);
    // 跳转时还没统计到：行号空着。
    let jump = document.jump().expect("jump");
    let focus = document.row_index(jump.row).expect("focus");
    let focus_text = document.rows()[focus].text.clone();

    // 统计完成：补行号。
    let cursor = IndexCursor {
        counted_until: len,
        lines_before: 200_000,
        last_line_start: len,
    };
    let checkpoints = (0..=(len / index::CHECKPOINT_BYTES))
        .map(|checkpoint| {
            let offset = checkpoint * index::CHECKPOINT_BYTES;
            let source = document.source();
            let reader = RowReader {
                source: source.as_ref(),
                decoding: document.decoding(),
                len,
            };
            let lines_before = reader
                .lines_before(
                    offset,
                    LineCountAt {
                        offset: 0,
                        lines_before: 0,
                    },
                )
                .expect("count");
            index::Checkpoint {
                lines_before,
                line_start: reader.line_start_of(offset, None).expect("line start"),
            }
        })
        .collect();
    let reads = document.accept_index_progress(IndexProgress {
        first_checkpoint: 0,
        checkpoints,
        cursor,
        finished: true,
    });
    settle(&mut document, reads);
    let number = document.rows()[focus].line_number.expect("numbered");
    assert_eq!(focus_text, format!("line {number}"));
    assert_eq!(
        document.line_count_status(),
        LineCountStatus::Done { total: 200_000 }
    );
}

#[test]
fn appended_lines_follow_the_end_only_when_the_viewport_was_at_the_bottom() {
    let mut bytes = numbered_lines(50);
    let mut document = document_from_bytes(bytes.clone(), TextJump::Start);
    let viewport = TextViewport {
        at_bottom: true,
        ..viewport_at(&document, 40, 10)
    };
    assert!(document.accept_viewport(viewport).is_empty());

    bytes.extend_from_slice(b"line 50\nline 51\n");
    let len = bytes.len() as u64;
    // 测试文档的来源换成追加后的字节（真实文件句柄读到的就是新内容）。
    document.source = Arc::new(bytes);
    let stamp = FileStamp {
        len,
        ..document.stamp().clone()
    };
    let reads = document.accept_append(stamp);
    settle(&mut document, reads);
    assert!(document.at_end());
    assert_eq!(document.rows().len(), 52);
    let jump = document.jump().expect("follow jump");
    assert_eq!(jump.place, JumpPlace::Bottom);
    assert_eq!(jump.row, document.rows().back().expect("last").start);

    // 不在底部时位置不动：没有新的跳转。
    let revision = jump.revision;
    let viewport = viewport_at(&document, 0, 10);
    let reads = document.accept_viewport(viewport);
    settle(&mut document, reads);
    let mut more = numbered_lines(53);
    more.extend_from_slice(b"tail\n");
    let len = more.len() as u64;
    document.source = Arc::new(more);
    let stamp = FileStamp {
        len,
        ..document.stamp().clone()
    };
    let reads = document.accept_append(stamp);
    settle(&mut document, reads);
    assert_eq!(document.jump().map(|jump| jump.revision), Some(revision));
    assert_eq!(
        document.rows().back().map(|row| row.text.as_str()),
        Some("tail")
    );
}

#[test]
fn copying_more_than_fifteen_megabytes_is_refused() {
    let from = TextPosition { row: 0, column: 0 };
    assert!(TextPreviewDocument::copy_fits(
        from,
        TextPosition {
            row: COPY_LIMIT_BYTES,
            column: 0
        }
    ));
    assert!(!TextPreviewDocument::copy_fits(
        from,
        TextPosition {
            row: COPY_LIMIT_BYTES + 1,
            column: 0
        }
    ));
}
