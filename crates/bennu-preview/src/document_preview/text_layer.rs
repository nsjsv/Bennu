//! 文档的文字层：pdftotext 给出每页每个词的位置（点，已按页面旋转换成显示方向），换算成
//! 页内比例存着，搜索高亮、拖选复制都用它。整份文档只抽一次，第一次搜索或拖选时才抽。

use std::ops::Range;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::text_document::{SearchDirection, SearchSummary};

/// 页内比例（0–1，左上为原点）的矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl PageRect {
    fn distance_to(&self, x: f32, y: f32) -> f32 {
        let dx = (self.left - x).max(0.0).max(x - self.right);
        let dy = (self.top - y).max(0.0).max(y - self.bottom);
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextWord {
    pub text: String,
    pub bounds: PageRect,
    /// 这个词是一行的最后一个：复制时后面换行。
    pub ends_line: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    pub words: Vec<TextWord>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentText {
    pub pages: Vec<PageText>,
}

#[derive(Debug, Clone)]
pub enum DocumentTextLayer {
    NotLoaded,
    Loading,
    Ready(Arc<DocumentText>),
    Failed(String),
}

/// 一处匹配：某页从第几个词到第几个词（含）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextMatch {
    pub page: usize,
    pub first_word: usize,
    pub last_word: usize,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentSearch {
    pub query: String,
    pub case_sensitive: bool,
    pub matches: Vec<TextMatch>,
    pub current: Option<usize>,
}

impl DocumentSearch {
    pub fn summary(&self, layer: &DocumentTextLayer) -> Option<SearchSummary> {
        if self.query.trim().is_empty() {
            return None;
        }
        Some(SearchSummary {
            ordinal: self.current.map(|index| index as u64 + 1),
            total: self.matches.len() as u64,
            finished: !matches!(
                layer,
                DocumentTextLayer::Loading | DocumentTextLayer::NotLoaded
            ),
        })
    }

    /// 重新找全部匹配；当前那处挑 `from_page` 起的第一处。
    pub fn refresh(&mut self, text: &DocumentText, from_page: usize) {
        self.matches = find_matches(text, &self.query, self.case_sensitive);
        self.current = self
            .matches
            .iter()
            .position(|found| found.page >= from_page)
            .or((!self.matches.is_empty()).then_some(0));
    }

    /// 下一处/上一处，到头绕回。
    pub fn step(&mut self, direction: SearchDirection) {
        let count = self.matches.len();
        if count == 0 {
            self.current = None;
            return;
        }
        self.current = Some(match (self.current, direction) {
            (None, _) => 0,
            (Some(index), SearchDirection::Forward) => (index + 1) % count,
            (Some(index), SearchDirection::Backward) => (index + count - 1) % count,
        });
    }

    pub fn current_match(&self) -> Option<TextMatch> {
        self.matches.get(self.current?).copied()
    }
}

/// 选区的一端：第几页第几个词，词里第几个字之前（按字素数：0 是词首，字数是词尾）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub page: usize,
    pub word: usize,
    pub grapheme: usize,
}

/// 拖选：按下的地方和现在指着的地方，中间的字都选中；两端在同一处时什么都没选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSelection {
    pub anchor: TextPosition,
    pub focus: TextPosition,
}

impl DocumentSelection {
    /// 整个词：双击。
    pub fn word(page: usize, word: usize, text: &DocumentText) -> Option<Self> {
        let graphemes = grapheme_count(&text.pages.get(page)?.words.get(word)?.text);
        Some(Self {
            anchor: TextPosition {
                page,
                word,
                grapheme: 0,
            },
            focus: TextPosition {
                page,
                word,
                grapheme: graphemes,
            },
        })
    }

    fn ordered(&self) -> (TextPosition, TextPosition) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// 这一页每个被选中的词，和词里选中的字（字素下标，左闭右开，不空）。
    pub fn graphemes_on_page(
        &self,
        page: usize,
        text: &DocumentText,
    ) -> Vec<(usize, Range<usize>)> {
        let (start, end) = self.ordered();
        let Some(words) = text.pages.get(page).map(|page| &page.words) else {
            return Vec::new();
        };
        if page < start.page || page > end.page || words.is_empty() {
            return Vec::new();
        }
        let first = if page == start.page { start.word } else { 0 };
        let last = if page == end.page {
            end.word.min(words.len() - 1)
        } else {
            words.len() - 1
        };
        (first..=last)
            .filter_map(|index| {
                let count = grapheme_count(&words[index].text);
                let from = if page == start.page && index == start.word {
                    start.grapheme.min(count)
                } else {
                    0
                };
                let to = if page == end.page && index == end.word {
                    end.grapheme.min(count)
                } else {
                    count
                };
                (from < to).then_some((index, from..to))
            })
            .collect()
    }

