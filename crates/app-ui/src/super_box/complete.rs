//! 像 fish 那样补全：Tab 补框里最后那个词，边打边在后面淡淡地显示建议，→ 接受。路径模式里整段字是
//! 一个路径；命令模式里第一个词是命令名（在 PATH 里找），后面的词是路径，空格这些用反斜杠转义（POSIX
//! shell 和 fish 都认）。光标位置拿不到，补的总是最后那个词。
//!
//! 候选由应用层在后台列（`BoxLookup::List`），这里只看字。

use std::path::{Path, PathBuf};

use super::{targets, BoxMode};

/// 列出来的一个名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
}

/// 后台列出来的、名字开头（不分大小写）对得上的候选，按名字排好。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    /// 列的是哪个文件夹；命令名没有。
    pub(crate) folder: Option<PathBuf>,
    pub(crate) candidates: Vec<Candidate>,
    /// 太多了只留了前面一部分：这时不知道是不是只有一个、共同的开头有多长。
    pub(crate) truncated: bool,
}

/// 到哪里列候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListSource {
    /// PATH 里的可执行文件。
    Programs,
    Folder(PathBuf),
}

/// 框里要补的那个词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    /// 词在框里从哪儿开始（字节）。
    start: usize,
    /// 词里文件夹那一截，原样留着（命令模式里还转义着）；命令名没有。
    folder_text: Option<String>,
    /// 要补的名字已经打了的开头，去掉了转义。
    pub(crate) name: String,
    escaped: bool,
}

pub(crate) fn word(mode: BoxMode, input: &str) -> Option<Word> {
    match mode {
        BoxMode::Path => Some(path_word(input, 0, false)),
        BoxMode::Shell => {
            let start = last_word_start(input);
            let raw = &input[start..];
            if input[..start].trim().is_empty() && !raw.contains('/') {
                return Some(Word {
                    start,
                    folder_text: None,
                    name: unescape(raw),
                    escaped: true,
                });
            }
            Some(path_word(input, start, true))
        }
        BoxMode::Search | BoxMode::Files | BoxMode::New => None,
    }
}

fn path_word(input: &str, start: usize, escaped: bool) -> Word {
    let raw = &input[start..];
    let split = raw.rfind('/').map_or(0, |slash| slash + 1);
    Word {
        start,
        folder_text: Some(raw[..split].to_owned()),
        name: if escaped {
            unescape(&raw[split..])
        } else {
            raw[split..].to_owned()
        },
        escaped,
    }
}

/// 最后一个没转义的空格后面。
fn last_word_start(input: &str) -> usize {
    let mut start = 0;
    let mut escaping = false;
    for (at, character) in input.char_indices() {
        if escaping {
            escaping = false;
        } else if character == '\\' {
            escaping = true;
        } else if character == ' ' {
            start = at + 1;
        }
    }
    start
}

fn unescape(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => plain.extend(characters.next()),
            other => plain.push(other),
        }
    }
    plain
}

/// shell 里有特殊意思的字前面加反斜杠；字母数字（包括中文）和路径里常见的符号不动。
fn escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        if !(character.is_alphanumeric() || "._-+,:@%=/~^".contains(character)) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

impl Word {
    /// 到哪里列候选；相对路径接在当前文件夹后面，`~` 是主目录。没有当前文件夹时列不了。
    pub(crate) fn source(&self, current_dir: Option<&Path>, home: &Path) -> Option<ListSource> {
        let Some(folder_text) = &self.folder_text else {
            return Some(ListSource::Programs);
        };
        let folder = if self.escaped {
            unescape(folder_text)
        } else {
            folder_text.clone()
        };
        if folder.is_empty() {
            return current_dir.map(|current_dir| ListSource::Folder(current_dir.to_path_buf()));
        }
        targets::jump_path(&folder, current_dir, home).map(ListSource::Folder)
    }

    /// 词的名字换成 `name` 后的整个框。
    fn with_name(&self, input: &str, name: &str) -> String {
        let name = if self.escaped {
            escape(name)
        } else {
            name.to_owned()
        };
        let folder = self.folder_text.as_deref().unwrap_or("");
        format!("{}{folder}{name}", &input[..self.start])
    }

