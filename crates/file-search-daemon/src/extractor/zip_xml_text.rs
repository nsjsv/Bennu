use std::fs;
use std::io::BufReader;
use std::path::Path;

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Decoder, Reader};
use zip::ZipArchive;

use crate::error::{SearchError, SearchResult};

use super::{ExtractionOutcome, ExtractionStatus};

/// 共享字符串表的条目数上限。字符串总字节数受输出预算约束，但空条目不占字节；
/// 没有硬上限时恶意压缩包可以用海量空条目撑大 daemon 常驻内存。
const MAX_SHARED_STRING_ENTRIES: usize = 262_144;

/// zip 容器 + XML 正文的文档格式。这些格式的正文提取都在进程内完成，
/// 不依赖外部工具，因此不存在 ToolUnavailable 降级路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZippedXmlDocumentKind {
    WordDocument,
    Spreadsheet,
    Presentation,
    OpenDocumentText,
}

pub(crate) fn zipped_xml_document_kind(extension: &str) -> Option<ZippedXmlDocumentKind> {
    match extension {
        "docx" => Some(ZippedXmlDocumentKind::WordDocument),
        "xlsx" => Some(ZippedXmlDocumentKind::Spreadsheet),
        "pptx" => Some(ZippedXmlDocumentKind::Presentation),
        "odt" => Some(ZippedXmlDocumentKind::OpenDocumentText),
        _ => None,
    }
}

/// 累积正文文本并强制输出预算。zip 条目可以声明任意大的解压尺寸（zip 炸弹），
/// 这个预算计数是唯一的可信边界，所有 XML 文本都必须经过它。
struct BoundedText {
    text: String,
    max_bytes: u64,
    limit_exceeded: bool,
}

impl BoundedText {
    fn new(max_bytes: u64) -> Self {
        Self {
            text: String::new(),
            max_bytes,
            limit_exceeded: false,
        }
    }

    fn push_str(&mut self, fragment: &str) {
        if self.limit_exceeded || fragment.is_empty() {
            return;
        }
        let remaining = (self.max_bytes.saturating_sub(self.text.len() as u64)) as usize;
        if fragment.len() <= remaining {
            self.text.push_str(fragment);
            return;
        }
        let mut retained_bytes = remaining;
        while retained_bytes > 0 && !fragment.is_char_boundary(retained_bytes) {
            retained_bytes -= 1;
        }
        self.text.push_str(&fragment[..retained_bytes]);
        self.limit_exceeded = true;
    }

    fn end_paragraph(&mut self) {
        self.push_str("\n");
    }

    fn outcome(self) -> ExtractionOutcome {
        if self.limit_exceeded {
            ExtractionOutcome::skipped(ExtractionStatus::TooLarge)
        } else {
            ExtractionOutcome::text(self.text)
        }
    }
}

/// XML 解析或结构失败统一降级为 Skipped；crawl 不应因为单个损坏文档而中断。
fn xml_failure_outcome(message: impl std::fmt::Display) -> ExtractionOutcome {
    ExtractionOutcome::skipped(ExtractionStatus::ReadFailed {
        message: message.to_string(),
    })
}

pub(crate) fn extract_zipped_xml_text(
    path: &Path,
    document_kind: ZippedXmlDocumentKind,
    max_output_bytes: u64,
) -> SearchResult<ExtractionOutcome> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(source) => {
            if source.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(SearchError::Inaccessible {
                    path: path.to_path_buf(),
                    source,
                });
            }
            return Ok(ExtractionOutcome::skipped(ExtractionStatus::ReadFailed {
                message: source.to_string(),
            }));
        }
    };
    let mut archive = match ZipArchive::new(BufReader::new(file)) {
        Ok(archive) => archive,
        Err(source) => {
            return Ok(ExtractionOutcome::skipped(ExtractionStatus::ReadFailed {
                message: format!("not a readable office container: {source}"),
            }))
        }
    };

    let outcome = match document_kind {
        ZippedXmlDocumentKind::WordDocument => {
            extract_word_document(&mut archive, max_output_bytes)
        }
        ZippedXmlDocumentKind::Spreadsheet => extract_spreadsheet(&mut archive, max_output_bytes),
        ZippedXmlDocumentKind::Presentation => extract_presentation(&mut archive, max_output_bytes),
        ZippedXmlDocumentKind::OpenDocumentText => {
            extract_open_document_text(&mut archive, max_output_bytes)
        }
    };
    Ok(outcome)
}

