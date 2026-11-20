//! 超级输入框（默认 Alt+Space）：键盘就能找到并执行功能、动作、设置页、收藏，搜索索引，跳到一个
//! 路径，新建文件或文件夹，执行一条命令。这里是纯模型——模式、候选和排序、结果行、命令的运行状态；
//! 怎么执行在 `app/super_box*.rs`。
//!
//! 结果行在改输入、换模式、后台结果回来时算一次（输入只校验这一次），界面直接画，执行时信行里的值。
//! 这里不读盘：路径补全、路径检查、索引查询都由应用层在后台做，结果按 `generation` 对号交回来。

mod find;
mod matching;
mod mode;
mod rows;
mod shell;
mod targets;

use std::path::{Path, PathBuf};

use file_operations::CommandOutput;
use file_search::{SearchHit, SearchTextScope};

pub(crate) use find::{read_find_query, FindQuery};
pub(crate) use matching::{BoxEntry, BoxTarget, EntryKind, MOST_ROWS};
pub(crate) use mode::BoxMode;
pub(crate) use shell::shell_invocation;
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
    /// 索引里找到的一项：`found` 里的下标。
    Found(usize),
    /// 框是空的时候：以前执行过的输入（带前缀），执行时填进框里。
    History(String),
    JumpTo(PathBuf),
    JumpSuggestion(PathBuf),
    Create(NewEntry),
    /// 命令模式：要执行的命令。
    Run(String),
    /// 不能执行的说明，选不中。
    Problem(String),
}

impl BoxRow {
    fn selectable(&self) -> bool {
        !matches!(self, Self::Problem(_))
    }
}

/// 应用层要在后台做的事，做完按 `generation` 交回来。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BoxLookup {
    /// 补全路径模式里正在打的字：`path` 是 `~` 换成主目录、相对路径接好的完整写法。
    Jump { generation: u64, path: String },
    /// 查索引（停顿一下再查）。
    Find {
        generation: u64,
        query: Box<FindQuery>,
        text_scope: SearchTextScope,
        limit: usize,
    },
}

/// 索引查询的情况。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FindStatus {
    Idle,
    Searching,
    /// 搜索服务没在跑、索引没建好、查询出错：给用户看的原因。
    Unavailable(String),
}

/// 命令模式里正在跑的、或者跑完的那一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRun {
    pub(crate) task_id: u64,
    pub(crate) command: String,
    pub(crate) state: RunState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunState {
    Running,
    /// 退出码 0：要显示的输出，结束时拼好一次。
    Finished(ShownOutput),
    /// 退出码不是 0、没启动起来、被停止：给用户看的原因。
    Failed(String),
}

/// 框里显示的输出：最后 [`SHOWN_OUTPUT_LINES`] 行，标准输出在前、标准错误在后。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShownOutput {
    pub(crate) text: String,
    pub(crate) lines: usize,
}

const SHOWN_OUTPUT_LINES: usize = 200;

impl RunState {
    pub(crate) fn finished(output: &CommandOutput) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines = stdout
            .lines()
            .chain(stderr.lines())
            .map(str::trim_end)
            .collect::<Vec<_>>();
        let shown = &lines[lines.len().saturating_sub(SHOWN_OUTPUT_LINES)..];
        Self::Finished(ShownOutput {
            text: shown.join("\n"),
            lines: shown.len(),
        })
    }
}

/// 搜索模式里最多列几个索引结果，排在功能、动作这些后面。
const FOUND_IN_SEARCH: usize = 5;

/// 不能复制：它拿着在途查询的取消钥匙，框没了查询就跟着取消。
#[derive(Debug)]
pub(crate) struct SuperBox {
    mode: BoxMode,
    input: String,
    entries: Vec<BoxEntry>,
    /// 打开时的历史快照，带前缀，最新的在前。
    history: Vec<String>,
    suggestions: Vec<PathBuf>,
    found: Vec<SearchHit>,
    find_status: FindStatus,
    /// 在途的索引查询：换成新的、或者框关掉时，旧的就取消。
    find_in_flight: Option<tokio_util::sync::DropGuard>,
    run: Option<CommandRun>,
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
            found: Vec::new(),
            find_status: FindStatus::Idle,
            find_in_flight: None,
            run: None,
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