    /// 复制出来的文字：词之间空格，行末换行，页之间空一行；词选了一半就只取那一半。
    pub fn text(&self, text: &DocumentText) -> String {
        let (start, end) = self.ordered();
        let mut copied = String::new();
        for page in start.page..=end.page {
            let selected = self.graphemes_on_page(page, text);
            if selected.is_empty() {
                continue;
            }
            if !copied.is_empty() && !copied.ends_with('\n') {
                copied.push('\n');
            }
            if page != start.page {
                copied.push('\n');
            }
            for (index, range) in selected {
                let word = &text.pages[page].words[index];
                copied.push_str(grapheme_slice(&word.text, range.clone()));
                if range.end == grapheme_count(&word.text) {
                    copied.push(if word.ends_line { '\n' } else { ' ' });
                }
            }
        }
        copied.trim_end().to_owned()
    }
}

pub fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

fn grapheme_slice(text: &str, range: Range<usize>) -> &str {
    let mut boundaries = text
        .grapheme_indices(true)
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()));
    let from = boundaries.nth(range.start).unwrap_or(text.len());
    let to = if range.end > range.start {
        boundaries
            .nth(range.end - range.start - 1)
            .unwrap_or(text.len())
    } else {
        from
    };
    &text[from..to]
}

/// 指着的地方离哪个词最近（在词里就是那个词）。
pub fn word_near(page: &PageText, x: f32, y: f32) -> Option<usize> {
    page.words
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            left.bounds
                .distance_to(x, y)
                .total_cmp(&right.bounds.distance_to(x, y))
        })
        .map(|(index, _)| index)
}

/// 每页把词按顺序用空格连起来找；跨行、跨词的短语也能找到，命中的词整个高亮。
pub fn find_matches(text: &DocumentText, query: &str, case_sensitive: bool) -> Vec<TextMatch> {
    let fold = |value: &str| {
        if case_sensitive {
            value.to_owned()
        } else {
            value.to_lowercase()
        }
    };
    let query = fold(&query.split_whitespace().collect::<Vec<_>>().join(" "));
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (page_index, page) in text.pages.iter().enumerate() {
        let mut joined = String::new();
        // 拼起来的字符串里每个字节属于第几个词（空格算前一个词）。
        let mut owner = Vec::new();
        for (word_index, word) in page.words.iter().enumerate() {
            if word_index > 0 {
                joined.push(' ');
                owner.push(word_index - 1);
            }
            let folded = fold(&word.text);
            owner.extend(std::iter::repeat_n(word_index, folded.len()));
            joined.push_str(&folded);
        }
        for (start, found) in joined.match_indices(&query) {
            matches.push(TextMatch {
                page: page_index,
                first_word: owner[start],
                last_word: owner[start + found.len() - 1],
            });
        }
    }
    matches
}

/// pdftotext `-bbox-layout` 的输出（XHTML）：`page` > … > `line` > `word`，词带四个坐标（点）。
/// 坐标已按页面旋转换成显示方向，按显示出来的页宽页高（`page_sizes`，点）换成比例。
pub fn parse_bbox_layout(xml: &str, page_sizes: &[(f64, f64)]) -> Result<DocumentText, String> {
    // 输出开头带 XHTML 的 DOCTYPE；不去读外部 DTD，只允许它出现。
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let document = roxmltree::Document::parse_with_options(xml, options)
        .map_err(|error| format!("could not read pdftotext output: {error}"))?;
    let mut pages = Vec::new();
    for (page_node, (width, height)) in document
        .descendants()
        .filter(|node| node.has_tag_name("page"))
        .zip(page_sizes)
    {
        let mut words = Vec::new();
        for line in page_node
            .descendants()
            .filter(|node| node.has_tag_name("line"))
        {
            let start = words.len();
            for word in line.children().filter(|node| node.has_tag_name("word")) {
                let coordinate = |name| {
                    word.attribute(name)
                        .and_then(|value| value.parse::<f64>().ok())
                };
                let (Some(left), Some(top), Some(right), Some(bottom)) = (
                    coordinate("xMin"),
                    coordinate("yMin"),
                    coordinate("xMax"),
                    coordinate("yMax"),
                ) else {
                    continue;
                };
                let text = word.text().unwrap_or_default().trim();
                if text.is_empty() {
                    continue;
                }
                words.push(TextWord {
                    text: text.to_owned(),
                    bounds: PageRect {
                        left: (left / width) as f32,
                        top: (top / height) as f32,
                        right: (right / width) as f32,
                        bottom: (bottom / height) as f32,
                    },
                    ends_line: false,
                });
            }
            if let Some(last) = words.get_mut(start..).and_then(|line| line.last_mut()) {
                last.ends_line = true;
            }
        }
        pages.push(PageText { words });
    }
    Ok(DocumentText { pages })
}

#[cfg(test)]
#[path = "text_layer/tests.rs"]
mod tests;
