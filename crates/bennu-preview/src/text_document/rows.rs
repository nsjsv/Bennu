//! 显示行的读取：向后读、向前读、定位任意偏移所在的行、读一段选区的文字。全部是阻塞
//! I/O，调用方放在 `spawn_blocking` 里跑。
//!
//! 显示行边界只由文件字节决定（见 `decoding` 的切点规则），三种访问方式得到同样的边界：
//! 窗口往前补、往后补、拖动滚动条跳到任意位置之后，同一个字节总在同一行里。

use std::io;
use std::os::unix::fs::FileExt;

use super::decoding::{TextDecoding, CUT_LOOKAHEAD_BYTES, MULTI_BYTE_RESYNC_BYTES, SEGMENT_BYTES};

/// 数换行、往回找行首时每次读的块大小。
const SCAN_BLOCK_BYTES: u64 = 64 * 1024;

/// 读文件字节的来源：真实文件用 `pread`（多个后台任务共用一个句柄互不干扰），
/// 测试用内存。
pub(crate) trait ByteSource: Send + Sync {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize>;

    /// 顺序扫过的一段以后不会马上再读：行号统计扫完 2 GB 不该把页缓存撑满。
    fn release(&self, _offset: u64, _len: u64) {}
}

impl ByteSource for std::fs::File {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        FileExt::read_at(self, buffer, offset)
    }

    fn release(&self, offset: u64, len: u64) {
        use std::os::fd::AsRawFd;
        // 只是建议内核丢掉干净页；失败不影响正确性。
        let _ = unsafe {
            libc::posix_fadvise(
                self.as_raw_fd(),
                offset as libc::off_t,
                len as libc::off_t,
                libc::POSIX_FADV_DONTNEED,
            )
        };
    }
}

impl ByteSource for Vec<u8> {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        let start = (offset as usize).min(self.len());
        let count = buffer.len().min(self.len() - start);
        buffer[..count].copy_from_slice(&self[start..start + count]);
        Ok(count)
    }
}

/// 一个显示行：普通行就是一行，超长行的每一段各是一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow {
    /// 本行第一个字节在文件里的偏移，也是行的身份。
    pub start: u64,
    /// 下一行的起点（含换行单元）。
    pub end: u64,
    /// 所属原始行的起点；`start == line_start` 的是原始行的第一段。
    pub line_start: u64,
    /// 所属原始行的行号（从 0 数）；行号统计还没覆盖到时为 None。
    pub line_number: Option<u64>,
    /// 解码后的文字，不含换行与紧挨换行的回车。
    pub text: String,
    /// 以换行结束（下一行是新的原始行）；超长行中间的段和没有换行的末行为假。
    pub ends_line: bool,
}

impl TextRow {
    pub fn is_line_start(&self) -> bool {
        self.start == self.line_start
    }

    pub(crate) fn row_start(&self) -> RowStart {
        RowStart {
            start: self.start,
            line_start: self.line_start,
        }
    }

    /// 紧跟在这一行后面的那一行的起点。
    pub(crate) fn next_row_start(&self) -> RowStart {
        RowStart {
            start: self.end,
            line_start: if self.ends_line {
                self.end
            } else {
                self.line_start
            },
        }
    }

    /// 紧跟在这一行后面的那一行所属原始行的行号。
    pub(crate) fn next_line_number(&self) -> Option<u64> {
        if self.ends_line {
            self.line_number.map(|number| number + 1)
        } else {
            self.line_number
        }
    }
}

/// 文档里的一个位置：显示行起点 + 该行解码文字里的字节下标。文件末尾是
/// `(文件长度, 0)`，于是「选到文件尾」不需要读到文件尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub row: u64,
    pub column: usize,
}

/// 一个显示行的起点及其所属原始行的起点（往后读需要两者才能接着切段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowStart {
    pub(crate) start: u64,
    pub(crate) line_start: u64,
}

/// 一次读多少：行数与字节数先到哪个停哪个。
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowBudget {
    pub(crate) rows: usize,
    pub(crate) bytes: u64,
}