    pub(crate) fn found(&self, index: usize) -> &SearchHit {
        &self.found[index]
    }

    pub(crate) fn find_status(&self) -> &FindStatus {
        &self.find_status
    }

    pub(crate) fn run(&self) -> Option<&CommandRun> {
        self.run.as_ref()
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
    ) -> Option<BoxLookup> {
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

    pub(crate) fn next_mode(&mut self, context: &BoxContext<'_>) -> Option<BoxLookup> {
        self.mode = self.mode.next();
        self.changed(context)
    }

    pub(crate) fn previous_mode(&mut self, context: &BoxContext<'_>) -> Option<BoxLookup> {
        self.mode = self.mode.previous();
        self.changed(context)
    }

    fn changed(&mut self, context: &BoxContext<'_>) -> Option<BoxLookup> {
        self.generation += 1;
        self.notice = None;
        self.suggestions.clear();
        self.found.clear();
        let lookup = self.lookup(context);
        self.find_status = match lookup {
            Some(BoxLookup::Find { .. }) => FindStatus::Searching,
            _ => FindStatus::Idle,
        };
        self.refresh(context);
        lookup
    }

    fn lookup(&self, context: &BoxContext<'_>) -> Option<BoxLookup> {
        let generation = self.generation;
        let text = self.input.trim();
        if text.is_empty() {
            return None;
        }
        match self.mode {
            BoxMode::Path => {
                let path = targets::jump_path(text, context.current_dir, context.home)?;
                Some(BoxLookup::Jump {
                    generation,
                    path: path.to_string_lossy().into_owned(),
                })
            }
            // 搜索模式只按名字找几个，排在别的结果后面。
            BoxMode::Search => Some(BoxLookup::Find {
                generation,
                query: Box::new(FindQuery::names(text)),
                text_scope: SearchTextScope::NameOnly,
                limit: FOUND_IN_SEARCH,
            }),
            BoxMode::Files => find::read_find_query(text)
                .ok()
                .filter(|query| !query.terms.is_empty())
                .map(|query| BoxLookup::Find {
                    generation,
                    query: Box::new(query),
                    text_scope: SearchTextScope::NameAndContent,
                    limit: MOST_ROWS,
                }),
            BoxMode::New | BoxMode::Shell => None,
        }
    }

    /// 这次查询开始了；之前没查完的那次取消。
    pub(crate) fn track_find(&mut self, cancellation: tokio_util::sync::CancellationToken) {
        self.find_in_flight = Some(cancellation.drop_guard());
    }

    /// 后台查到的索引结果；输入已经变了的过期结果不要。
    pub(crate) fn accept_found(
        &mut self,
        generation: u64,
        found: Result<Vec<SearchHit>, String>,
        context: &BoxContext<'_>,
    ) {
        if generation != self.generation {
            return;
        }
        (self.found, self.find_status) = match found {
            Ok(found) => (found, FindStatus::Idle),
            Err(reason) => (Vec::new(), FindStatus::Unavailable(reason)),
        };
        self.refresh(context);
    }

    /// 命令进了队列。
    pub(crate) fn start_run(&mut self, task_id: u64, command: String) {
        self.run = Some(CommandRun {
            task_id,
            command,
            state: RunState::Running,
        });
    }

    /// 队列里的任务结束了；不是这个框发的就不管。
    pub(crate) fn finish_run(&mut self, task_id: u64, state: RunState) {
        if let Some(run) = self.run.as_mut().filter(|run| run.task_id == task_id) {
            run.state = state;
        }
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
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
