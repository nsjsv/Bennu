//! 渲染模式里的搜索：在解析好的 Markdown 的各个单元（正文段、代码块、表格单元格）里
//! 匹配，不匹配标记符号。内容最多 2 MB、已经在内存里，改词时在界面线程一次算完。

use std::ops::Range;

use super::search_scan::{SearchDirection, SearchMatcher};
use crate::markdown_render::RenderedMarkdown;

/// 渲染内容里的一处匹配：第几个单元、单元文字里的字节范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMatch {
    pub unit: usize,
    pub range: Range<usize>,
}

pub(crate) struct RenderedSearch {
    matches: Vec<RenderedMatch>,
    current: Option<usize>,
}

impl RenderedSearch {
    /// 全部匹配按出现顺序排好；有匹配时当前是第一个。
    pub(crate) fn new(markdown: &RenderedMarkdown, matcher: &SearchMatcher) -> Self {
        let matches: Vec<RenderedMatch> = markdown
            .units()
            .flat_map(|unit| {
                matcher
                    .find_iter(&unit.inline.text)
                    .map(|range| RenderedMatch {
                        unit: unit.unit,
                        range,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let current = (!matches.is_empty()).then_some(0);
        Self { matches, current }
    }

    pub(crate) fn matches(&self) -> &[RenderedMatch] {
        &self.matches
    }

    pub(crate) fn current(&self) -> Option<usize> {
        self.current
    }

    /// 下一个/上一个，到头从另一端接着。
    pub(crate) fn step(&mut self, direction: SearchDirection) {
        let len = self.matches.len();
        if len == 0 {
            return;
        }
        self.current = Some(match (self.current, direction) {
            (Some(index), SearchDirection::Forward) => (index + 1) % len,
            (Some(index), SearchDirection::Backward) => (index + len - 1) % len,
            (None, SearchDirection::Forward) => 0,
            (None, SearchDirection::Backward) => len - 1,
        });
    }
}