/// 已知行号的一个位置：`offset` 处之前有 `lines_before` 个换行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineCountAt {
    pub(crate) offset: u64,
    pub(crate) lines_before: u64,
}

/// 文档读取的固定参数：来源、编码与这一刻的文件长度（追加后长度会变，每次读取前取）。
pub(crate) struct RowReader<'a> {
    pub(crate) source: &'a dyn ByteSource,
    pub(crate) decoding: TextDecoding,
    pub(crate) len: u64,
}

impl RowReader<'_> {
    /// 从 `from` 起往后读显示行。`line_number` 是 `from` 所属原始行的行号（未知为 None）。
    pub(crate) fn rows_after(
        &self,
        from: RowStart,
        line_number: Option<u64>,
        budget: RowBudget,
    ) -> io::Result<Vec<TextRow>> {
        let breaks = self.decoding.line_breaks();
        let mut rows = Vec::new();
        let mut position = from.start;
        let mut line_start = from.line_start;
        let mut line_number = line_number;
        let mut buffer = Vec::new();
        let mut buffer_offset = position;
        let mut used_bytes = 0;
        while position < self.len && rows.len() < budget.rows && used_bytes < budget.bytes {
            let segment = (position - line_start) / SEGMENT_BYTES;
            let target = line_start + (segment + 1) * SEGMENT_BYTES;
            let needed_end = (target + CUT_LOOKAHEAD_BYTES).min(self.len);
            self.fill(&mut buffer, &mut buffer_offset, position, needed_end)?;
            let window =
                &buffer[(position - buffer_offset) as usize..(needed_end - buffer_offset) as usize];
            let end = match breaks.next_line_end(window) {
                Some(line_end) if line_end as u64 <= target - position => {
                    position + line_end as u64
                }
                Some(line_end) => {
                    let line_end = position + line_end as u64;
                    let content_end = line_end
                        - breaks.terminator_len(&window[..(line_end - position) as usize]) as u64;
                    self.cut_after(&buffer, buffer_offset, position, target)
                        .filter(|cut| *cut < content_end)
                        .unwrap_or(line_end)
                }
                None if needed_end == self.len && self.len <= target => self.len,
                None => self
                    .cut_after(&buffer, buffer_offset, position, target)
                    .filter(|cut| *cut < self.len)
                    .unwrap_or(self.len),
            };
            let bytes =
                &buffer[(position - buffer_offset) as usize..(end - buffer_offset) as usize];
            let terminator = breaks.terminator_len(bytes);
            rows.push(TextRow {
                start: position,
                end,
                line_start,
                line_number,
                text: self.decoding.decode(&bytes[..bytes.len() - terminator]),
                ends_line: terminator > 0,
            });
            used_bytes += end - position;
            if terminator > 0 {
                line_start = end;
                line_number = line_number.map(|number| number + 1);
            }
            position = end;
        }
        Ok(rows)
    }

    /// 紧挨在 `before`（一个显示行的起点，或文件长度）之前、合计约 `budget.bytes` 字节的
    /// 显示行，按文件顺序返回，最多 `budget.rows` 行。`line_number` 是 `before` 所在原始行
    /// 的行号。先往回定位起点所在行，再往后读到 `before`：边界与往后读完全一致。
    pub(crate) fn rows_before(
        &self,
        before: RowStart,
        line_number: Option<u64>,
        budget: RowBudget,
    ) -> io::Result<Vec<TextRow>> {
        let data_start = self.decoding.data_start();
        if before.start <= data_start {
            return Ok(Vec::new());
        }
        let target = before.start.saturating_sub(budget.bytes).max(data_start);
        // 起点还在 `before` 这一原始行里时行首已知，不用再往回找（超长行里往回翻页靠这个）。
        let floor = (target >= before.line_start).then_some(before);
        let from = self.row_at(target, floor)?;
        let mut rows = self.rows_after(
            from,
            None,
            RowBudget {
                rows: usize::MAX,
                bytes: before.start - from.start,
            },
        )?;
        let mut line_number = line_number;
        let mut crossed_line_start = before.start == before.line_start;
        for row in rows.iter_mut().rev() {
            if crossed_line_start {
                line_number = line_number.and_then(|number| number.checked_sub(1));
            }
            row.line_number = line_number;
            crossed_line_start = row.is_line_start();
        }
        let excess = rows.len().saturating_sub(budget.rows);
        rows.drain(..excess);
        Ok(rows)
    }

    /// `offset` 所在的显示行（`offset` 超出文件时取最后一行）。`floor` 是不晚于 `offset`
    /// 的已知行首，往回找行首最多找到那里。
    pub(crate) fn row_at(&self, offset: u64, floor: Option<RowStart>) -> io::Result<RowStart> {
        let data_start = self.decoding.data_start();
        let unit = self.decoding.line_breaks().unit_len() as u64;
        let offset = offset.min(self.len.saturating_sub(unit).max(data_start));
        let line_start = self.line_start_of(offset, floor)?;
        let mut segment = (offset - line_start) / SEGMENT_BYTES;
        loop {
            if segment == 0 {
                return Ok(RowStart {
                    start: line_start,
                    line_start,
                });
            }
            let start = self.segment_start(line_start, segment)?;
            // 切点落在行尾换行上时这一段不存在（往后读时同样不会在那里切）。
            if start <= offset && self.has_content_at(start)? {
                return Ok(RowStart { start, line_start });
            }
            segment -= 1;
        }
    }

    /// `offset` 处是不是还有行内文字（不是文件尾，也不是换行或「回车+换行」的开头）。
    fn has_content_at(&self, offset: u64) -> io::Result<bool> {
        let breaks = self.decoding.line_breaks();
        let unit = breaks.unit_len();
        let mut bytes = [0_u8; 4];
        let read = self.read_full(offset, &mut bytes[..unit * 2])?;
        if read < unit {
            return Ok(false);
        }
        let head = &bytes[..read];
        let starts_terminator = |length: usize| {
            head.len() >= length && breaks.terminator_len(&head[..length]) == length
        };
        Ok(!starts_terminator(unit) && !starts_terminator(unit * 2))
    }

    /// 读 `[from, to)` 之间的文字。同一原始行的各段直接相连，原始行之间补 `\n`。
    /// `to.row` 可以是文件长度（选到文件尾）。
    pub(crate) fn text_between(&self, from: TextPosition, to: TextPosition) -> io::Result<String> {
        let first = self.row_at(from.row, None)?;
        let rows = self.rows_after(
            first,
            None,
            RowBudget {
                rows: usize::MAX,
                bytes: to.row.saturating_sub(first.start) + 1,
            },
        )?;
        let mut out = String::new();
        for (index, row) in rows.iter().enumerate() {
            if row.start > to.row {
                break;
            }
            if index > 0 && row.is_line_start() {
                out.push('\n');
            }
            let low = if row.start == from.row {
                row.text.floor_char_boundary(from.column)
            } else {
                0
            };
            let high = if row.start == to.row {
                row.text.floor_char_boundary(to.column)
            } else {
                row.text.len()
            };
            if low < high {
                out.push_str(&row.text[low..high]);
            }
        }
        Ok(out)
    }

    /// `offset` 之前（含 `offset` 处结束的换行）有多少个换行单元，从已知位置 `known` 数起。
    pub(crate) fn lines_before(&self, offset: u64, known: LineCountAt) -> io::Result<u64> {
        let breaks = self.decoding.line_breaks();
        let mut count = known.lines_before;
        let mut position = known.offset;
        let mut block = vec![0; SCAN_BLOCK_BYTES as usize];
        while position < offset {
            let wanted = (offset - position).min(SCAN_BLOCK_BYTES) as usize;
            let read = self.read_full(position, &mut block[..wanted])?;
            let complete = breaks.complete_len(read);
            if complete == 0 {
                break;
            }
            count += breaks.count(&block[..complete]);
            position += complete as u64;
        }
        Ok(count)
    }

    /// `offset` 所在原始行的行首：`offset` 处或之前最后一个换行单元之后。`floor` 是已知
    /// 的某个行首，找到它还没遇到换行就是它。
    pub(crate) fn line_start_of(&self, offset: u64, floor: Option<RowStart>) -> io::Result<u64> {
        let data_start = self.decoding.data_start();
        let breaks = self.decoding.line_breaks();
        let unit = breaks.unit_len() as u64;
        let lowest = floor.map_or(data_start, |floor| floor.line_start.max(data_start));
        // 以 `offset` 结束的换行单元也算（`offset` 本身就是行首）。
        let mut high = (offset + unit).min(self.len).max(lowest);
        high -= (high - data_start) % unit;
        let mut block = vec![0; SCAN_BLOCK_BYTES as usize];
        while high > lowest {
            let low = high.saturating_sub(SCAN_BLOCK_BYTES).max(lowest);
            let low = low + (low - data_start) % unit;
            let read = self.read_full(low, &mut block[..(high - low) as usize])?;
            let complete = breaks.complete_len(read);
            if let Some(line_end) = breaks.last_line_end(&block[..complete]) {
                let line_start = low + line_end as u64;
                if line_start <= offset {
                    return Ok(line_start);
                }
                // 换行正好结束在 `offset` 之后（`offset` 落在换行单元里）：再往前找。
                high = line_start - unit;
                continue;
            }
            high = low;
        }
        Ok(lowest)
    }

    /// 原始行里第 `segment` 段的起点。
    fn segment_start(&self, line_start: u64, segment: u64) -> io::Result<u64> {
        if segment == 0 {
            return Ok(line_start);
        }
        let target = line_start + segment * SEGMENT_BYTES;
        let low = target
            .saturating_sub(MULTI_BYTE_RESYNC_BYTES)
            .max(line_start);
        let high = (target + CUT_LOOKAHEAD_BYTES).min(self.len);
        let mut bytes = vec![0; (high - low) as usize];
        let read = self.read_full(low, &mut bytes)?;
        bytes.truncate(read);
        let known = (low == line_start).then_some(line_start);
        if let Some(cut) = self.decoding.segment_cut(&bytes, low, known, target) {
            return Ok(cut);
        }
        // 旧式多字节编码附近没有同步点：从行首重读（只在没有一个 ASCII 标点的超长行里发生）。
        let mut bytes = vec![0; (high - line_start) as usize];
        let read = self.read_full(line_start, &mut bytes)?;
        bytes.truncate(read);
        Ok(self
            .decoding
            .segment_cut(&bytes, line_start, Some(line_start), target)
            .expect("the line start is a known boundary"))
    }

    fn cut_after(
        &self,
        buffer: &[u8],
        buffer_offset: u64,
        row_start: u64,
        target: u64,
    ) -> Option<u64> {
        self.decoding
            .segment_cut(buffer, buffer_offset, Some(row_start), target)
            .filter(|cut| *cut > row_start)
    }

    /// 让 `buffer` 盖住 `[position, needed_end)`：丢掉 `position` 之前的，补读后面的。
    fn fill(
        &self,
        buffer: &mut Vec<u8>,
        buffer_offset: &mut u64,
        position: u64,
        needed_end: u64,
    ) -> io::Result<()> {
        let consumed = (position - *buffer_offset) as usize;
        buffer.drain(..consumed.min(buffer.len()));
        *buffer_offset = position;
        let have_end = position + buffer.len() as u64;
        if have_end < needed_end {
            let old_len = buffer.len();
            buffer.resize((needed_end - position) as usize, 0);
            let read = self.read_full(have_end, &mut buffer[old_len..])?;
            buffer.truncate(old_len + read);
        }
        Ok(())
    }

    fn read_full(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        read_full(self.source, offset, buffer)
    }
}

/// 读满 `buffer`，到文件尾为止；返回读到的字节数。
pub(crate) fn read_full(
    source: &dyn ByteSource,
    offset: u64,
    buffer: &mut [u8],
) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read_at(offset + filled as u64, &mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