    /// 换成这个候选后的整个框：文件夹后面带 `/`；`finished`（只有它一个）时命令模式里文件和命令
    /// 后面再带个空格，接着打下一个词。
    pub(crate) fn with_candidate(
        &self,
        input: &str,
        candidate: &Candidate,
        finished: bool,
    ) -> String {
        let mut text = self.with_name(input, &candidate.name);
        if candidate.is_dir {
            text.push('/');
        } else if finished && self.escaped {
            text.push(' ');
        }
        text
    }
}

/// 名字以 `name` 开头的候选：先要大小写一样的，没有再不管大小写。`.` 开头的只在 `name` 也以 `.`
/// 开头时才算。
pub(crate) fn matching<'a>(name: &str, candidates: &'a [Candidate]) -> Vec<&'a Candidate> {
    let shown = |candidate: &&Candidate| name.starts_with('.') || !candidate.name.starts_with('.');
    let same_case: Vec<_> = candidates
        .iter()
        .filter(shown)
        .filter(|candidate| candidate.name.starts_with(name))
        .collect();
    if !same_case.is_empty() {
        return same_case;
    }
    let folded = name.to_lowercase();
    candidates
        .iter()
        .filter(shown)
        .filter(|candidate| candidate.name.to_lowercase().starts_with(&folded))
        .collect()
}

/// 按一下 Tab 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Completed {
    /// 框里换成这些字。
    Text(String),
    /// 补不动了：列出这些让用户挑。
    Choices(Vec<Candidate>),
    Nothing,
}

/// 只有一个就补全；好几个就补到它们共同的开头；共同的开头没比打了的长就列出来。
pub(crate) fn complete(word: &Word, input: &str, listing: &Listing) -> Completed {
    let matches = matching(&word.name, &listing.candidates);
    match matches.as_slice() {
        [] => Completed::Nothing,
        [only] if !listing.truncated => Completed::Text(word.with_candidate(input, only, true)),
        many => {
            let common = common_start(many.iter().map(|candidate| candidate.name.as_str()));
            if !listing.truncated && common.chars().count() > word.name.chars().count() {
                Completed::Text(word.with_name(input, &common))
            } else {
                Completed::Choices(many.iter().map(|candidate| (*candidate).clone()).collect())
            }
        }
    }
}

fn common_start<'a>(mut names: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = names.next() else {
        return String::new();
    };
    let mut common = first;
    for name in names {
        let shared = common
            .char_indices()
            .zip(name.chars())
            .find(|((_, left), right)| left != right)
            .map_or(common.len().min(name.len()), |((at, _), _)| at);
        common = &common[..shared];
    }
    common.to_owned()
}

/// 候选给的淡色建议：排在最前、大小写也对得上的那个，框里的字后面还差的那截。
pub(crate) fn suggestion(word: &Word, input: &str, listing: &Listing) -> Option<String> {
    if word.name.is_empty() {
        return None;
    }
    let first = listing
        .candidates
        .iter()
        .find(|candidate| candidate.name.starts_with(&word.name))?;
    let full = word.with_candidate(input, first, false);
    full.strip_prefix(input)
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
}

/// Tab 补不动时列出来的候选；再按 Tab 一个个换进框里，打字就收起。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choices {
    word: Word,
    /// 列出来时框里的字：换的时候总是从它换。
    input: String,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) chosen: Option<usize>,
}

impl Choices {
    pub(crate) fn new(word: Word, input: String, candidates: Vec<Candidate>) -> Self {
        Self {
            word,
            input,
            candidates,
            chosen: None,
        }
    }

    /// 换下一个，到头绕回；交回换好后的框。
    pub(crate) fn next(&mut self) -> String {
        let chosen = self
            .chosen
            .map_or(0, |chosen| (chosen + 1) % self.candidates.len());
        self.chosen = Some(chosen);
        self.word
            .with_candidate(&self.input, &self.candidates[chosen], false)
    }
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
