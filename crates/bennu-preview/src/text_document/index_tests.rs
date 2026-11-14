use std::sync::Arc;

use encoding_rs::UTF_8;
use iced::futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::text_document::rows::{LineCountAt, RowReader};

fn numbered_lines(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("line {index} {}\n", "x".repeat(index % 97)))
        .collect::<String>()
        .into_bytes()
}

async fn counted(bytes: Vec<u8>, index: &mut LineIndex, len: u64) {
    let decoding = TextDecoding::new(UTF_8, 0);
    let source: Arc<dyn ByteSource> = Arc::new(bytes);
    let mut progress = count_lines(
        source,
        decoding,
        len,
        index.cursor(),
        index.checkpoints.len(),
        CancellationToken::new(),
    );
    while let Some(step) = progress.next().await {
        index.merge(step);
    }
}

fn naive_lines_before(bytes: &[u8], offset: u64) -> u64 {
    bytes[..offset as usize]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u64
}

#[tokio::test]
async fn checkpoints_give_the_same_line_numbers_as_counting_from_the_start() {
    let bytes = numbered_lines(60_000);
    let len = bytes.len() as u64;
    assert!(len > 3 * CHECKPOINT_BYTES);
    let mut index = LineIndex::new(0);
    counted(bytes.clone(), &mut index, len).await;

    assert_eq!(index.status(len), LineCountStatus::Done { total: 60_000 });
    let reader = RowReader {
        source: &bytes,
        decoding: TextDecoding::new(UTF_8, 0),
        len,
    };
    for offset in (0..len).step_by(77_777).chain([len - 1, CHECKPOINT_BYTES]) {
        let known = index.line_count_before(offset).expect("counted");
        assert!(known.offset <= offset && offset - known.offset < CHECKPOINT_BYTES);
        assert_eq!(
            reader.lines_before(offset, known).expect("count"),
            naive_lines_before(&bytes, offset)
        );
        let floor = index.line_start_floor(offset).expect("floor");
        assert!(floor.start <= offset);
        assert!(floor.start == 0 || bytes[floor.start as usize - 1] == b'\n');
    }
}

#[tokio::test]
async fn counting_resumes_after_the_file_grows() {
    let bytes = numbered_lines(40_000);
    let first_len = bytes.len() as u64 * 2 / 3;
    let mut resumed = LineIndex::new(0);
    counted(bytes.clone(), &mut resumed, first_len).await;
    assert!(matches!(
        resumed.status(first_len),
        LineCountStatus::Done { .. }
    ));
    counted(bytes.clone(), &mut resumed, bytes.len() as u64).await;

    let mut whole = LineIndex::new(0);
    counted(bytes.clone(), &mut whole, bytes.len() as u64).await;
    assert_eq!(resumed.checkpoints, whole.checkpoints);
    assert_eq!(resumed.cursor(), whole.cursor());
}

#[test]
fn a_file_without_a_final_newline_counts_its_last_line() {
    let mut index = LineIndex::new(0);
    index.merge(IndexProgress {
        first_checkpoint: 0,
        checkpoints: vec![Checkpoint {
            lines_before: 0,
            line_start: 0,
        }],
        cursor: IndexCursor {
            counted_until: 7,
            lines_before: 1,
            last_line_start: 4,
        },
        finished: true,
    });
    assert_eq!(index.status(7), LineCountStatus::Done { total: 2 });
    assert_eq!(
        index.status(9),
        LineCountStatus::Counting { lines_so_far: 1 }
    );
    assert_eq!(
        index.line_count_before(3),
        Some(LineCountAt {
            offset: 0,
            lines_before: 0
        })
    );
    assert_eq!(index.line_count_before(8), None);
}

fn naive_line_start(bytes: &[u8], line: u64) -> u64 {
    let mut starts = vec![0_u64];
    starts.extend(
        bytes
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b'\n')
            .map(|(index, _)| index as u64 + 1),
    );
    // 以换行结尾的文件没有空的末行。
    if starts.last() == Some(&(bytes.len() as u64)) && starts.len() > 1 {
        starts.pop();
    }
    starts[(line as usize).min(starts.len() - 1)]
}

#[tokio::test]
async fn line_starts_are_found_from_checkpoints_or_while_still_counting() {
    let bytes = numbered_lines(60_000);
    let len = bytes.len() as u64;
    let reader = RowReader {
        source: &bytes,
        decoding: TextDecoding::new(UTF_8, 0),
        len,
    };
    let mut counting = LineIndex::new(0);
    let mut index = LineIndex::new(0);
    counted(bytes.clone(), &mut index, len).await;
    for line in [0, 1, 7_777, 30_001, 59_999, 60_000, 1_000_000] {
        let expected = naive_line_start(&bytes, line);
        for known in [&index, &mut counting] {
            let seek = known.seek_line(line);
            assert!(seek.lines_before <= line);
            assert_eq!(
                line_start_for_number(&reader, line, seek).expect("line"),
                expected,
                "line {line}"
            );
        }
    }
    // 没有换行结尾的最后一行也能跳到。
    let tail = b"one\ntwo\nthree".to_vec();
    let reader = RowReader {
        source: &tail,
        decoding: TextDecoding::new(UTF_8, 0),
        len: tail.len() as u64,
    };
    let seek = LineIndex::new(0).seek_line(9);
    assert_eq!(line_start_for_number(&reader, 9, seek).expect("line"), 8);
}
