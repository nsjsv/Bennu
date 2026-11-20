//! 服务菜单的 `Exec`。语义同 KIO（`DesktopExecParser` 和 `KMacroExpanderBase::expandMacrosShellQuote`）：
//! 整行是一条 shell 命令，字段码按它所在的引号上下文展开，没有文件字段码时末尾补 `%f`，
//! 只有单数的文件字段码时每项启动一次。
//!
//! 和 KIO 不同的是，值从不拼进命令文字里。字段码换成对 `sh` 位置参数的引用（`"${1}"`），
//! 值作为 `sh -c` 后面的参数传进去。参数展开的结果不会再被解析，所以第一层 shell
//! 不会把文件名当成代码；在各种引号里得到的字符串和 KIO 一样。
//!
//! 第二层就管不到了：字段码写在一段带空格、要交给别的程序当命令行的引号里时
//! （`bash -c "mpv \"%f\""`），那个程序会再解析一遍。这时按 `handed_on` 的规则检查路径，
//! 不安全就整次不启动。

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use super::super::handed_on::unsafe_character;
pub(crate) use super::super::handed_on::UnsafeName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecTemplate {
    pieces: Vec<Piece>,
    /// 没有复数的文件字段码：选了几项就启动几次，每次一项。
    per_item: bool,
    /// 第一个词是普通的程序名或路径时记下它，启动前检查它在不在（同 KIO；
    /// 整行以变量赋值、引号、`$` 这些开头时跳过）。
    program: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Field(Field),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    code: FieldCode,
    quoting: Quoting,
    /// 所在的引号里还有带空白的文字：这段引号是交给别的程序的命令行。
    handed_on: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldCode {
    /// `%f` `%u`：本地路径。
    Path,
    /// `%F` `%U`
    Paths,
    /// `%d`：所在的文件夹，带结尾的 `/`（同 KIO）。
    Folder,
    /// `%D`
    Folders,
    /// `%n`：文件名。
    Name,
    /// `%N`
    Names,
    /// `%c`
    ActionName,
    /// `%i`：`--icon <图标>`，没有图标时什么都没有。
    Icon,
    /// `%k`
    DesktopFile,
}

impl FieldCode {
    fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'f' | 'u' => Self::Path,
            'F' | 'U' => Self::Paths,
            'd' => Self::Folder,
            'D' => Self::Folders,
            'n' => Self::Name,
            'N' => Self::Names,
            'c' => Self::ActionName,
            'i' => Self::Icon,
            'k' => Self::DesktopFile,
            _ => return None,
        })
    }

    fn is_plural(self) -> bool {
        matches!(self, Self::Paths | Self::Folders | Self::Names)
    }

    /// 值来自文件名，可能是别人起的；`%c %k %i` 来自可信的服务菜单文件本身。
    fn comes_from_names(self) -> bool {
        !matches!(self, Self::ActionName | Self::Icon | Self::DesktopFile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    Plain,
    Double,
    Single,
    /// `$'…'`
    DollarSingle,
}

/// 扫描时的嵌套层次，同 KIO 的状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nesting {
    Single,
    Double,
    DollarSingle,
    /// `$(…)`、`(…)`：里面重新算作不在引号里。
    Paren,
    /// 反引号：同上。
    Backtick,
    /// `$((…))`
    Math,
    /// `${…}`、`{…}`：继承外面是不是在双引号里。
    Brace,
}

struct Frame {
    nesting: Nesting,
    has_blank: bool,
    fields: Vec<usize>,
}

/// 写错的 Exec（已经是给用户看的话）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecMistake(pub(crate) String);

/// 启动时要用到的、来自服务菜单文件本身的值。
pub(crate) struct ExecValues<'a> {
    pub(crate) name: &'a str,
    pub(crate) icon: &'a str,
    pub(crate) desktop_file: &'a Path,
}

/// 一次启动：`/bin/sh -c <script> sh <arguments…>`，在 `directory` 里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellLaunch {
    pub(crate) script: String,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) directory: PathBuf,
}

