//! 超级输入框（默认 Alt+Space）：键盘就能找到并执行功能、动作、设置页、收藏，跳到一个路径，
//! 新建文件或文件夹。这里是纯模型——模式、候选和排序、结果行；怎么执行在 `app/super_box.rs`。
//!
//! 结果行在改输入、换模式时算一次（输入只校验这一次），界面直接画，执行时信行里的值。
//! 这里不读盘：路径补全和路径检查由应用层在后台做，结果按 `generation` 对号交回来。

mod matching;
mod mode;
mod targets;

use std::path::{Path, PathBuf};

pub(crate) use matching::{BoxEntry, BoxTarget, EntryKind, MOST_ROWS};
pub(crate) use mode::BoxMode;
pub(crate) use targets::NewEntry;

use crate::model::InputHistory;

pub(crate) type SuperBoxHistory = InputHistory<100>;

/// 算结果行要的外部情况，由应用层按当前标签页给。
pub(crate) struct BoxContext<'a> {
    /// 当前标签页的文件夹；回收站、搜索结果里没有。
    pub(crate) current_dir: Option<&'a Path>,
    pub(crate) home: &'a Path,
    /// 这里不能新建时的原因（回收站、压缩包里、搜索结果）。
    pub(crate) cannot_create: Option<String>,
    /// 当前文件夹里是不是已经有这个名字（按已经列出的条目回答）。
    pub(crate) taken: &'a dyn Fn(&str) -> bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoxRow {
    /// 搜索模式的一条候选：`entries` 里的下标。
    Entry(usize),
    /// 框是空的时候：以前执行过的输入（带前缀），执行时填进框里。
    History(String),
    JumpTo(PathBuf),
    JumpSuggestion(PathBuf),
    Create(NewEntry),
    /// 不能执行的说明，选不中。
    Problem(String),
}

impl BoxRow {
    fn selectable(&self) -> bool {
        !matches!(self, Self::Problem(_))
    }
}

/// 应用层要在后台列的子文件夹：补全路径模式里正在打的字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JumpLookup {
    pub(crate) generation: u64,
    /// `~` 已经换成主目录、相对路径已经接好的完整写法。
    pub(crate) path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SuperBox {
    mode: BoxMode,
    input: String,
    entries: Vec<BoxEntry>,
    /// 打开时的历史快照，带前缀，最新的在前。
    history: Vec<String>,
    suggestions: Vec<PathBuf>,
    rows: Vec<BoxRow>,
    selected: Option<usize>,
    generation: u64,
    /// 上一次执行没成的原因（路径不存在）；一改输入就清掉。
    notice: Option<String>,
}

impl SuperBox {
    pub(crate) fn open(
        mode: BoxMode,
        entries: Vec<BoxEntry>,
        history: Vec<String>,
        context: &BoxContext<'_>,
    ) -> Self {
        let mut opened = Self {
            mode,
            input: String::new(),
            entries,
            history,
            suggestions: Vec::new(),
            rows: Vec::new(),
            selected: None,
            generation: 0,
            notice: None,
        };
        opened.refresh(context);
        opened
    }

    pub(crate) fn mode(&self) -> BoxMode {
        self.mode
    }

    pub(crate) fn input(&self) -> &str {
        &self.input
    }

    pub(crate) fn rows(&self) -> &[BoxRow] {
        &self.rows
    }

    pub(crate) fn entry(&self, index: usize) -> &BoxEntry {
        &self.entries[index]
    }

    pub(crate) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected_row(&self) -> Option<&BoxRow> {
        self.rows.get(self.selected?)
    }

    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// 打了字。开头是前缀就切模式、去掉前缀；搜索模式里以 `/` `~` 开头也切到路径。
    pub(crate) fn set_input(
        &mut self,
        text: String,
        context: &BoxContext<'_>,
    ) -> Option<JumpLookup> {
        let (mode, text) = match mode::split_prefix(&text) {
            Some((mode, rest)) => (mode, rest.to_owned()),
            None if self.mode == BoxMode::Search && text.starts_with(['/', '~']) => {
                (BoxMode::Path, text)
            }
            None => (self.mode, text),
        };
        self.mode = mode;
        self.input = text;
        self.changed(context)
    }