/// docx：正文在 word/document.xml，`w:t` 是文本，`w:p` 结束即段落换行。
fn extract_word_document(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    max_output_bytes: u64,
) -> ExtractionOutcome {
    let entry_outcome = read_required_entry(
        archive,
        "word/document.xml",
        max_output_bytes,
        |entry, text| {
            read_tagged_text_document(&mut Reader::from_reader(BufReader::new(entry)), text)
        },
    );
    finish_extraction(entry_outcome)
}

/// xlsx：先读共享字符串表，再按顺序读全部 worksheet。
/// 单元格按行拼接、以制表符分隔，保证相邻单元格文本不会拼成同一个词。
fn extract_spreadsheet(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    max_output_bytes: u64,
) -> ExtractionOutcome {
    let shared_strings = match read_shared_strings(archive, max_output_bytes) {
        Ok(Some(shared_strings)) => shared_strings,
        Ok(None) => return ExtractionOutcome::skipped(ExtractionStatus::TooLarge),
        Err(failure) => return xml_failure_outcome(failure),
    };

    let mut sheet_names = zip_entry_names(archive, "xl/worksheets/", ".xml");
    sort_entries_numerically(&mut sheet_names);
    let mut text = BoundedText::new(max_output_bytes);
    for sheet_name in sheet_names {
        let entry = match archive.by_name(&sheet_name) {
            Ok(entry) => entry,
            Err(source) => return xml_failure_outcome(xml_entry_error(&sheet_name, source)),
        };
        let mut reader = Reader::from_reader(BufReader::new(entry));
        if let Err(source) = read_worksheet(&mut reader, &mut text, &shared_strings) {
            return xml_failure_outcome(source);
        }
    }
    text.outcome()
}

/// pptx：幻灯片正文与演讲者备注都进索引；按文件名里的数字排序，
/// 让 slide10 排在 slide2 之后，正文顺序与演示顺序一致。
fn extract_presentation(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    max_output_bytes: u64,
) -> ExtractionOutcome {
    let mut slide_names = zip_entry_names(archive, "ppt/slides/slide", ".xml");
    let mut notes_names = zip_entry_names(archive, "ppt/notesSlides/notesSlide", ".xml");
    sort_entries_numerically(&mut slide_names);
    sort_entries_numerically(&mut notes_names);

    let mut text = BoundedText::new(max_output_bytes);
    for entry_name in slide_names.into_iter().chain(notes_names) {
        let entry = match archive.by_name(&entry_name) {
            Ok(entry) => entry,
            Err(source) => return xml_failure_outcome(xml_entry_error(&entry_name, source)),
        };
        let mut reader = Reader::from_reader(BufReader::new(entry));
        if let Err(source) = read_tagged_text_document(&mut reader, &mut text) {
            return xml_failure_outcome(source);
        }
    }
    text.outcome()
}

/// odt：正文在 content.xml，`text:p`/`text:h` 是块级文本，结束即换行。
fn extract_open_document_text(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    max_output_bytes: u64,
) -> ExtractionOutcome {
    let entry_outcome =
        read_required_entry(archive, "content.xml", max_output_bytes, |entry, text| {
            read_block_text_document(&mut Reader::from_reader(BufReader::new(entry)), text)
        });
    finish_extraction(entry_outcome)
}

fn finish_extraction(entry_outcome: Result<BoundedText, String>) -> ExtractionOutcome {
    match entry_outcome {
        Ok(text) => text.outcome(),
        Err(failure) => xml_failure_outcome(failure),
    }
}

/// 打开一个必需的 zip 条目并把它交给对应的 XML 解析器；条目缺失或解析失败
/// 都折算成 String 诊断，由调用方统一转成 Skipped。
fn read_required_entry(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    entry_name: &str,
    max_output_bytes: u64,
    parse: impl FnOnce(
        zip::read::ZipFile<'_, BufReader<fs::File>>,
        &mut BoundedText,
    ) -> Result<(), quick_xml::Error>,
) -> Result<BoundedText, String> {
    let entry = archive
        .by_name(entry_name)
        .map_err(|source| xml_entry_error(entry_name, source))?;
    let mut text = BoundedText::new(max_output_bytes);
    parse(entry, &mut text).map_err(|source| source.to_string())?;
    Ok(text)
}

fn xml_entry_error(entry_name: &str, source: zip::result::ZipError) -> String {
    format!("missing required office entry {entry_name}: {source}")
}

fn zip_entry_names(
    archive: &ZipArchive<BufReader<fs::File>>,
    prefix: &str,
    suffix: &str,
) -> Vec<String> {
    archive
        .file_names()
        .filter(|name| name.starts_with(prefix) && name.ends_with(suffix))
        .map(|name| name.to_owned())
        .collect()
}

