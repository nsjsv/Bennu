use iced::widget::{container, row, scrollable, Column, Space};
use iced::{Alignment, Element, Font, Length};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};
use crate::model::{
    Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, TextPreviewLineLimitNotice,
};
use crate::typography::{localized_text, readable_text};

const MARKDOWN_BODY_TEXT_SIZE: u32 = 14;
const MARKDOWN_CODE_TEXT_SIZE: u32 = 13;
const MARKDOWN_BLOCK_SPACING: f32 = 10.0;
const MARKDOWN_LIST_INDENT: f32 = 20.0;
const MARKDOWN_MARKER_WIDTH: f32 = 22.0;
const MARKDOWN_SCROLLBAR_WIDTH: f32 = 6.0;

pub(super) fn markdown_preview_body(
    markdown: &str,
    line_limit_notice: Option<TextPreviewLineLimitNotice>,
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'static, Message> {
    let blocks = markdown_preview_blocks(markdown);
    let mut content = Column::new()
        .spacing(MARKDOWN_BLOCK_SPACING)
        .width(Length::Fill);

    if blocks.is_empty() {
        content = content.push(readable_text("(empty file)").size(MARKDOWN_BODY_TEXT_SIZE));
    } else {
        for block in blocks {
            content = content.push(markdown_block_view(block));
        }
    }
    if let Some(notice) = line_limit_notice {
        content = content.push(localized_text(notice.label()).size(12));
    }

    let scroll_region = ScrollbarRegion::MarkdownPreview;
    let scroller = scrollable(smooth_scroll_content(content, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            MARKDOWN_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(scroll_height))
        .width(Length::Fill)
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |viewport| {
            let offset = viewport.absolute_offset();
            let bounds = viewport.bounds();
            let content_bounds = viewport.content_bounds();
            Message::MarkdownPreviewScrolled {
                offset_y: offset.y,
                viewport_height: bounds.height,
                content_height: content_bounds.height,
            }
        }));

    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        MARKDOWN_SCROLLBAR_WIDTH,
    )
}

fn markdown_block_view(block: MarkdownBlock) -> Element<'static, Message> {
    match block {
        MarkdownBlock::Paragraph(content) => readable_text(content)
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fill)
            .into(),
        MarkdownBlock::Heading { level, content } => readable_text(content)
            .size(markdown_heading_size(level))
            .width(Length::Fill)
            .into(),
        MarkdownBlock::ListItem {
            depth,
            marker,
            checked,
            content,
        } => markdown_list_item_view(depth, marker, checked, content),
        MarkdownBlock::CodeBlock { language, content } => {
            markdown_code_block_view(language, content)
        }
        MarkdownBlock::Quote { depth, content } => markdown_quote_view(depth, content),
        MarkdownBlock::Rule => readable_text("-----")
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fill)
            .into(),
        MarkdownBlock::TableRow(cells) => readable_text(cells.join("    |    "))
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fill)
            .into(),
    }
}

fn markdown_list_item_view(
    depth: usize,
    marker: MarkdownListMarker,
    checked: Option<bool>,
    content: String,
) -> Element<'static, Message> {
    row![
        Space::new().width(Length::Fixed(depth as f32 * MARKDOWN_LIST_INDENT)),
        readable_text(list_marker_label(marker, checked))
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fixed(MARKDOWN_MARKER_WIDTH)),
        readable_text(content)
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Start)
    .into()
}

fn markdown_code_block_view(
    language: Option<String>,
    content: String,
) -> Element<'static, Message> {
    let mut code = Column::new().spacing(4).width(Length::Fill);
    if let Some(language) = language.filter(|language| !language.is_empty()) {
        code = code.push(
            readable_text(language)
                .font(Font::MONOSPACE)
                .size(MARKDOWN_CODE_TEXT_SIZE),
        );
    }
    code = code.push(
        readable_text(content)
            .font(Font::MONOSPACE)
            .size(MARKDOWN_CODE_TEXT_SIZE)
            .width(Length::Fill),
    );

    container(code).padding(8).width(Length::Fill).into()
}