    pub(crate) fn next_mode(&mut self, context: &BoxContext<'_>) -> Option<JumpLookup> {
        self.mode = self.mode.next();
        self.changed(context)
    }

    pub(crate) fn previous_mode(&mut self, context: &BoxContext<'_>) -> Option<JumpLookup> {
        self.mode = self.mode.previous();
        self.changed(context)
    }

    fn changed(&mut self, context: &BoxContext<'_>) -> Option<JumpLookup> {
        self.generation += 1;
        self.notice = None;
        self.suggestions.clear();
        self.refresh(context);
        if self.mode != BoxMode::Path {
            return None;
        }
        let path = targets::jump_path(&self.input, context.current_dir, context.home)?;
        Some(JumpLookup {
            generation: self.generation,
            path: path.to_string_lossy().into_owned(),
        })
    }

    /// 后台列好的子文件夹；输入已经变了的过期结果不要。
    pub(crate) fn accept_suggestions(
        &mut self,
        generation: u64,
        suggestions: Vec<PathBuf>,
        context: &BoxContext<'_>,
    ) {
        if generation != self.generation || self.mode != BoxMode::Path {
            return;
        }
        self.suggestions = suggestions;
        self.refresh(context);
    }

    /// 执行没成（路径不存在），框不关，说明为什么。
    pub(crate) fn show_notice(&mut self, generation: u64, notice: String) {
        if generation == self.generation {
            self.notice = Some(notice);
        }
    }

    pub(crate) fn select_next(&mut self) {
        self.step_selection(1);
    }

    pub(crate) fn select_previous(&mut self) {
        self.step_selection(self.rows.len().saturating_sub(1));
    }

    /// 鼠标指着的行。
    pub(crate) fn select(&mut self, index: usize) {
        if self.rows.get(index).is_some_and(BoxRow::selectable) {
            self.selected = Some(index);
        }
    }

    /// 往后挪 `offset` 行（挪 `len - 1` 就是往前一行），跳过选不中的行，到头绕回。
    fn step_selection(&mut self, offset: usize) {
        let count = self.rows.len();
        let Some(current) = self.selected else {
            return;
        };
        self.selected = (1..=count)
            .map(|step| (current + offset * step) % count)
            .find(|index| self.rows[*index].selectable());
    }

    /// 执行成功后记进历史的写法。
    pub(crate) fn history_text(&self) -> String {
        mode::history_text(self.mode, self.input.trim())
    }

    fn refresh(&mut self, context: &BoxContext<'_>) {
        self.rows = self.compute_rows(context);
        self.selected = self.rows.iter().position(BoxRow::selectable);
    }

    fn compute_rows(&self, context: &BoxContext<'_>) -> Vec<BoxRow> {
        if self.input.trim().is_empty() {
            return self
                .history
                .iter()
                .filter(|entry| mode::read_history(entry).0 == self.mode)
                .take(MOST_ROWS)
                .map(|entry| BoxRow::History(entry.clone()))
                .collect();
        }
        match self.mode {
            BoxMode::Search => matching::ranked(&self.entries, &self.input)
                .into_iter()
                .map(BoxRow::Entry)
                .collect(),
            BoxMode::Path => {
                let Some(target) =
                    targets::jump_path(&self.input, context.current_dir, context.home)
                else {
                    return vec![no_folder_here()];
                };
                let suggestions = self
                    .suggestions
                    .iter()
                    .filter(|suggestion| **suggestion != target)
                    .take(MOST_ROWS - 1)
                    .cloned()
                    .map(BoxRow::JumpSuggestion)
                    .collect::<Vec<_>>();
                std::iter::once(BoxRow::JumpTo(target))
                    .chain(suggestions)
                    .collect()
            }
            BoxMode::New => {
                if let Some(reason) = &context.cannot_create {
                    return vec![BoxRow::Problem(reason.clone())];
                }
                match targets::read_new_entry(&self.input, context.taken) {
                    Ok(entry) => vec![BoxRow::Create(entry)],
                    Err(reason) => vec![BoxRow::Problem(reason)],
                }
            }
        }
    }
}

fn no_folder_here() -> BoxRow {
    BoxRow::Problem(tr!(
        "Relative paths need a folder; type a path starting with / or ~"
    ))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
