//! Markdown 渲染模式的内容：打开文件时在后台解析一次（≤ 2 MB），文档里留着，画的时候
//! 直接借用，不再每帧重新解析。
//!
//! 行内格式（粗体、斜体、代码、删除线、链接）按样式段记下；表格保留列对齐与每个单元格；
//! 围栏代码块按标注的语言高亮。能搜的文字按「单元」排好：一段正文、一个代码块、一个表格
//! 单元格各是一个单元，搜索只在单元内匹配，不匹配 Markdown 标记符号。

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::text_highlight::{language_for_fence, HighlightCheckpoint, LineHighlighter, TextToken};

/// 行内样式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

/// 带样式的一段文字：`runs` 首尾相接盖住整个 `text`（每段记终点）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub runs: Vec<(usize, InlineStyle)>,
}

impl Inline {
    fn push(&mut self, text: &str, style: InlineStyle) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        let end = self.text.len();
        match self.runs.last_mut() {
            Some((last_end, last_style)) if *last_style == style => *last_end = end,
            _ => self.runs.push((end, style)),
        }
    }

    /// 去掉首尾空白（样式段跟着移）；全是空白时 None。
    fn trimmed(self) -> Option<Self> {
        let start = self.text.len() - self.text.trim_start().len();
        let end = self.text.trim_end().len();
        if start >= end {
            return None;
        }
        let runs = self
            .runs
            .into_iter()
            .filter(|(run_end, _)| *run_end > start)
            .map(|(run_end, style)| (run_end.min(end) - start, style))
            .fold(Vec::new(), |mut runs: Vec<(usize, InlineStyle)>, run| {
                if runs.last().is_none_or(|last| last.0 < run.0) {
                    runs.push(run);
                }
                runs
            });
        Some(Self {
            text: self.text[start..end].to_owned(),
            runs,
        })
    }

    /// 按样式切开：（字节范围，样式）。
    pub fn styled_ranges(&self) -> impl Iterator<Item = (Range<usize>, InlineStyle)> + '_ {
        let mut start = 0;
        self.runs.iter().map(move |(end, style)| {
            let range = start..*end;
            start = *end;
            (range, *style)
        })
    }
}

/// 列表项前面的符号；待办项画成勾选框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    Bullet,
    Ordered(u64),
    Task { done: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextBlockKind {
    Paragraph,
    Heading(HeadingLevel),
    ListItem { depth: usize },
    Quote { depth: usize },
}

/// 一个能搜的单元：正文段、代码块或表格单元格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownUnit {
    pub unit: usize,
    pub inline: Inline,
}

/// 代码块的一行：在代码块文字里的范围与高亮记号（行内下标）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLine {
    pub range: Range<usize>,
    pub tokens: Vec<TextToken>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownBlock {
    Text {
        kind: TextBlockKind,
        /// 列表项的符号，不算能搜的文字。
        marker: Option<ListMarker>,
        content: MarkdownUnit,
    },
    Code {
        language: Option<String>,
        /// 文字在单元里；`lines` 是逐行的高亮（认不出语言时没有记号）。
        content: MarkdownUnit,
        lines: Vec<CodeLine>,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Vec<MarkdownUnit>,
        rows: Vec<Vec<MarkdownUnit>>,
    },
    Rule,
}

#[derive(Debug, Default, PartialEq)]
pub struct RenderedMarkdown {
    blocks: Vec<MarkdownBlock>,
    /// 单元 → 所在块的下标（搜索跳过去时滚到那一块）。
    unit_blocks: Vec<usize>,
}

impl RenderedMarkdown {
    pub fn parse(markdown: &str) -> Self {
        let mut collector = Collector::default();
        for event in Parser::new_ext(markdown, markdown_options()) {
            collector.push_event(event);
        }
        collector.finish()
    }

    pub fn blocks(&self) -> &[MarkdownBlock] {
        &self.blocks
    }

    pub fn block_of_unit(&self, unit: usize) -> Option<usize> {
        self.unit_blocks.get(unit).copied()
    }

    /// 所有能搜的单元，按出现顺序（单元号递增）。
    pub fn units(&self) -> impl Iterator<Item = &MarkdownUnit> {
        self.blocks
            .iter()
            .flat_map(|block| -> Box<dyn Iterator<Item = &MarkdownUnit>> {
                match block {
                    MarkdownBlock::Text { content, .. } | MarkdownBlock::Code { content, .. } => {
                        Box::new(std::iter::once(content))
                    }
                    MarkdownBlock::Table { header, rows, .. } => {
                        Box::new(header.iter().chain(rows.iter().flatten()))
                    }
                    MarkdownBlock::Rule => Box::new(std::iter::empty()),
                }
            })
    }
}

fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_SUBSCRIPT);
    options.insert(Options::ENABLE_SUPERSCRIPT);
    options
}

/// 正在收集的块。
enum Active {
    Text {
        kind: TextBlockKind,
        marker: Option<ListMarker>,
        inline: Inline,
    },
    Code {
        language: Option<String>,
        text: String,
    },
}

#[derive(Default)]
struct Collector {
    blocks: Vec<MarkdownBlock>,
    unit_blocks: Vec<usize>,
    active: Option<Active>,
    /// 每层列表下一个序号（无序列表为 None）。
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    style: InlineStyle,
    /// 嵌套的强调层数：结束时减一，减到 0 才去掉样式。
    bold_depth: usize,
    italic_depth: usize,
    strike_depth: usize,
    link_depth: usize,
    table: Option<TableBuilder>,
}

#[derive(Default)]
struct TableBuilder {
    alignments: Vec<Alignment>,
    header: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    row: Vec<Inline>,
    cell: Option<Inline>,
    in_head: bool,
}

impl Collector {
    fn push_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.append(&text),
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                let style = self.style;
                self.append_styled(
                    &code,
                    InlineStyle {
                        code: true,
                        ..style
                    },
                );
            }
            Event::SoftBreak => self.append_break(" "),
            Event::HardBreak => self.append_break("\n"),
            Event::Rule => {
                self.finish_active();
                self.blocks.push(MarkdownBlock::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Active::Text { marker, .. }) = &mut self.active {
                    *marker = Some(ListMarker::Task { done: checked });
                }
            }
            Event::FootnoteReference(label) => self.append(&format!("[{label}]")),
            Event::Html(html) | Event::InlineHtml(html) => self.append(&html),
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_text(TextBlockKind::Heading(level), None),
            Tag::BlockQuote(_) => {
                self.finish_active();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.finish_active();
                self.active = Some(Active::Code {
                    language: match kind {
                        CodeBlockKind::Fenced(label) => {
                            let label = label.trim();
                            (!label.is_empty()).then(|| label.to_owned())
                        }
                        CodeBlockKind::Indented => None,
                    },
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                self.finish_active();
                self.lists.push(start);
            }
            Tag::Item => {
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        ListMarker::Ordered(*number - 1)
                    }
                    _ => ListMarker::Bullet,
                };
                self.start_text(TextBlockKind::ListItem { depth }, Some(marker));
            }
            Tag::Emphasis => {
                self.italic_depth += 1;
                self.style.italic = true;
            }
            Tag::Strong => {
                self.bold_depth += 1;
                self.style.bold = true;
            }
            Tag::Strikethrough => {
                self.strike_depth += 1;
                self.style.strike = true;
            }
            Tag::Link { .. } => {
                self.link_depth += 1;
                self.style.link = true;
            }
            Tag::Image { .. } => self.append("🖼 "),
            Tag::Table(alignments) => {
                self.finish_active();
                self.table = Some(TableBuilder {
                    alignments,
                    ..TableBuilder::default()
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = true;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.cell = Some(Inline::default());
                }
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => self.finish_active(),
            Tag::HtmlBlock
            | Tag::Superscript
            | Tag::Subscript
            | Tag::DefinitionList
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if matches!(
                    self.active,
                    Some(Active::Text {
                        kind: TextBlockKind::Paragraph | TextBlockKind::Quote { .. },
                        ..
                    })
                ) {
                    self.finish_active();
                }
            }
            TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => self.finish_active(),
            TagEnd::BlockQuote(_) => {
                self.finish_active();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.finish_active();
                self.lists.pop();
            }
            TagEnd::Emphasis => {
                self.italic_depth = self.italic_depth.saturating_sub(1);
                self.style.italic = self.italic_depth > 0;
            }
            TagEnd::Strong => {
                self.bold_depth = self.bold_depth.saturating_sub(1);
                self.style.bold = self.bold_depth > 0;
            }
            TagEnd::Strikethrough => {
                self.strike_depth = self.strike_depth.saturating_sub(1);
                self.style.strike = self.strike_depth > 0;
            }
            TagEnd::Link => {
                self.link_depth = self.link_depth.saturating_sub(1);
                self.style.link = self.link_depth > 0;
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = false;
                    table.header = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    let cell = table.cell.take().unwrap_or_default();
                    table.row.push(cell);
                }
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => self.finish_active(),
            TagEnd::Image
            | TagEnd::HtmlBlock
            | TagEnd::DefinitionList
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    /// 列表项里的段落接在列表项上；引用里的段落是引用块。
    fn start_paragraph(&mut self) {
        if matches!(
            self.active,
            Some(Active::Text {
                kind: TextBlockKind::ListItem { .. },
                ..
            })
        ) {
            return;
        }
        let kind = if self.quote_depth > 0 {
            TextBlockKind::Quote {
                depth: self.quote_depth,
            }
        } else {
            TextBlockKind::Paragraph
        };
        self.start_text(kind, None);
    }

    fn start_text(&mut self, kind: TextBlockKind, marker: Option<ListMarker>) {
        self.finish_active();
        self.active = Some(Active::Text {
            kind,
            marker,
            inline: Inline::default(),
        });
    }

    fn append(&mut self, text: &str) {
        let style = self.style;
        self.append_styled(text, style);
    }

    fn append_styled(&mut self, text: &str, style: InlineStyle) {
        if let Some(cell) = self.table.as_mut().and_then(|table| table.cell.as_mut()) {
            cell.push(text, style);
            return;
        }
        if self.active.is_none() {
            self.start_paragraph();
        }
        match &mut self.active {
            Some(Active::Text { inline, .. }) => inline.push(text, style),
            Some(Active::Code { text: code, .. }) => code.push_str(text),
            None => {}
        }
    }

    fn append_break(&mut self, text: &str) {
        match &mut self.active {
            Some(Active::Code { text: code, .. }) => code.push('\n'),
            _ => self.append(text),
        }
    }

    fn next_unit(&mut self) -> usize {
        self.unit_blocks.push(self.blocks.len());
        self.unit_blocks.len() - 1
    }

    fn finish_active(&mut self) {
        let block = match self.active.take() {
            Some(Active::Text {
                kind,
                marker,
                inline,
            }) => {
                let Some(inline) = inline.trimmed() else {
                    return;
                };
                MarkdownBlock::Text {
                    kind,
                    marker,
                    content: MarkdownUnit {
                        unit: self.next_unit(),
                        inline,
                    },
                }
            }
            Some(Active::Code { language, text }) => {
                let text = text.trim_end().to_owned();
                let lines = highlighted_lines(language.as_deref(), &text);
                let mut inline = Inline::default();
                inline.push(
                    &text,
                    InlineStyle {
                        code: true,
                        ..InlineStyle::default()
                    },
                );
                MarkdownBlock::Code {
                    language,
                    content: MarkdownUnit {
                        unit: self.next_unit(),
                        inline,
                    },
                    lines,
                }
            }
            None => return,
        };
        self.blocks.push(block);
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let mut unit = |inline: Inline| MarkdownUnit {
            unit: self.next_unit(),
            inline: inline.trimmed().unwrap_or_default(),
        };
        let header = table.header.into_iter().map(&mut unit).collect();
        let rows = table
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(&mut unit).collect())
            .collect();
        self.blocks.push(MarkdownBlock::Table {
            alignments: table.alignments,
            header,
            rows,
        });
    }

    fn finish(mut self) -> RenderedMarkdown {
        self.finish_active();
        self.finish_table();
        RenderedMarkdown {
            blocks: self.blocks,
            unit_blocks: self.unit_blocks,
        }
    }
}

/// 代码块逐行高亮：标注的语言认不出就只分行、没有记号。
fn highlighted_lines(language: Option<&str>, text: &str) -> Vec<CodeLine> {
    let mut highlighter = language
        .and_then(language_for_fence)
        .map(|language| LineHighlighter::resume(&HighlightCheckpoint::at_start(&language, 0)));
    let mut start = 0;
    text.split('\n')
        .map(|line| {
            let range = start..start + line.len();
            start = range.end + 1;
            let tokens = highlighter
                .as_mut()
                .map_or_else(Vec::new, |highlighter| highlighter.line(line));
            CodeLine { range, tokens }
        })
        .collect()
}

#[cfg(test)]
#[path = "markdown_render_tests.rs"]
mod tests;