impl ExecTemplate {
    pub(crate) fn parse(exec: &str) -> Result<Self, ExecMistake> {
        let unbalanced = || ExecMistake(tr!("The quotes or brackets in Exec do not match"));
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut stack: Vec<Frame> = Vec::new();
        let mut has_file_code = false;
        let mut characters = exec.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '%' {
                let Some(letter) = characters.next() else {
                    return Err(ExecMistake(tr!("Exec ends with a lone %")));
                };
                if letter == '%' {
                    text.push('%');
                    continue;
                }
                // %v（设备）和 %m（小图标）已经废弃，KIO 也展开成空。
                if matches!(letter, 'v' | 'm') {
                    has_file_code |= letter == 'v';
                    continue;
                }
                let Some(code) = FieldCode::from_letter(letter) else {
                    // 不认识的原样留下，同 KIO。
                    text.push('%');
                    text.push(letter);
                    continue;
                };
                has_file_code |= code.comes_from_names();
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                let (quoting, owner) = quoting_at(&stack);
                if let Some(owner) = owner {
                    stack[owner].fields.push(pieces.len());
                }
                pieces.push(Piece::Field(Field {
                    code,
                    quoting,
                    handed_on: false,
                }));
                continue;
            }
            text.push(character);
            let top = stack.last().map(|frame| frame.nesting);
            if top == Some(Nesting::Single) {
                if character == '\'' {
                    close(&mut stack, &mut pieces);
                } else if character.is_whitespace() {
                    stack.last_mut().expect("top exists").has_blank = true;
                }
                continue;
            }
            if character == '\\' {
                // 反斜杠吞掉下一个字符（同 KIO），被转义的 % 也不展开。
                if let Some(escaped) = characters.next() {
                    text.push(escaped);
                }
                continue;
            }
            if top == Some(Nesting::DollarSingle) {
                if character == '\'' {
                    close(&mut stack, &mut pieces);
                } else if character.is_whitespace() {
                    stack.last_mut().expect("top exists").has_blank = true;
                }
                continue;
            }
            let in_double = quoting_at(&stack).0 == Quoting::Double;
            match character {
                '$' => {
                    let Some(&next) = characters.peek() else {
                        continue;
                    };
                    // 和 KIO 一样，`$` 后面那个字符一起吞掉，不当字段码看。
                    text.push(next);
                    characters.next();
                    match next {
                        '(' if characters.peek() == Some(&'(') => {
                            text.push('(');
                            characters.next();
                            open(&mut stack, Nesting::Math);
                        }
                        '(' => open(&mut stack, Nesting::Paren),
                        '{' => open(&mut stack, Nesting::Brace),
                        '\'' if !in_double => open(&mut stack, Nesting::DollarSingle),
                        '"' if !in_double => open(&mut stack, Nesting::Double),
                        _ => {}
                    }
                }
                '`' if top == Some(Nesting::Backtick) => close(&mut stack, &mut pieces),
                '`' => open(&mut stack, Nesting::Backtick),
                '"' if top == Some(Nesting::Double) => close(&mut stack, &mut pieces),
                _ if top == Some(Nesting::Double) => {}
                '\'' if !in_double => open(&mut stack, Nesting::Single),
                '"' if !in_double => open(&mut stack, Nesting::Double),
                ')' => match top {
                    Some(Nesting::Paren) => close(&mut stack, &mut pieces),
                    Some(Nesting::Math) if characters.peek() == Some(&')') => {
                        text.push(')');
                        characters.next();
                        close(&mut stack, &mut pieces);
                    }
                    // 原来是 `$( (…)`：里面那层括号关上了，剩下外面的 `$(`。
                    Some(Nesting::Math) => {
                        stack.last_mut().expect("top exists").nesting = Nesting::Paren;
                    }
                    _ => return Err(unbalanced()),
                },
                '}' if top == Some(Nesting::Brace) => close(&mut stack, &mut pieces),
                '(' => open(&mut stack, Nesting::Paren),
                '{' => open(&mut stack, Nesting::Brace),
                _ => {}
            }
            if character.is_whitespace() {
                if let (_, Some(owner)) = quoting_at(&stack) {
                    stack[owner].has_blank = true;
                }
            }
        }
        if !stack.is_empty() {
            return Err(unbalanced());
        }
        if !text.is_empty() {
            pieces.push(Piece::Text(text));
        }
        // 一个文件字段码都没写：同 KIO，当作末尾有 %f。
        if !has_file_code {
            pieces.push(Piece::Text(" ".to_owned()));
            pieces.push(Piece::Field(Field {
                code: FieldCode::Path,
                quoting: Quoting::Plain,
                handed_on: false,
            }));
        }
        let per_item = !pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Field(field) if field.code.is_plural()));
        Ok(Self {
            pieces,
            per_item,
            program: plain_program(exec),
        })
    }

    pub(crate) fn program(&self) -> Option<&str> {
        self.program.as_deref()
    }

    /// 对这些项要启动的进程。`items` 不为空。
    pub(crate) fn launches(
        &self,
        items: &[PathBuf],
        values: &ExecValues<'_>,
    ) -> Result<Vec<ShellLaunch>, UnsafeName> {
        if self.per_item {
            items
                .iter()
                .map(|item| self.launch(std::slice::from_ref(item), values))
                .collect()
        } else {
            Ok(vec![self.launch(items, values)?])
        }
    }

    fn launch(
        &self,
        items: &[PathBuf],
        values: &ExecValues<'_>,
    ) -> Result<ShellLaunch, UnsafeName> {
        let mut script = String::new();
        let mut arguments = Vec::new();
        for piece in &self.pieces {
            let field = match piece {
                Piece::Text(text) => {
                    script.push_str(text);
                    continue;
                }
                Piece::Field(field) => field,
            };
            let field_values = field_values(field.code, items, values);
            if field.handed_on && field.code.comes_from_names() {
                for (item, value) in &field_values {
                    if let Some(character) = unsafe_character(value) {
                        return Err(UnsafeName {
                            path: item.to_path_buf(),
                            character,
                        });
                    }
                }
            }
            let references = field_values
                .into_iter()
                .map(|(_, value)| {
                    arguments.push(value);
                    format!("${{{}}}", arguments.len())
                })
                .collect::<Vec<_>>();
            if references.is_empty() {
                continue;
            }
            match field.quoting {
                Quoting::Plain => {
                    let quoted = references
                        .iter()
                        .map(|reference| format!("\"{reference}\""))
                        .collect::<Vec<_>>();
                    script.push_str(&quoted.join(" "));
                }
                Quoting::Double => script.push_str(&references.join(" ")),
                // 先关上单引号，放进双引号里的引用，再打开。
                Quoting::Single => script.push_str(&format!("'\"{}\"'", references.join(" "))),
                Quoting::DollarSingle => {
                    script.push_str(&format!("'\"{}\"$'", references.join(" ")));
                }
            }
        }
        // 同 KIO：在第一项所在的文件夹里启动。
        let directory = items[0]
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("/"))
            .to_path_buf();
        Ok(ShellLaunch {
            script,
            arguments,
            directory,
        })
    }
}

