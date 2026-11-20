//! 超级输入框（默认 Alt+Space）：键盘就能找到并执行功能、动作、设置页、收藏，搜索索引，跳到一个
//! 路径，新建文件或文件夹，执行一条命令。这里是纯模型——模式、候选和排序、结果行、命令的运行状态；
//! 怎么执行在 `app/super_box*.rs`。
//!
//! 结果行在改输入、换模式、后台结果回来时算一次（输入只校验这一次），界面直接画，执行时信行里的值。
//! 这里不读盘：路径补全、路径检查、索引查询都由应用层在后台做，结果按 `generation` 对号交回来。

mod complete;
mod find;
mod found;
mod matching;
mod mode;
mod pipe;
mod rows;
mod shell;
mod targets;

use std::path::{Path, PathBuf};

use file_search::{SearchHit, SearchTextScope};

pub(crate) use complete::{Candidate, Choices, ListSource, Listing};
pub(crate) use find::{read_find_query, FindQuery};
pub(crate) use found::FoundInColumns;
pub(crate) use matching::{BoxEntry, BoxTarget, EntryKind, MOST_ROWS};
use mode::fold;
pub(crate) use mode::{read_history, BoxMode};
pub(crate) use pipe::{Pipe, PipeAction, PipeSource, Stage};

/// 管道一次最多对这么多项：再多就是写宽了，让用户加筛选缩小范围（用户定）。
pub(crate) const MOST_PICKED: usize = 1000;
pub(crate) use shell::shell_invocation;
pub(crate) use targets::NewEntry;

use crate::model::InputHistory;
use crate::terminal_panel::session::SessionId;

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
    /// 管道：挑出来的项接着各步，内容在 [`SuperBox::pipe`]、[`SuperBox::picked`]。
    Pipe,
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
    /// 列补全的候选（路径模式、命令模式、管道里的文件夹）：名字开头是 `name`（不分大小写）的。
    List {
        generation: u64,
        source: ListSource,
        name: String,
    },
    /// 按管道前面写的挑出项来；结果按挑法认，只改后面的步骤时不重挑。最多要 `MOST_PICKED + 1` 个。
    Pick(PipeSource),
    /// 查索引（停顿一下再查）。
    Find {
        generation: u64,
        query: Box<FindQuery>,
        text_scope: SearchTextScope,
        limit: usize,
    },
}

/// 管道挑出来的项，和它是按什么挑的。
#[derive(Debug, Clone, PartialEq)]
struct Picked {
    source: PipeSource,
    items: PickedItems,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickedItems {
    Waiting,
    Found(Vec<PathBuf>),
    /// 搜索服务没在跑这些：给用户看的原因。
    Unavailable(String),
}

/// Tab 列出来、再按 Tab 一个个换的东西；打字就收起。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabList {
    /// 补不动时的名字。
    Names(Choices),
    /// 框空着按 Tab：能打的标签（默认模式以外的模式）。
    Tags {
        tags: Vec<BoxMode>,
        chosen: Option<usize>,
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

/// 命令模式里正在跑的、或者跑完的那一次。命令在不显示的终端标签里跑，`session` 是它的会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRun {
    pub(crate) session: SessionId,
    pub(crate) command: String,
    pub(crate) state: RunState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunState {
    Running,
    Ended(RunEnd),
}

/// 一条命令怎么结束的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunEnd {
    /// 程序退出了：怎么退出的，和它终端里最后的输出（结束时取一次）。
    Exited { exit: Exit, output: ShownOutput },
    /// 等不到它的退出情况：原因。
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Exit {
    Code(u32),
    /// 被信号结束（比如按了停止）：信号的名字。
    Signal(String),
}

/// 框里显示的输出：终端里最后 [`SHOWN_OUTPUT_LINES`] 行，就是终端里显示的样子。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShownOutput {
    pub(crate) text: String,
    pub(crate) lines: usize,
}

pub(crate) const SHOWN_OUTPUT_LINES: usize = 200;

impl ShownOutput {
    pub(crate) fn from_text(text: String) -> Self {
        let lines = text.lines().count();
        Self { text, lines }
    }
}

/// 以前跑过的一条命令最后一次的结果。存在状态库里，重启后还在；命令模式里选中那条历史时显示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PastRun {
    /// 框里打的命令（不带模式前缀）。
    pub(crate) command: String,
    /// Unix 毫秒。
    pub(crate) at_ms: i64,
    pub(crate) end: RunEnd,
}

/// 记几条命令的结果。
pub(crate) const PAST_RUNS_KEPT: usize = 20;

/// 搜索模式里最多列几个索引结果，排在功能、动作这些后面。
const FOUND_IN_SEARCH: usize = 5;