/// `slide10.xml` 必须排在 `slide2.xml` 之后；按结尾数字排序，非数字名称退化为字典序。
fn sort_entries_numerically(entry_names: &mut [String]) {
    entry_names.sort_by(|left, right| {
        let left_number = trailing_number(left);
        let right_number = trailing_number(right);
        left_number.cmp(&right_number).then_with(|| left.cmp(right))
    });
}

fn trailing_number(entry_name: &str) -> Option<u64> {
    let stem = entry_name.strip_suffix(".xml")?;
    let digits = stem
        .rsplit(|character: char| !character.is_ascii_digit())
        .next()?;
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// 捕获 `t` 元素内的文本（docx 的 `w:t`、pptx 的 `a:t`），`p` 元素结束时换行。
/// 两者的命名空间前缀不同但本地名相同，所以一个解析器可以同时服务两种格式。
fn read_tagged_text_document(
    reader: &mut Reader<impl std::io::BufRead>,
    text: &mut BoundedText,
) -> Result<(), quick_xml::Error> {
    let mut capturing_text = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                if element.local_name().as_ref() == b"t" {
                    capturing_text = true;
                }
            }
            Event::End(element) => match element.local_name().as_ref() {
                b"t" => capturing_text = false,
                b"p" => text.end_paragraph(),
                _ => {}
            },
            Event::Text(text_node) => {
                if capturing_text {
                    text.push_str(&text_node.xml_content()?);
                }
            }
            Event::CData(text_node) => {
                if capturing_text {
                    text.push_str(&text_node.xml_content()?);
                }
            }
            Event::GeneralRef(reference) => {
                if capturing_text {
                    text.push_str(&resolved_reference(&reference));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buffer.clear();
    }
}

/// odt：捕获 `p`/`h` 块级元素内的全部文本节点（含内联 span 的文字）。
fn read_block_text_document(
    reader: &mut Reader<impl std::io::BufRead>,
    text: &mut BoundedText,
) -> Result<(), quick_xml::Error> {
    let mut paragraph_depth = 0_usize;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                if matches!(element.local_name().as_ref(), b"p" | b"h") {
                    paragraph_depth += 1;
                }
            }
            Event::End(element) => {
                if matches!(element.local_name().as_ref(), b"p" | b"h") {
                    paragraph_depth = paragraph_depth.saturating_sub(1);
                    text.end_paragraph();
                }
            }
            Event::Text(text_node) => {
                if paragraph_depth > 0 {
                    text.push_str(&text_node.xml_content()?);
                }
            }
            Event::CData(text_node) => {
                if paragraph_depth > 0 {
                    text.push_str(&text_node.xml_content()?);
                }
            }
            Event::GeneralRef(reference) => {
                if paragraph_depth > 0 {
                    text.push_str(&resolved_reference(&reference));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buffer.clear();
    }
}

/// 把 `&amp;`/`&#38;`/`&#x26;` 这类实体引用还原成字符。
/// 无法识别的引用（自定义 DTD 实体）直接丢弃——它们在 Office 文档里不存在，
/// 丢弃比让整个文档降级成 ReadFailed 更符合索引用途。
fn resolved_reference(reference: &BytesRef<'_>) -> String {
    let reference_text = reference
        .xml_content()
        .map(|text| text.into_owned())
        .unwrap_or_default();
    if reference.is_char_ref() {
        let code_point = if let Some(hex_digits) = reference_text.strip_prefix("#x") {
            u32::from_str_radix(hex_digits, 16).ok()
        } else if let Some(decimal_digits) = reference_text.strip_prefix('#') {
            decimal_digits.parse::<u32>().ok()
        } else {
            None
        };
        return code_point
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default();
    }
    quick_xml::escape::resolve_xml_entity(&reference_text)
        .map(str::to_owned)
        .unwrap_or_default()
}

/// 读取 xlsx 共享字符串表；`None` 表示表格超出输出预算，整个文件按 TooLarge 处理。
/// 共享字符串表不存在时返回空表（只用了内联字符串的工作簿是合法的）。
fn read_shared_strings(
    archive: &mut ZipArchive<BufReader<fs::File>>,
    max_output_bytes: u64,
) -> Result<Option<Vec<String>>, String> {
    let entry = match archive.by_name("xl/sharedStrings.xml") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(Some(Vec::new())),
        Err(source) => return Err(xml_entry_error("xl/sharedStrings.xml", source)),
    };
    let mut reader = Reader::from_reader(BufReader::new(entry));
    let mut strings = Vec::new();
    let mut total_bytes = 0_u64;
    let mut current = String::new();
    let mut inside_item = false;
    let mut budget_exceeded = false;
    let mut buffer = Vec::new();
    let read_result = (|| -> Result<(), quick_xml::Error> {
        loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(element) => {
                    if element.local_name().as_ref() == b"si" {
                        inside_item = true;
                        current.clear();
                    }
                }
                Event::End(element) => {
                    if element.local_name().as_ref() == b"si" {
                        inside_item = false;
                        total_bytes += current.len() as u64;
                        if total_bytes > max_output_bytes
                            || strings.len() >= MAX_SHARED_STRING_ENTRIES
                        {
                            budget_exceeded = true;
                            return Ok(());
                        }
                        strings.push(current.clone());
                    }
                }
                Event::Text(text_node) => {
                    if inside_item {
                        current.push_str(&text_node.xml_content()?);
                    }
                }
                Event::CData(text_node) => {
                    if inside_item {
                        current.push_str(&text_node.xml_content()?);
                    }
                }
                Event::GeneralRef(reference) => {
                    if inside_item {
                        current.push_str(&resolved_reference(&reference));
                    }
                }
                Event::Eof => return Ok(()),
                _ => {}
            }
            buffer.clear();
        }
    })();
    match read_result {
        Ok(()) if budget_exceeded => Ok(None),
        Ok(()) => Ok(Some(strings)),
        Err(source) => Err(source.to_string()),
    }
}