/// 字段码展开成的值，每个值带着它来自哪一项（拦截时报出来）。单数的字段码在一次启动里
/// 有多项时什么都不给（同 KIO 的 `KRunMX2`）。
fn field_values<'a>(
    code: FieldCode,
    items: &'a [PathBuf],
    values: &'a ExecValues<'_>,
) -> Vec<(&'a Path, OsString)> {
    let per_item = |value: fn(&Path) -> OsString| -> Vec<(&'a Path, OsString)> {
        let chosen: &[PathBuf] = if code.is_plural() || items.len() == 1 {
            items
        } else {
            &[]
        };
        chosen
            .iter()
            .map(|item| (item.as_path(), value(item)))
            .collect()
    };
    match code {
        FieldCode::Path | FieldCode::Paths => per_item(|item| item.as_os_str().to_owned()),
        FieldCode::Folder | FieldCode::Folders => per_item(folder_with_slash),
        FieldCode::Name | FieldCode::Names => {
            per_item(|item| item.file_name().unwrap_or_default().to_owned())
        }
        FieldCode::ActionName => vec![(values.desktop_file, values.name.into())],
        FieldCode::Icon if values.icon.is_empty() => Vec::new(),
        FieldCode::Icon => vec![
            (values.desktop_file, "--icon".into()),
            (values.desktop_file, values.icon.into()),
        ],
        FieldCode::DesktopFile => vec![(values.desktop_file, values.desktop_file.into())],
    }
}

fn folder_with_slash(item: &Path) -> OsString {
    let mut folder = item
        .parent()
        .unwrap_or(Path::new("/"))
        .as_os_str()
        .to_owned()
        .into_vec();
    if folder.last() != Some(&b'/') {
        folder.push(b'/');
    }
    OsString::from_vec(folder)
}

/// 这一位置字段码的引号上下文，和管着它的那层引号在栈里的位置。
fn quoting_at(stack: &[Frame]) -> (Quoting, Option<usize>) {
    for (index, frame) in stack.iter().enumerate().rev() {
        match frame.nesting {
            Nesting::Single => return (Quoting::Single, Some(index)),
            Nesting::Double => return (Quoting::Double, Some(index)),
            Nesting::DollarSingle => return (Quoting::DollarSingle, Some(index)),
            Nesting::Paren | Nesting::Backtick | Nesting::Math => return (Quoting::Plain, None),
            Nesting::Brace => {}
        }
    }
    (Quoting::Plain, None)
}

fn open(stack: &mut Vec<Frame>, nesting: Nesting) {
    stack.push(Frame {
        nesting,
        has_blank: false,
        fields: Vec::new(),
    });
}

/// 关上最内层。关上的是一段带空白的引号时，里面的字段码都标成"交给别的程序"。
fn close(stack: &mut Vec<Frame>, pieces: &mut [Piece]) {
    let frame = stack.pop().expect("only called with an open frame");
    if !frame.has_blank {
        return;
    }
    for index in frame.fields {
        if let Piece::Field(field) = &mut pieces[index] {
            field.handed_on = true;
        }
    }
}

fn plain_program(exec: &str) -> Option<String> {
    let line = exec.trim_start();
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let word = &line[..end];
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/+-".contains(&byte));
    plain.then(|| word.to_owned())
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
