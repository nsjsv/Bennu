//! 管道：`挑文件 | 一步 | 一步`，像 shell。只在搜索模式和文件模式里认（命令模式里 `|` 是 shell 的）。
//!
//! - `|` 前面挑文件：什么都不写是选中的项；搜索模式里写的是当前文件夹里名字的通配（`*.mp4`，同
//!   shell）；文件模式里是索引查询（写法和筛选同 `f:`）。
//! - `|` 后面每段是一步：已有动作的名字（它的各步原样接进来），或者动作文件里一步的写法，中英都认
//!   （`转换 m4a`、`move_to ~/备份`）。回收站、服务菜单只能是最后一段。
//!
//! 这里只看字；挑文件、执行在应用层。

use crate::actions::{read_step, StepKey};
use crate::actions::{ActionId, ActionStep};

use super::find::{self, FindQuery};
use super::mode::fold;
use super::BoxMode;

/// 管道里能写的一个动作：名字对上就是它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PipeAction {
    pub(crate) name: String,
    pub(crate) id: ActionId,
    /// 它后面不能再接：服务菜单（只启动，不交结果），或者最后一步是移到回收站。
    pub(crate) ends_the_pipe: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PipeSource {
    Selection,
    /// 当前文件夹里名字匹配这个通配的。
    Glob(String),
    Find(Box<FindQuery>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stage {
    Step(ActionStep),
    Action(ActionId),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pipe {
    pub(crate) source: PipeSource,
    pub(crate) stages: Vec<Stage>,
    /// 每段的说明，预览里接在一起。
    pub(crate) labels: Vec<String>,
}

/// 一步的写法：中文、英文（同动作文件的键），没写值时用的值。
struct StepWord {
    words: [&'static str; 2],
    /// `copy_paths` 没有值，是 None。
    key: Option<StepKey>,
    default: Option<&'static str>,
}

const STEP_WORDS: [StepWord; 8] = [
    StepWord {
        words: ["转换", "convert"],
        key: Some(StepKey::Convert),
        default: None,
    },
    StepWord {
        words: ["压缩", "compress"],
        key: Some(StepKey::Compress),
        default: None,
    },
    StepWord {
        words: ["解压", "extract"],
        key: Some(StepKey::Extract),
        default: Some("here"),
    },
    StepWord {
        words: ["复制到", "copy_to"],
        key: Some(StepKey::CopyTo),
        default: None,
    },
    StepWord {
        words: ["移动到", "move_to"],
        key: Some(StepKey::MoveTo),
        default: None,
    },
    StepWord {
        words: ["回收站", "trash"],
        key: Some(StepKey::Trash),
        default: Some("current"),
    },
    StepWord {
        words: ["执行", "run"],
        key: Some(StepKey::Run),
        default: None,
    },
    StepWord {
        words: ["复制路径", "copy_paths"],
        key: None,
        default: None,
    },
];

/// 后面跟着文件夹的步骤：Tab 在它们后面补路径。
pub(crate) fn takes_a_folder(verb: &str) -> bool {
    STEP_WORDS
        .iter()
        .filter(|word| matches!(word.key, Some(StepKey::CopyTo | StepKey::MoveTo)))
        .any(|word| word.words.contains(&verb))
}

/// 补全用的：步骤的写法。
pub(crate) fn step_words() -> impl Iterator<Item = &'static str> {
    STEP_WORDS.iter().flat_map(|word| word.words)
}

/// 有不在引号里的 `|` 才是管道（None 不是）；写错时是给用户看的原因。
pub(crate) fn read_pipe(
    mode: BoxMode,
    text: &str,
    actions: &[PipeAction],
) -> Option<Result<Pipe, String>> {
    if !matches!(mode, BoxMode::Search | BoxMode::Files) {
        return None;
    }
    let parts = split_unquoted(text);
    if parts.len() < 2 {
        return None;
    }
    Some(read_parts(mode, &parts, actions))
}

/// 只读挑法：后面的步骤还没写完时也先挑起来，写完时项已经挑好了。
pub(crate) fn read_pipe_source(mode: BoxMode, text: &str) -> Option<Result<PipeSource, String>> {
    if !matches!(mode, BoxMode::Search | BoxMode::Files) {
        return None;
    }
    let parts = split_unquoted(text);
    (parts.len() >= 2).then(|| read_source(mode, parts[0].trim()))
}

/// 最后一个不在引号里的 `|` 后面从哪儿开始（字节）。
pub(crate) fn last_stage_start(text: &str) -> Option<usize> {
    let parts = split_unquoted(text);
    (parts.len() >= 2).then(|| text.len() - parts[parts.len() - 1].len())
}

fn read_source(mode: BoxMode, text: &str) -> Result<PipeSource, String> {
    match mode {
        BoxMode::Files => {
            let query = find::read_find_query(text)?;
            if query.terms.is_empty() {
                return Err(tr!("Type what to look for; filters only narrow it down"));
            }
            Ok(PipeSource::Find(Box::new(query)))
        }
        _ if text.is_empty() => Ok(PipeSource::Selection),
        _ if text.contains('/') => Err(tr!(
            "Only names in this folder can be matched here; start with f: to look further"
        )),
        _ => Ok(PipeSource::Glob(text.to_owned())),
    }
}

fn read_parts(mode: BoxMode, parts: &[&str], actions: &[PipeAction]) -> Result<Pipe, String> {
    let source = read_source(mode, parts[0].trim())?;
    let last = parts.len() - 2;
    let mut stages = Vec::new();
    let mut labels = Vec::new();
    for (index, part) in parts[1..].iter().enumerate() {
        let text = part.trim();
        if text.is_empty() {
            return Err(tr!("Write a step after each |"));
        }
        let (stage, label, ends_the_pipe) = read_stage(text, actions)?;
        if ends_the_pipe && index != last {
            return Err(tr!("“%1” has to be the last step", text));
        }
        stages.push(stage);
        labels.push(label);
    }
    Ok(Pipe {
        source,
        stages,
        labels,
    })
}

/// 一段：先认动作的名字（整段对上），再认步骤的写法。
fn read_stage(text: &str, actions: &[PipeAction]) -> Result<(Stage, String, bool), String> {
    let folded = fold(text);
    if let Some(action) = actions.iter().find(|action| fold(&action.name) == folded) {
        return Ok((
            Stage::Action(action.id.clone()),
            action.name.clone(),
            action.ends_the_pipe,
        ));
    }
    let (verb, value) = match text.split_once(char::is_whitespace) {
        Some((verb, value)) => (verb, value.trim()),
        None => (text, ""),
    };
    let Some(word) = STEP_WORDS.iter().find(|word| word.words.contains(&verb)) else {
        return Err(tr!(
            "Unknown step “%1”. Write an action's name, or one of: %2",
            text,
            STEP_WORDS
                .iter()
                .map(|word| word.words[0])
                .collect::<Vec<_>>()
                .join(" ")
        ));
    };
    let step = match word.key {
        Some(key) => read_step(key, value_or(value, word.default))?,
        None if value.is_empty() => ActionStep::CopyPaths,
        None => return Err(tr!("“%1” takes nothing after it", verb)),
    };
    let ends_the_pipe = matches!(step, ActionStep::Trash(_));
    Ok((Stage::Step(step.clone()), step.label(), ends_the_pipe))
}

fn value_or<'a>(value: &'a str, default: Option<&'a str>) -> &'a str {
    match default {
        Some(default) if value.is_empty() => default,
        _ => value,
    }
}

/// 按不在引号里的 `|` 切开。引号和反斜杠的规则同 shell：单引号里什么都不转义。
fn split_unquoted(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut escaping = false;
    for (at, character) in text.char_indices() {
        if escaping {
            escaping = false;
            continue;
        }
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaping = true,
            (None, '\'' | '"') => quote = Some(character),
            (None, '|') => {
                parts.push(&text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

#[cfg(test)]
#[path = "pipe_tests.rs"]
mod tests;
