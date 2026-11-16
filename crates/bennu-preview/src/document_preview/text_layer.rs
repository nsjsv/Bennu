//! 文档的文字层：pdftotext 给出每页每个词的位置（点，已按页面旋转换成显示方向），换算成
//! 页内比例存着，搜索高亮、拖选复制都用它。整份文档只抽一次，第一次搜索或拖选时才抽。

use std::sync::Arc;

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

/// 拖选：按下的那个词和现在指着的那个词（页，词），中间的都选中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSelection {
    pub anchor: (usize, usize),
    pub focus: (usize, usize),
}

impl DocumentSelection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// 这一页被选中的词（含首尾），没有返回 None。
    pub fn words_on_page(&self, page: usize, text: &DocumentText) -> Option<(usize, usize)> {
        let ((first_page, first_word), (last_page, last_word)) = self.ordered();
        if page < first_page || page > last_page {
            return None;
        }
        let count = text.pages.get(page)?.words.len();
        if count == 0 {
            return None;
        }
        let start = if page == first_page { first_word } else { 0 };
        let end = if page == last_page {
            last_word
        } else {
            count - 1
        };
        (start <= end).then_some((start, end.min(count - 1)))
    }

    /// 复制出来的文字：词之间空格，行末换行，页之间空一行。
    pub fn text(&self, text: &DocumentText) -> String {
        let ((first_page, _), (last_page, _)) = self.ordered();
        let mut copied = String::new();
        for page in first_page..=last_page {
            let Some((start, end)) = self.words_on_page(page, text) else {
                continue;
            };
            if !copied.is_empty() && !copied.ends_with('\n') {
                copied.push('\n');
            }
            if page != first_page {
                copied.push('\n');
            }
            for word in &text.pages[page].words[start..=end] {
                copied.push_str(&word.text);
                copied.push(if word.ends_line { '\n' } else { ' ' });
            }
        }
        copied.trim_end().to_owned()
    }
}

/// 指着的地方离哪个词最近（在词里就是那个词）。
/// 指在哪个词上（落在词的框里）；指在空白处是 `None`。
pub fn word_under(page: &PageText, x: f32, y: f32) -> Option<usize> {
    page.words
        .iter()
        .position(|word| word.bounds.distance_to(x, y) == 0.0)
}

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
