use encoding_rs::{Encoding, GBK, UTF_16LE, UTF_8};

use super::*;

const ALL: RowBudget = RowBudget {
    rows: usize::MAX,
    bytes: u64::MAX,
};

/// 一个测试文件：编码后的字节（UTF-16 带 BOM）与期望读出的原始行。
struct Sample {
    bytes: Vec<u8>,
    decoding: TextDecoding,
    lines: Vec<String>,
}

fn sample(encoding: &'static Encoding, content: &str) -> Sample {
    let (bytes, data_start) = if encoding == UTF_16LE {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(content.encode_utf16().flat_map(u16::to_le_bytes));
        (bytes, 2)
    } else {
        (encoding.encode(content).0.into_owned(), 0)
    };
    let mut lines: Vec<String> = content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect();
    // 以换行结尾的文件没有空的末行。
    if content.ends_with('\n') || content.is_empty() {
        lines.pop();
    }
    Sample {
        bytes,
        decoding: TextDecoding::new(encoding, data_start),
        lines,
    }
}

fn reader(sample: &Sample) -> RowReader<'_> {
    RowReader {
        source: &sample.bytes,
        decoding: sample.decoding,
        len: sample.bytes.len() as u64,
    }
}

fn all_rows(sample: &Sample) -> Vec<TextRow> {
    let start = sample.decoding.data_start();
    reader(sample)
        .rows_after(
            RowStart {
                start,
                line_start: start,
            },
            Some(0),
            ALL,
        )
        .expect("rows")
}

fn mixed_content() -> String {
    let long_ascii = "abcdefghij".repeat(2_500);
    let long_chinese =
        "超长的中文行，夹着 ASCII 标点, 和组合符号 e\u{301} 以及 emoji 👨\u{200d}👩\u{200d}👧。"
            .repeat(400);
    format!(
        "first line\r\nsecond\rnot a break\n\n{long_ascii}\n{long_chinese}\nshort\r\n{long_ascii}"
    )
}

fn samples() -> Vec<Sample> {
    let content = mixed_content();
    vec![
        sample(UTF_8, &content),
        sample(UTF_8, &format!("{content}\n")),
        sample(UTF_16LE, &content),
        sample(
            GBK,
            &content.replace(['👨', '👩', '👧', '\u{200d}', '\u{301}'], "x"),
        ),
        sample(UTF_8, ""),
        sample(UTF_8, "only\n"),
    ]
}

#[test]
fn rows_cover_the_file_and_rejoin_into_the_original_lines() {
    for sample in samples() {
        let rows = all_rows(&sample);
        let data_start = sample.decoding.data_start();
        let len = sample.bytes.len() as u64;
        assert_eq!(rows.first().map_or(data_start, |row| row.start), data_start);
        assert_eq!(rows.last().map_or(data_start, |row| row.end), len);
        for pair in rows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        // 段内不超过一段的长度（加上找切点的余量）。
        for row in &rows {
            assert!(row.end - row.start <= SEGMENT_BYTES + CUT_LOOKAHEAD_BYTES + 2);
        }
        let mut lines: Vec<String> = Vec::new();
        for row in &rows {
            if row.is_line_start() {
                lines.push(String::new());
            }
            lines.last_mut().expect("a line").push_str(&row.text);
        }
        assert_eq!(lines, sample.lines, "{}", sample.decoding.encoding().name());
        // 行号：每个原始行的行号就是它的序号。
        let mut expected_line = 0;
        for row in &rows {
            if row.is_line_start() && row.start != data_start {
                expected_line += 1;
            }
            assert_eq!(row.line_number, Some(expected_line));
        }
    }
}

#[test]
fn every_access_path_agrees_on_row_boundaries() {
    for sample in samples() {
        let rows = all_rows(&sample);
        let reader = reader(&sample);
        for (index, row) in rows.iter().enumerate() {
            // 定位行内任意位置都落回这一行。
            for offset in [
                row.start,
                row.start + 1,
                (row.start + row.end) / 2,
                row.end - 1,
            ] {
                if offset >= row.end {
                    continue;
                }
                assert_eq!(
                    reader.row_at(offset, None).expect("row at"),
                    row.row_start(),
                    "offset {offset}"
                );
            }
            // 从这一行往后读与从头读的结果相同。
            let after = reader
                .rows_after(row.row_start(), row.line_number, ALL)
                .expect("rows after");
            assert_eq!(after, rows[index..]);
            // 往前读到这一行为止，与从头读的前缀相同。
            let before = reader
                .rows_before(
                    row.row_start(),
                    row.line_number,
                    RowBudget {
                        rows: 50,
                        bytes: 30_000,
                    },
                )
                .expect("rows before");
            assert_eq!(before, rows[index - before.len()..index]);
            assert!(index == 0 || !before.is_empty());
        }
    }
}

#[test]
fn line_counts_match_row_line_numbers() {
    for sample in samples() {
        let reader = reader(&sample);
        let start = LineCountAt {
            offset: sample.decoding.data_start(),
            lines_before: 0,
        };
        for row in all_rows(&sample).iter().filter(|row| row.is_line_start()) {
            assert_eq!(
                reader.lines_before(row.start, start).expect("count"),
                row.line_number.expect("numbered")
            );
        }
    }
}

#[test]
fn text_between_joins_segments_without_breaks_and_lines_with_newlines() {
    let sample = sample(UTF_8, &mixed_content());
    let rows = all_rows(&sample);
    let reader = reader(&sample);
    let whole = sample.lines.join("\n");
    let everything = reader
        .text_between(
            TextPosition { row: 0, column: 0 },
            TextPosition {
                row: sample.bytes.len() as u64,
                column: 0,
            },
        )
        .expect("text");
    assert_eq!(everything, whole);

    // 从第 2 行第 3 个字节到某个长行中间那一段的第 5 个字节。
    let from = TextPosition {
        row: rows[1].start,
        column: 3,
    };
    let to_row = rows
        .iter()
        .find(|row| !row.is_line_start())
        .expect("a segment");
    let to = TextPosition {
        row: to_row.start,
        column: 5,
    };
    let expected_start = sample.lines[0].len() + 1 + 3;
    let expected_end = rows
        .iter()
        .take_while(|row| row.start < to_row.start)
        .fold(0, |length, row| {
            length + row.text.len() + usize::from(row.is_line_start() && row.start != 0)
        })
        + 5;
    assert_eq!(
        reader.text_between(from, to).expect("text"),
        whole[expected_start..expected_end]
    );
}