/// worksheet 单元格文本来源：共享字符串索引、内联字符串，或直接取 `v` 的
/// 数字/公式缓存值。共享索引必须解析回真实字符串，绝不能把索引数字当正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorksheetCellKind {
    SharedIndex,
    Inline,
    Literal,
}

fn read_worksheet(
    reader: &mut Reader<impl std::io::BufRead>,
    text: &mut BoundedText,
    shared_strings: &[String],
) -> Result<(), quick_xml::Error> {
    let mut cell_kind = WorksheetCellKind::Literal;
    let mut value_buffer = String::new();
    let mut inline_buffer = String::new();
    let mut capturing_value = false;
    let mut capturing_inline = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => match element.local_name().as_ref() {
                b"c" => {
                    cell_kind = worksheet_cell_kind(&element, reader.decoder())?;
                    value_buffer.clear();
                    inline_buffer.clear();
                }
                b"v" => capturing_value = true,
                b"is" => capturing_inline = true,
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                b"c" => {
                    let cell_text: Option<&str> = match cell_kind {
                        WorksheetCellKind::SharedIndex => value_buffer
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| shared_strings.get(index))
                            .map(String::as_str),
                        WorksheetCellKind::Inline => Some(inline_buffer.as_str()),
                        WorksheetCellKind::Literal => Some(value_buffer.trim()),
                    };
                    if let Some(cell_text) = cell_text {
                        if !cell_text.is_empty() {
                            text.push_str(cell_text);
                            text.push_str("\t");
                        }
                    }
                }
                b"v" => capturing_value = false,
                b"is" => capturing_inline = false,
                b"row" => text.end_paragraph(),
                _ => {}
            },
            Event::Text(text_node) => {
                if capturing_value {
                    value_buffer.push_str(&text_node.xml_content()?);
                }
                if capturing_inline {
                    inline_buffer.push_str(&text_node.xml_content()?);
                }
            }
            Event::CData(text_node) => {
                if capturing_value {
                    value_buffer.push_str(&text_node.xml_content()?);
                }
                if capturing_inline {
                    inline_buffer.push_str(&text_node.xml_content()?);
                }
            }
            Event::GeneralRef(reference) => {
                if capturing_value {
                    value_buffer.push_str(&resolved_reference(&reference));
                }
                if capturing_inline {
                    inline_buffer.push_str(&resolved_reference(&reference));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buffer.clear();
    }
}

fn worksheet_cell_kind(
    cell: &BytesStart<'_>,
    decoder: Decoder,
) -> Result<WorksheetCellKind, quick_xml::Error> {
    let mut cell_kind = WorksheetCellKind::Literal;
    for attribute in cell.attributes().with_checks(false) {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == b"t" {
            cell_kind = match attribute.decode_and_unescape_value(decoder)?.as_ref() {
                "s" => WorksheetCellKind::SharedIndex,
                "inlineStr" => WorksheetCellKind::Inline,
                _ => WorksheetCellKind::Literal,
            };
        }
    }
    Ok(cell_kind)
}

#[cfg(test)]
#[path = "zip_xml_text/tests.rs"]
mod tests;