fn markdown_quote_view(depth: usize, content: String) -> Element<'static, Message> {
    row![
        Space::new().width(Length::Fixed(
            depth.saturating_sub(1) as f32 * MARKDOWN_LIST_INDENT
        )),
        readable_text(">").size(MARKDOWN_BODY_TEXT_SIZE),
        readable_text(content)
            .size(MARKDOWN_BODY_TEXT_SIZE)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Start)
    .into()
}

fn markdown_heading_size(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 28,
        HeadingLevel::H2 => 24,
        HeadingLevel::H3 => 20,
        HeadingLevel::H4 => 18,
        HeadingLevel::H5 => 16,
        HeadingLevel::H6 => 14,
    }
}

fn list_marker_label(marker: MarkdownListMarker, checked: Option<bool>) -> String {
    match checked {
        Some(true) => "☑".to_owned(),
        Some(false) => "☐".to_owned(),
        None => match marker {
            MarkdownListMarker::Bullet => "•".to_owned(),
            MarkdownListMarker::Ordered(number) => format!("{number}."),
        },
    }
}

fn markdown_preview_blocks(markdown: &str) -> Vec<MarkdownBlock> {
    let mut collector = MarkdownPreviewCollector::default();
    for event in Parser::new_ext(markdown, markdown_options()) {
        collector.push_event(event);
    }
    collector.finish()
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

#[derive(Default)]
struct MarkdownPreviewCollector {
    blocks: Vec<MarkdownBlock>,
    active_block: Option<ActiveMarkdownBlock>,
    list_stack: Vec<MarkdownListState>,
    quote_depth: usize,
    link_destinations: Vec<String>,
    image_destinations: Vec<String>,
    table_row: Option<Vec<String>>,
    table_cell: Option<String>,
}

impl MarkdownPreviewCollector {
    fn push_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.append_text(&text),
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                self.append_text(&code)
            }
            Event::SoftBreak => self.append_soft_break(),
            Event::HardBreak => self.append_hard_break(),
            Event::Rule => {
                self.finish_active_block();
                self.blocks.push(MarkdownBlock::Rule);
            }
            Event::TaskListMarker(checked) => self.set_active_task_state(checked),
            Event::FootnoteReference(label) => self.append_text(&format!("[{label}]")),
            Event::Html(html) | Event::InlineHtml(html) => self.append_text(&html),
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => {
                self.finish_active_block();
                self.active_block = Some(ActiveMarkdownBlock::Heading {
                    level,
                    content: String::new(),
                });
            }
            Tag::BlockQuote(_) => {
                self.finish_active_block();
                self.quote_depth = self.quote_depth.saturating_add(1);
            }
            Tag::CodeBlock(kind) => {
                self.finish_active_block();
                self.active_block = Some(ActiveMarkdownBlock::CodeBlock {
                    language: code_block_language(kind),
                    content: String::new(),
                });
            }
            Tag::List(start) => {
                self.finish_active_block();
                self.list_stack
                    .push(MarkdownListState { next_number: start });
            }
            Tag::Item => self.start_list_item(),
            Tag::Link { dest_url, .. } => self.link_destinations.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => {
                self.image_destinations.push(dest_url.to_string());
                self.append_text("Image: ");
            }
            Tag::Table(_) => self.finish_active_block(),
            Tag::TableRow => {
                self.finish_active_block();
                self.table_row = Some(Vec::new());
            }
            Tag::TableHead => {
                self.finish_active_block();
                self.table_row = Some(Vec::new());
            }
            Tag::TableCell => self.table_cell = Some(String::new()),
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => self.finish_active_block(),
            Tag::HtmlBlock
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::DefinitionList
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.finish_paragraph(),
            TagEnd::Heading(_) => self.finish_active_block(),
            TagEnd::BlockQuote(_) => {
                self.finish_active_block();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.finish_active_block(),
            TagEnd::List(_) => {
                self.finish_active_block();
                self.list_stack.pop();
            }
            TagEnd::Item => self.finish_active_block(),
            TagEnd::Link => self.finish_link(),
            TagEnd::Image => self.finish_image(),
            TagEnd::Table => self.finish_table_row(),
            TagEnd::TableHead => self.finish_table_row(),
            TagEnd::TableRow => self.finish_table_row(),
            TagEnd::TableCell => self.finish_table_cell(),
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => self.finish_active_block(),
            TagEnd::HtmlBlock
            | TagEnd::DefinitionList
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.table_cell.is_some()
            || matches!(
                self.active_block,
                Some(ActiveMarkdownBlock::ListItem { .. })
            )
        {
            return;
        }

        self.finish_active_block();
        self.active_block = Some(if self.quote_depth > 0 {
            ActiveMarkdownBlock::Quote {
                depth: self.quote_depth,
                content: String::new(),
            }
        } else {
            ActiveMarkdownBlock::Paragraph(String::new())
        });
    }

    fn finish_paragraph(&mut self) {
        if matches!(
            self.active_block,
            Some(ActiveMarkdownBlock::Paragraph(_) | ActiveMarkdownBlock::Quote { .. })
        ) {
            self.finish_active_block();
        }
    }

    fn start_list_item(&mut self) {
        self.finish_active_block();
        let depth = self.list_stack.len().saturating_sub(1);
        let marker = self.next_list_marker();
        self.active_block = Some(ActiveMarkdownBlock::ListItem {
            depth,
            marker,
            checked: None,
            content: String::new(),
        });
    }

    fn next_list_marker(&mut self) -> MarkdownListMarker {
        let Some(state) = self.list_stack.last_mut() else {
            return MarkdownListMarker::Bullet;
        };

        match state.next_number {
            Some(number) => {
                state.next_number = Some(number.saturating_add(1));
                MarkdownListMarker::Ordered(number)
            }
            None => MarkdownListMarker::Bullet,
        }
    }

    fn append_text(&mut self, text: &str) {
        if let Some(cell) = self.table_cell.as_mut() {
            cell.push_str(text);
            return;
        }

        if self.active_block.is_none() {
            self.start_paragraph();
        }
        if let Some(block) = self.active_block.as_mut() {
            block.push_text(text);
        }
    }

    fn append_soft_break(&mut self) {
        if let Some(cell) = self.table_cell.as_mut() {
            cell.push(' ');
            return;
        }
        if let Some(block) = self.active_block.as_mut() {
            block.push_soft_break();
        }
    }

    fn append_hard_break(&mut self) {
        if let Some(cell) = self.table_cell.as_mut() {
            cell.push('\n');
            return;
        }
        if let Some(block) = self.active_block.as_mut() {
            block.push_hard_break();
        }
    }

    fn set_active_task_state(&mut self, checked: bool) {
        if let Some(ActiveMarkdownBlock::ListItem { checked: state, .. }) =
            self.active_block.as_mut()
        {
            *state = Some(checked);
        }
    }

    fn finish_link(&mut self) {
        let Some(destination) = self.link_destinations.pop() else {
            return;
        };
        if !destination.is_empty() {
            self.append_text(&format!(" ({destination})"));
        }
    }

    fn finish_image(&mut self) {
        let Some(destination) = self.image_destinations.pop() else {
            return;
        };
        if !destination.is_empty() {
            self.append_text(&format!(" ({destination})"));
        }
    }

    fn finish_table_cell(&mut self) {
        let Some(cell) = self.table_cell.take() else {
            return;
        };
        let Some(row) = self.table_row.as_mut() else {
            return;
        };
        row.push(cell.trim().to_owned());
    }

    fn finish_table_row(&mut self) {
        self.finish_table_cell();
        let Some(cells) = self.table_row.take() else {
            return;
        };
        if cells.iter().any(|cell| !cell.is_empty()) {
            self.blocks.push(MarkdownBlock::TableRow(cells));
        }
    }

    fn finish_active_block(&mut self) {
        let Some(block) = self.active_block.take() else {
            return;
        };
        if let Some(block) = block.finish() {
            self.blocks.push(block);
        }
    }

    fn finish(mut self) -> Vec<MarkdownBlock> {
        self.finish_active_block();
        self.finish_table_row();
        self.blocks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownBlock {
    Paragraph(String),
    Heading {
        level: HeadingLevel,
        content: String,
    },
    ListItem {
        depth: usize,
        marker: MarkdownListMarker,
        checked: Option<bool>,
        content: String,
    },
    CodeBlock {
        language: Option<String>,
        content: String,
    },
    Quote {
        depth: usize,
        content: String,
    },
    Rule,
    TableRow(Vec<String>),
}

enum ActiveMarkdownBlock {
    Paragraph(String),
    Heading {
        level: HeadingLevel,
        content: String,
    },
    ListItem {
        depth: usize,
        marker: MarkdownListMarker,
        checked: Option<bool>,
        content: String,
    },
    CodeBlock {
        language: Option<String>,
        content: String,
    },
    Quote {
        depth: usize,
        content: String,
    },
}

impl ActiveMarkdownBlock {
    fn push_text(&mut self, text: &str) {
        self.content_mut().push_str(text);
    }

    fn push_soft_break(&mut self) {
        match self {
            Self::CodeBlock { content, .. } => content.push('\n'),
            _ => self.content_mut().push(' '),
        }
    }

    fn push_hard_break(&mut self) {
        self.content_mut().push('\n');
    }

    fn content_mut(&mut self) -> &mut String {
        match self {
            Self::Paragraph(content)
            | Self::Heading { content, .. }
            | Self::ListItem { content, .. }
            | Self::CodeBlock { content, .. }
            | Self::Quote { content, .. } => content,
        }
    }

    fn finish(self) -> Option<MarkdownBlock> {
        match self {
            Self::Paragraph(content) => trim_inline_content(content).map(MarkdownBlock::Paragraph),
            Self::Heading { level, content } => trim_inline_content(content)
                .map(|content| MarkdownBlock::Heading { level, content }),
            Self::ListItem {
                depth,
                marker,
                checked,
                content,
            } => trim_inline_content(content).map(|content| MarkdownBlock::ListItem {
                depth,
                marker,
                checked,
                content,
            }),
            Self::CodeBlock { language, content } => Some(MarkdownBlock::CodeBlock {
                language,
                content: content.trim_end().to_owned(),
            }),
            Self::Quote { depth, content } => {
                trim_inline_content(content).map(|content| MarkdownBlock::Quote { depth, content })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownListMarker {
    Bullet,
    Ordered(u64),
}

struct MarkdownListState {
    next_number: Option<u64>,
}

fn code_block_language(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Fenced(language) => {
            let language = language.trim();
            (!language.is_empty()).then(|| language.to_owned())
        }
        CodeBlockKind::Indented => None,
    }
}

fn trim_inline_content(content: String) -> Option<String> {
    let content = content.trim().to_owned();
    (!content.is_empty()).then_some(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_preview_blocks_keep_headings_and_list_markers() {
        let blocks = markdown_preview_blocks("# Title\n\n- [x] done\n- next\n\n1. first\n");

        assert_eq!(
            blocks[0],
            MarkdownBlock::Heading {
                level: HeadingLevel::H1,
                content: "Title".to_owned(),
            }
        );
        assert_eq!(
            blocks[1],
            MarkdownBlock::ListItem {
                depth: 0,
                marker: MarkdownListMarker::Bullet,
                checked: Some(true),
                content: "done".to_owned(),
            }
        );
        assert_eq!(
            blocks[2],
            MarkdownBlock::ListItem {
                depth: 0,
                marker: MarkdownListMarker::Bullet,
                checked: None,
                content: "next".to_owned(),
            }
        );
        assert_eq!(
            blocks[3],
            MarkdownBlock::ListItem {
                depth: 0,
                marker: MarkdownListMarker::Ordered(1),
                checked: None,
                content: "first".to_owned(),
            }
        );
    }

    #[test]
    fn markdown_preview_blocks_keep_code_language_and_tables() {
        let blocks = markdown_preview_blocks(
            "```rust\nfn main() {}\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n",
        );

        assert_eq!(
            blocks[0],
            MarkdownBlock::CodeBlock {
                language: Some("rust".to_owned()),
                content: "fn main() {}".to_owned(),
            }
        );
        assert_eq!(
            blocks[1],
            MarkdownBlock::TableRow(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            blocks[2],
            MarkdownBlock::TableRow(vec!["1".to_owned(), "2".to_owned()])
        );
    }
}