/// 不能复制：它拿着在途查询的取消钥匙，框没了查询就跟着取消。
#[derive(Debug)]
pub(crate) struct SuperBox {
    /// 设置里的默认模式：不打标签直接打字就是它，它不显示胶囊。
    default_mode: BoxMode,
    mode: BoxMode,
    input: String,
    entries: Vec<BoxEntry>,
    /// 管道里能写名字的动作（不管适不适用于现在选中的项）。
    pipe_actions: Vec<PipeAction>,
    /// 这次输入是不是管道、读成了什么；改输入时读一次。
    pipe: Option<Result<Pipe, String>>,
    picked: Option<Picked>,
    /// 打开时的历史快照，带前缀，最新的在前。
    history: Vec<String>,
    /// 这次输入的补全候选；路径模式的子文件夹行也从这里来。
    listing: Listing,
    tab_list: Option<TabList>,
    /// 框里的字后面淡淡显示的建议，→ 接受。
    suggestion: Option<String>,
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
        pipe_actions: Vec<PipeAction>,
        history: Vec<String>,
        context: &BoxContext<'_>,
    ) -> Self {
        let mut opened = Self {
            default_mode: mode,
            mode,
            input: String::new(),
            entries,
            pipe_actions,
            pipe: None,
            picked: None,
            history,
            listing: Listing::default(),
            tab_list: None,
            suggestion: None,
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

    /// 写对了的管道。
    pub(crate) fn pipe(&self) -> Option<&Pipe> {
        self.pipe.as_ref()?.as_ref().ok()
    }

    /// 管道挑出来的项（挑好了才有）。
    pub(crate) fn picked(&self) -> Option<&[PathBuf]> {
        match &self.picked.as_ref()?.items {
            PickedItems::Found(items) => Some(items),
            PickedItems::Waiting | PickedItems::Unavailable(_) => None,
        }
    }

    pub(crate) fn tab_list(&self) -> Option<&TabList> {
        self.tab_list.as_ref()
    }

    /// 这个模式要不要显示胶囊：默认模式不用（直接打字就是它）。
    pub(crate) fn shows_tag(&self, mode: BoxMode) -> bool {
        mode != self.default_mode
    }

    pub(crate) fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// 打了字。开头是前缀就切模式（框里显示成前面的胶囊）、去掉前缀；搜索模式里以 `/` `~` 开头也切到
    /// 路径。
    pub(crate) fn set_input(&mut self, text: String, context: &BoxContext<'_>) -> Vec<BoxLookup> {
        let (mode, text) = match mode::split_prefix(&text) {
            Some((mode, rest)) => (mode, rest.to_owned()),
            // 已经空了又按退格：删掉前面的胶囊，回到默认模式。输入框空着按退格也照样报一次“变成
            // 空的”，只认这一条消息；全局按键和它先后不定，不能靠按键判断。
            None if text.is_empty() && self.input.is_empty() => (self.default_mode, text),
            None if self.mode == BoxMode::Search && text.starts_with(['/', '~']) => {
                (BoxMode::Path, text)
            }
            None => (self.mode, text),
        };
        self.mode = mode;
        self.input = text;
        self.tab_list = None;
        self.changed(context)
    }

    /// Shift+Tab：换下一种模式，到头绕回。
    pub(crate) fn next_mode(&mut self, context: &BoxContext<'_>) -> Vec<BoxLookup> {
        self.mode = self.mode.next();
        self.tab_list = None;
        self.changed(context)
    }

    /// Tab：框空着就列出能打的标签；有字就补最后那个词（候选已经列出来了才补，打字时就在列）。
    /// 列出来以后再按 Tab 一个个换。
    pub(crate) fn complete(&mut self, context: &BoxContext<'_>) -> Vec<BoxLookup> {
        match self.tab_list.as_mut() {
            Some(TabList::Names(choices)) => {
                self.input = choices.next();
                return self.changed(context);
            }
            Some(TabList::Tags { tags, chosen }) => {
                let next = chosen.map_or(0, |chosen| (chosen + 1) % tags.len());
                *chosen = Some(next);
                self.mode = tags[next];
                return self.changed(context);
            }
            None => {}
        }
        if self.input.is_empty() {
            let tags: Vec<_> = BoxMode::ALL
                .into_iter()
                .filter(|mode| self.shows_tag(*mode))
                .collect();
            let chosen = tags.iter().position(|mode| *mode == self.mode);
            self.tab_list = Some(TabList::Tags { tags, chosen });
            return Vec::new();
        }
        let Some(word) = complete::word(self.mode, &self.input) else {
            return Vec::new();
        };
        match complete::complete(&word, &self.input, &self.listing) {
            complete::Completed::Text(text) => {
                self.input = text;
                self.changed(context)
            }
            complete::Completed::Choices(candidates) => {
                self.tab_list = Some(TabList::Names(Choices::new(
                    word,
                    self.input.clone(),
                    candidates,
                )));
                Vec::new()
            }
            complete::Completed::Nothing => Vec::new(),
        }
    }

    /// →：接受淡色的建议。
    pub(crate) fn accept_suggestion(&mut self, context: &BoxContext<'_>) -> Vec<BoxLookup> {
        let Some(suggestion) = self.suggestion.take() else {
            return Vec::new();
        };
        self.input.push_str(&suggestion);
        self.tab_list = None;
        self.changed(context)
    }

    fn changed(&mut self, context: &BoxContext<'_>) -> Vec<BoxLookup> {
        self.generation += 1;
        self.notice = None;
        self.listing = Listing::default();
        self.found.clear();
        self.pipe = pipe::read_pipe(self.mode, &self.input, &self.pipe_actions);
        let mut lookups = Vec::new();
        match pipe::read_pipe_source(self.mode, &self.input).and_then(Result::ok) {
            // 挑法没变（只改了后面的步骤）：挑好的留着。
            Some(source) if self.picked.as_ref().map(|picked| &picked.source) == Some(&source) => {}
            Some(source) => {
                self.picked = Some(Picked {
                    source: source.clone(),
                    items: PickedItems::Waiting,
                });
                lookups.push(BoxLookup::Pick(source));
            }
            None => self.picked = None,
        }
        let word = complete::word(self.mode, &self.input);
        if let Some(word) = word.as_ref().filter(|word| word.lists_stages()) {
            self.listing = self.stage_listing(&word.name);
        }
        lookups.extend(self.lookup(word, context));
        let waiting = self
            .picked
            .as_ref()
            .is_some_and(|picked| picked.items == PickedItems::Waiting);
        let finding = lookups
            .iter()
            .any(|lookup| matches!(lookup, BoxLookup::Find { .. }));
        self.find_status = if waiting || finding {
            FindStatus::Searching
        } else {
            FindStatus::Idle
        };
        self.refresh(context);
        lookups
    }

    /// 管道一段开头的候选：动作名和步骤的写法，框自己就有。
    fn stage_listing(&self, name: &str) -> Listing {
        let mut candidates: Vec<_> = pipe::step_words()
            .map(str::to_owned)
            .chain(self.pipe_actions.iter().map(|action| action.name.clone()))
            .filter(|candidate| fold(candidate).starts_with(&fold(name)))
            .map(|name| Candidate {
                name,
                is_dir: false,
            })
            .collect();
        candidates.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Listing {
            folder: None,
            candidates,
            truncated: false,
        }
    }

    /// 补全要列的文件夹、命令；不是管道时搜索和文件模式查索引。
    fn lookup(&self, word: Option<complete::Word>, context: &BoxContext<'_>) -> Option<BoxLookup> {
        let generation = self.generation;
        let text = self.input.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(source) = word
            .as_ref()
            .and_then(|word| word.source(context.current_dir, context.home))
        {
            return Some(BoxLookup::List {
                generation,
                source,
                name: word?.name,
            });
        }
        if self.pipe.is_some() {
            return None;
        }
        match self.mode {
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
            BoxMode::Path | BoxMode::Shell | BoxMode::New => None,
        }
    }

    /// 管道挑出来的项；挑法已经变了的不要。
    pub(crate) fn accept_picked(
        &mut self,
        source: &PipeSource,
        items: Result<Vec<PathBuf>, String>,
        context: &BoxContext<'_>,
    ) {
        let Some(picked) = self
            .picked
            .as_mut()
            .filter(|picked| picked.source == *source)
        else {
            return;
        };
        picked.items = match items {
            Ok(items) => PickedItems::Found(items),
            Err(reason) => PickedItems::Unavailable(reason),
        };
        self.find_status = FindStatus::Idle;
        self.refresh(context);
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

    /// 命令开始跑了。
    pub(crate) fn start_run(&mut self, session: SessionId, command: String) {
        self.run = Some(CommandRun {
            session,
            command,
            state: RunState::Running,
        });
    }

    /// 命令结束了；不是这个框发的就不管。
    pub(crate) fn finish_run(&mut self, session: SessionId, end: RunEnd) {
        if let Some(run) = self.run.as_mut().filter(|run| run.session == session) {
            run.state = RunState::Ended(end);
        }
    }

    /// 后台列好的补全候选；输入已经变了的过期结果不要。
    pub(crate) fn accept_listing(
        &mut self,
        generation: u64,
        listing: Listing,
        context: &BoxContext<'_>,
    ) {
        if generation != self.generation {
            return;
        }
        self.listing = listing;
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
        self.suggestion = self.compute_suggestion();
    }

    /// 有能补的词时（路径、命令、管道的一段）：这个模式最近的历史里接得上的那条优先（同 fish），
    /// 没有再看候选。
    fn compute_suggestion(&self) -> Option<String> {
        let word = complete::word(self.mode, &self.input)?;
        if self.input.is_empty() {
            return None;
        }
        let from_history = self
            .history
            .iter()
            .map(|entry| mode::read_history(entry))
            .find(|(mode, text)| {
                *mode == self.mode && text.len() > self.input.len() && text.starts_with(&self.input)
            })
            .map(|(_, text)| text[self.input.len()..].to_owned());
        from_history.or_else(|| complete::suggestion(&word, &self.input, &self.listing))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
