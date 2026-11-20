//! 路径模板（复制到、移动到）和命令模板（执行命令）。读文件时就解析好：未知变量、开头不对、
//! 命令切不开、`{file}` 和 `{files}` 混用都在那时报错；运行时只把值填进去，从不重新切分，
//! 所以文件名里的空格、引号、`;`、`$()` 都只是参数里的普通字符。只有一个参数本身是交给
//! 别的程序的一行命令时（`sh -c '… {file}'`），那个程序会再解析一遍，见 `handed_on`。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::handed_on::{unsafe_character, UnsafeName};

/// 一次运行里所有模板共用的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunValues {
    /// 动作开始那天，YYYY-MM-DD。
    pub(crate) date: String,
    pub(crate) home: PathBuf,
}

impl RunValues {
    /// 现在开始的一次运行（编辑页的预览也按“现在开始”算）。
    pub(crate) fn starting_now() -> Self {
        Self {
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            home: file_core::app_dirs::home_dir(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variable {
    File,
    Files,
    Name,
    Stem,
    Ext,
    Dir,
    Date,
}

impl Variable {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "file" => Self::File,
            "files" => Self::Files,
            "name" => Self::Name,
            "stem" => Self::Stem,
            "ext" => Self::Ext,
            "dir" => Self::Dir,
            "date" => Self::Date,
            _ => return None,
        })
    }

    /// 按每一项各算一次的变量。
    fn is_per_item(self) -> bool {
        matches!(
            self,
            Self::File | Self::Name | Self::Stem | Self::Ext | Self::Dir
        )
    }

    /// 一项的值。`{dir}` 是这一项所在的文件夹（根目录就是它自己）；`{stem}` / `{ext}` 按最后
    /// 一个点分开，`{ext}` 不带点。
    fn item_value(self, item: &Path) -> OsString {
        match self {
            Self::File => item.as_os_str().to_owned(),
            Self::Name => item.file_name().unwrap_or_default().to_owned(),
            Self::Stem => item.file_stem().unwrap_or_default().to_owned(),
            Self::Ext => item.extension().unwrap_or_default().to_owned(),
            Self::Dir => item.parent().unwrap_or(item).as_os_str().to_owned(),
            Self::Files | Self::Date => OsString::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Segment<V> {
    Text(String),
    Variable(V),
}

/// 模板写错的原因，已经是给用户看的话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateError(pub(crate) String);

/// 把 `{变量}` 切出来。`{{` / `}}` 是字面的大括号。`allowed` 是报错时告诉用户能用哪些。
pub(super) fn parse_segments<V>(
    text: &str,
    variable_named: impl Fn(&str) -> Option<V>,
    allowed: &str,
) -> Result<Vec<Segment<V>>, TemplateError> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(index) = rest.find(['{', '}']) {
        literal.push_str(&rest[..index]);
        let brace = &rest[index..];
        if let Some(after) = brace
            .strip_prefix("{{")
            .or_else(|| brace.strip_prefix("}}"))
        {
            literal.push_str(&brace[..1]);
            rest = after;
            continue;
        }
        if let Some(after) = brace.strip_prefix('}') {
            literal.push('}');
            rest = after;
            continue;
        }
        let Some(end) = brace.find('}') else {
            return Err(TemplateError(tr!(
                "A “{” is not closed (write {{ for a brace)"
            )));
        };
        let name = &brace[1..end];
        let variable = variable_named(name).ok_or_else(|| {
            TemplateError(tr!(
                "{%1} cannot be used here. You can use: %2",
                name,
                allowed
            ))
        })?;
        if !literal.is_empty() {
            segments.push(Segment::Text(std::mem::take(&mut literal)));
        }
        segments.push(Segment::Variable(variable));
        rest = &brace[end + 1..];
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        segments.push(Segment::Text(literal));
    }
    Ok(segments)
}

fn fill(segments: &[Segment<Variable>], item: Option<&Path>, run: &RunValues) -> OsString {
    let mut value = OsString::new();
    for segment in segments {
        match segment {
            Segment::Text(text) => value.push(text),
            Segment::Variable(Variable::Date) => value.push(&run.date),
            Segment::Variable(variable) => {
                if let Some(item) = item {
                    value.push(variable.item_value(item));
                }
            }
        }
    }
    value
}

/// 复制到、移动到的目标文件夹。只能以 `/`、`~/` 或 `{dir}` 开头，所以展开出来总是绝对路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathTemplate {
    text: String,
    from_home: bool,
    segments: Vec<Segment<Variable>>,
}

impl PathTemplate {
    pub(crate) fn parse(text: &str) -> Result<Self, TemplateError> {
        let allowed = "{dir} {name} {stem} {ext} {date}";
        let (from_home, rest) = match text.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => (true, rest),
            _ => (false, text),
        };
        let segments = parse_segments(
            rest,
            |name| {
                Variable::named(name)
                    .filter(|variable| !matches!(variable, Variable::File | Variable::Files))
            },
            allowed,
        )?;
        let starts_well = from_home
            || match segments.first() {
                Some(Segment::Text(text)) => text.starts_with('/'),
                Some(Segment::Variable(variable)) => *variable == Variable::Dir,
                None => false,
            };
        if !starts_well {
            return Err(TemplateError(tr!(
                "The folder must start with /, ~/ or {dir}"
            )));
        }
        Ok(Self {
            text: text.to_owned(),
            from_home,
            segments,
        })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 这一项的目标文件夹。
    pub(crate) fn resolve(&self, item: &Path, run: &RunValues) -> PathBuf {
        let rest = fill(&self.segments, Some(item), run);
        if !self.from_home {
            return PathBuf::from(rest);
        }
        let mut path = run.home.clone().into_os_string();
        path.push(rest);
        PathBuf::from(path)
    }
}

/// 命令的一个参数：`{files}` 单独占一个参数，展开成每一项一个参数；其余是文字和变量拼起来的。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Argument {
    Files,
    Pieces {
        pieces: Vec<Segment<Variable>>,
        /// 文字里有空白、又用了按项算的变量：这个参数是交给别的程序的一行命令
        /// （`sh -c 'ffmpeg -i {file}'`），值要按 `handed_on` 的规则检查。
        handed_on: bool,
    },
}

/// 一条命令执行几次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invocations {
    /// 用了按每一项算的变量：每一项执行一次。
    EachItem,
    /// 用了 `{files}`：所有项一起作为参数执行一次。
    AllItems,
    /// 都没用：执行一次。
    Once,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandTemplate {
    text: String,
    arguments: Vec<Argument>,
    invocations: Invocations,
}

impl CommandTemplate {
    pub(crate) fn parse(text: &str) -> Result<Self, TemplateError> {
        let words = desktop_linux::command_line::split_command_line(text).map_err(|error| {
            TemplateError(tr!("The command cannot be split into arguments: %1", error))
        })?;
        let allowed = "{file} {files} {name} {stem} {ext} {dir} {date}";
        let mut arguments = Vec::with_capacity(words.len());
        for word in &words {
            if word == "{files}" {
                arguments.push(Argument::Files);
                continue;
            }
            let pieces = parse_segments(word, Variable::named, allowed)?;
            if pieces.contains(&Segment::Variable(Variable::Files)) {
                return Err(TemplateError(tr!("{files} must be an argument on its own")));
            }
            let handed_on = uses_item_variable(&pieces)
                && pieces.iter().any(
                    |piece| matches!(piece, Segment::Text(text) if text.contains(char::is_whitespace)),
                );
            arguments.push(Argument::Pieces { pieces, handed_on });
        }
        let uses_files = arguments.contains(&Argument::Files);
        let uses_item = arguments.iter().any(|argument| match argument {
            Argument::Files => false,
            Argument::Pieces { pieces, .. } => uses_item_variable(pieces),
        });
        let invocations = match (uses_item, uses_files) {
            (true, true) => {
                return Err(TemplateError(tr!(
                    "Use either {files} or the per-item variables ({file} {name} {stem} {ext} {dir}), not both"
                )))
            }
            (true, false) => Invocations::EachItem,
            (false, true) => Invocations::AllItems,
            (false, false) => Invocations::Once,
        };
        Ok(Self {
            text: text.to_owned(),
            arguments,
            invocations,
        })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 要执行的每一次：程序和它的参数。有一项的名字不能交给另一层命令行时一次都不执行。
    pub(crate) fn invocations(
        &self,
        items: &[PathBuf],
        run: &RunValues,
    ) -> Result<Vec<Vec<OsString>>, UnsafeName> {
        let invocation = |item: Option<&Path>| {
            let mut words = Vec::with_capacity(self.arguments.len());
            for argument in &self.arguments {
                match argument {
                    Argument::Files => {
                        words.extend(items.iter().map(|item| item.as_os_str().to_owned()))
                    }
                    Argument::Pieces { pieces, handed_on } => {
                        if let (true, Some(item)) = (*handed_on, item) {
                            check_handed_on(pieces, item)?;
                        }
                        words.push(fill(pieces, item, run));
                    }
                }
            }
            Ok(words)
        };
        match self.invocations {
            Invocations::EachItem => items
                .iter()
                .map(|item| invocation(Some(item.as_path())))
                .collect(),
            Invocations::AllItems | Invocations::Once => Ok(vec![invocation(None)?]),
        }
    }
}

fn uses_item_variable(pieces: &[Segment<Variable>]) -> bool {
    pieces
        .iter()
        .any(|piece| matches!(piece, Segment::Variable(variable) if variable.is_per_item()))
}

fn check_handed_on(pieces: &[Segment<Variable>], item: &Path) -> Result<(), UnsafeName> {
    for piece in pieces {
        if let Segment::Variable(variable) = piece {
            if let Some(character) = unsafe_character(&variable.item_value(item)) {
                return Err(UnsafeName {
                    path: item.to_path_buf(),
                    character,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
