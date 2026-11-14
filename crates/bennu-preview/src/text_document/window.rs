//! 文档窗口的维护：跳转替换、前后拼接、按视口补读、超限裁剪、窗口首行编号。

use super::open::{JumpPlace, RowBatch};
use super::rows::RowStart;
use super::{
    RowsRequest, TextPreviewDocument, TextRead, ViewerJump, PREFETCH_ROWS, WINDOW_MAX_BYTES,
    WINDOW_MAX_ROWS,
};

impl TextPreviewDocument {
    pub(super) fn replace_window(&mut self, batch: RowBatch) {
        // 旧的可见范围不属于新窗口；等查看器报告新的。
        self.viewport = None;
        self.at_start = batch.at_start;
        self.at_end = batch.at_end;
        self.rows = batch.rows.into();
        self.revision += 1;
        if let Some((row, place)) = batch.focus {
            self.jump = Some(ViewerJump {
                revision: self.jump.map_or(1, |jump| jump.revision + 1),
                row,
                place,
            });
        }
    }

    pub(super) fn append_rows(&mut self, batch: RowBatch) {
        let continues = self.rows.back().is_none_or(|last| {
            batch
                .rows
                .first()
                .is_none_or(|first| first.start == last.end)
        });
        if !continues {
            return;
        }
        self.rows.extend(batch.rows);
        self.at_end = batch.at_end;
        self.revision += 1;
        if self.follow_end && self.at_end {
            self.follow_end = false;
            if let Some(last) = self.rows.back() {
                self.jump = Some(ViewerJump {
                    revision: self.jump.map_or(1, |jump| jump.revision + 1),
                    row: last.start,
                    place: JumpPlace::Bottom,
                });
            }
        }
    }

    pub(super) fn prepend_rows(&mut self, batch: RowBatch) {
        let continues = self
            .rows
            .front()
            .is_none_or(|first| batch.rows.last().is_none_or(|last| last.end == first.start));
        if !continues {
            return;
        }
        for row in batch.rows.into_iter().rev() {
            self.rows.push_front(row);
        }
        self.at_start = batch.at_start;
        self.revision += 1;
    }

    /// 视口两侧剩下的行数；查看器还没报告当前窗口里的可见范围时不知道（窗口刚被
    /// 跳转替换，旧的可见范围已经不在里面了）。
    pub(super) fn margins(&self) -> Option<(usize, usize)> {
        let viewport = self.viewport?;
        let first = self.row_index_containing(viewport.first_row)?;
        let last = self.row_index_containing(viewport.last_row)?;
        Some((first, self.rows.len().saturating_sub(last + 1)))
    }

    pub(super) fn extension_reads(&mut self) -> Vec<TextRead> {
        if self.jump_in_flight {
            return Vec::new();
        }
        // 窗口空了（空文件后来写进了内容、末尾几行被丢掉重读）：查看器没有可见行可报，
        // 直接从头读。
        if self.rows.is_empty() && !self.at_end && !self.forward_in_flight {
            self.forward_in_flight = true;
            let start = self.decoding.data_start();
            return vec![TextRead::Rows {
                generation: self.generation,
                request: RowsRequest::Forward {
                    from: RowStart {
                        start,
                        line_start: start,
                    },
                    line_number: Some(0),
                },
            }];
        }
        let Some((before, after)) = self.margins() else {
            return Vec::new();
        };
        let mut reads = Vec::new();
        if !self.at_end && !self.forward_in_flight && after < PREFETCH_ROWS {
            let last = self.rows.back().expect("a visible row is in the window");
            let from = last.next_row_start();
            let line_number = last.next_line_number();
            self.forward_in_flight = true;
            reads.push(TextRead::Rows {
                generation: self.generation,
                request: RowsRequest::Forward { from, line_number },
            });
        }
        if !self.at_start && !self.backward_in_flight && before < PREFETCH_ROWS {
            if let Some(first) = self.rows.front() {
                self.backward_in_flight = true;
                reads.push(TextRead::Rows {
                    generation: self.generation,
                    request: RowsRequest::Backward {
                        before: first.row_start(),
                        line_number: first.line_number,
                    },
                });
            }
        }
        reads
    }

    pub(super) fn numbering_read(&mut self) -> Option<TextRead> {
        if self.numbering_in_flight || self.jump_in_flight {
            return None;
        }
        let first = self.rows.front()?;
        if first.line_number.is_some() {
            return None;
        }
        let known = self.index.line_count_before(first.line_start)?;
        self.numbering_in_flight = true;
        Some(TextRead::Number {
            generation: self.generation,
            line_start: first.line_start,
            known,
        })
    }

    /// 超出上限时从离视口远的一端丢，两侧至少留两倍预取余量。
    pub(super) fn trim_window(&mut self) {
        let keep = PREFETCH_ROWS * 2;
        let mut bytes: u64 = self.rows.iter().map(|row| row.end - row.start).sum();
        while self.rows.len() > WINDOW_MAX_ROWS || bytes > WINDOW_MAX_BYTES {
            let Some((before, after)) = self.margins() else {
                break;
            };
            let removed = if before >= after && before > keep {
                self.at_start = false;
                self.rows.pop_front()
            } else if after > keep {
                self.at_end = false;
                self.rows.pop_back()
            } else {
                None
            };
            let Some(removed) = removed else {
                break;
            };
            bytes -= removed.end - removed.start;
            self.revision += 1;
        }
        if let (Some(first), Some(last)) = (self.rows.front(), self.rows.back()) {
            self.highlight.retain_window(first.start, last.end);
        }
    }
}
