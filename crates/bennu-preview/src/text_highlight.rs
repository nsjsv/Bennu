//! 文本预览语法高亮：认语言、按行解析成少量几类记号、按配色上色。
//!
//! 结果必须与「从文件头一路解析下来」一致：块注释、多行字符串跨行时，中间某行的颜色取决于
//! 前面所有行。所以后台从头解析一遍（≤ 10 MB 的文件），每 256 行存一个解析状态；可见行
//! 从它前面最近的状态接着解析。超长行（> 2 万字节，常见于压缩过的代码）不解析、状态不变。

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use iced::Color;
use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};

/// 超过这个大小的文件不高亮（用户定的上限），状态栏注明。
pub const HIGHLIGHT_LIMIT_BYTES: u64 = 10 * 1024 * 1024;
/// 每隔多少行存一个解析状态。
pub(crate) const CHECKPOINT_LINES: usize = 256;
/// 更长的行不解析：正则引擎在几十万字符的一行上会很慢，这种行几乎都是压缩代码。
pub(crate) const MAX_HIGHLIGHT_LINE_BYTES: usize = 20_000;

/// 语法包只在第一次需要高亮时加载（几十毫秒），之后进程内共享。
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// 认出的语言：语法包里的名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightLanguage {
    name: String,
}

impl HighlightLanguage {
    pub fn name(&self) -> &str {
        &self.name
    }

    fn syntax(&self) -> &'static SyntaxReference {
        syntaxes()
            .find_syntax_by_name(&self.name)
            .expect("language comes from the same syntax set")
    }
}

/// 按文件名（Makefile、Dockerfile 这类整名也在语法的扩展名表里）、后缀、首行
/// （`#!/bin/sh`、`-*- mode -*-`）认语言；认不出或只认出纯文本就不高亮。
pub fn detect_language(path: &Path, first_line: &str) -> Option<HighlightLanguage> {
    let set = syntaxes();
    let file_name = path.file_name().and_then(|name| name.to_str());
    let extension = path.extension().and_then(|extension| extension.to_str());
    let syntax = file_name
        .and_then(|name| set.find_syntax_by_extension(name))
        .or_else(|| extension.and_then(|extension| set.find_syntax_by_extension(extension)))
        .or_else(|| {
            extension
                .and_then(|extension| set.find_syntax_by_extension(&extension.to_ascii_lowercase()))
        })
        .or_else(|| set.find_syntax_by_first_line(first_line))?;
    (syntax.name != "Plain Text").then(|| HighlightLanguage {
        name: syntax.name.clone(),
    })
}

/// 围栏代码块标注的语言（`rust`、`Python`、`js`、`rust,ignore`）：按语法名或扩展名认，
/// 不分大小写；认不出或只认出纯文本就不高亮。
pub(crate) fn language_for_fence(label: &str) -> Option<HighlightLanguage> {
    let token = label.split([' ', ',', '{']).next()?;
    let syntax = syntaxes().find_syntax_by_token(token)?;
    (syntax.name != "Plain Text").then(|| HighlightLanguage {
        name: syntax.name.clone(),
    })
}

/// 上色用的记号种类：比语法定义里的作用域粗得多，配色才好统一。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Comment,
    String,
    Escape,
    Number,
    Constant,
    Keyword,
    Type,
    Function,
    Tag,
    Attribute,
    Heading,
    Link,
    Invalid,
    Inserted,
    Deleted,
}

/// 一行文字里的一段记号（行内字节下标）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextToken {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// 某一行开头的解析状态：从这里接着解析，结果与从文件头解析下来一致。
pub(crate) struct HighlightCheckpoint {
    pub(crate) line_start: u64,
    parse: ParseState,
    scopes: ScopeStack,
}

impl HighlightCheckpoint {
    pub(crate) fn at_start(language: &HighlightLanguage, line_start: u64) -> Self {
        Self {
            line_start,
            parse: ParseState::new(language.syntax()),
            scopes: ScopeStack::new(),
        }
    }
}

/// 逐行解析的游标。
pub(crate) struct LineHighlighter {
    parse: ParseState,
    scopes: ScopeStack,
    matcher: &'static KindMatcher,
    /// 带换行的当前行，逐行复用。
    line: String,
}

impl LineHighlighter {
    pub(crate) fn resume(checkpoint: &HighlightCheckpoint) -> Self {
        Self {
            parse: checkpoint.parse.clone(),
            scopes: checkpoint.scopes.clone(),
            matcher: kind_matcher(),
            line: String::new(),
        }
    }

    pub(crate) fn checkpoint(&self, line_start: u64) -> HighlightCheckpoint {
        HighlightCheckpoint {
            line_start,
            parse: self.parse.clone(),
            scopes: self.scopes.clone(),
        }
    }

    /// 只推进解析状态、不出记号：后台从头解析时省掉归类，状态与 `line` 完全相同。
    pub(crate) fn advance(&mut self, text: &str) {
        for (_, operation) in self.operations(text).unwrap_or_default() {
            if self.scopes.apply(&operation).is_err() {
                return;
            }
        }
    }

    /// 解析一行（不含换行），返回记号；超长行或语法出错时不出记号、状态不变。
    pub(crate) fn line(&mut self, text: &str) -> Vec<TextToken> {
        let Some(operations) = self.operations(text) else {
            return Vec::new();
        };
        let mut tokens: Vec<TextToken> = Vec::new();
        let mut position = 0;
        for (offset, operation) in operations {
            let offset = offset.min(text.len());
            if offset > position {
                self.push_token(&mut tokens, position..offset);
            }
            position = position.max(offset);
            if self.scopes.apply(&operation).is_err() {
                return Vec::new();
            }
        }
        if position < text.len() {
            self.push_token(&mut tokens, position..text.len());
        }
        tokens
    }

    fn operations(&mut self, text: &str) -> Option<Vec<(usize, ScopeStackOp)>> {
        if text.len() > MAX_HIGHLIGHT_LINE_BYTES {
            return None;
        }
        // 语法包是「带换行」版本：每行要带上换行符，行尾匹配才对。
        self.line.clear();
        self.line.push_str(text);
        self.line.push('\n');
        self.parse.parse_line(&self.line, syntaxes()).ok()
    }

    fn push_token(&self, tokens: &mut Vec<TextToken>, range: Range<usize>) {
        let Some(kind) = self.matcher.kind(&self.scopes) else {
            return;
        };
        if let Some(last) = tokens.last_mut() {
            if last.kind == kind && last.range.end == range.start {
                last.range.end = range.end;
                return;
            }
        }
        tokens.push(TextToken { range, kind });
    }
}

/// 作用域前缀 → 记号种类；从最内层作用域往外找，同一层按表顺序取第一个匹配
/// （所以 `keyword.operator` 要排在 `keyword` 前面，运算符不按关键字上色）。
struct KindMatcher {
    rules: Vec<(Scope, Option<TokenKind>)>,
}

fn kind_matcher() -> &'static KindMatcher {
    static MATCHER: OnceLock<KindMatcher> = OnceLock::new();
    MATCHER.get_or_init(|| {
        let rules: [(&str, Option<TokenKind>); 32] = [
            ("comment", Some(TokenKind::Comment)),
            ("punctuation.definition.comment", Some(TokenKind::Comment)),
            ("constant.character.escape", Some(TokenKind::Escape)),
            ("string.regexp", Some(TokenKind::Escape)),
            ("string.other.link", Some(TokenKind::Link)),
            ("markup.underline.link", Some(TokenKind::Link)),
            ("string", Some(TokenKind::String)),
            ("markup.raw", Some(TokenKind::String)),
            ("constant.numeric", Some(TokenKind::Number)),
            ("constant", Some(TokenKind::Constant)),
            ("support.constant", Some(TokenKind::Constant)),
            ("keyword.operator", None),
            ("keyword", Some(TokenKind::Keyword)),
            ("storage.type", Some(TokenKind::Keyword)),
            ("storage.modifier", Some(TokenKind::Keyword)),
            ("variable.language", Some(TokenKind::Keyword)),
            ("entity.name.type", Some(TokenKind::Type)),
            ("entity.name.class", Some(TokenKind::Type)),
            ("entity.name.struct", Some(TokenKind::Type)),
            ("entity.name.enum", Some(TokenKind::Type)),
            ("support.type", Some(TokenKind::Type)),
            ("support.class", Some(TokenKind::Type)),
            ("entity.name.function", Some(TokenKind::Function)),
            ("support.function", Some(TokenKind::Function)),
            ("variable.function", Some(TokenKind::Function)),
            ("entity.name.tag", Some(TokenKind::Tag)),
            ("entity.other.attribute-name", Some(TokenKind::Attribute)),
            ("markup.heading", Some(TokenKind::Heading)),
            ("entity.name.section", Some(TokenKind::Heading)),
            ("invalid", Some(TokenKind::Invalid)),
            ("markup.inserted", Some(TokenKind::Inserted)),
            ("markup.deleted", Some(TokenKind::Deleted)),
        ];
        KindMatcher {
            rules: rules
                .into_iter()
                .map(|(prefix, kind)| (Scope::new(prefix).expect("valid scope"), kind))
                .collect(),
        }
    })
}

impl KindMatcher {
    fn kind(&self, scopes: &ScopeStack) -> Option<TokenKind> {
        for scope in scopes.as_slice().iter().rev() {
            if let Some((_, kind)) = self
                .rules
                .iter()
                .find(|(prefix, _)| prefix.is_prefix_of(*scope))
            {
                return *kind;
            }
        }
        None
    }
}

/// 记号配色：浅色、深色各一套，跟随应用的明暗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightPalette {
    #[default]
    Light,
    Dark,
}

impl HighlightPalette {
    /// 跟随应用当前的明暗。
    pub fn for_theme(theme: &iced::Theme) -> Self {
        match bennu_theme::ui_colors(theme).mode {
            bennu_theme::AppearanceMode::Light => Self::Light,
            bennu_theme::AppearanceMode::Dark => Self::Dark,
        }
    }

    /// 搜索匹配的底色：浅黄；深色主题底本来就深，透明度低一点。
    pub fn match_background(self) -> Color {
        match self {
            Self::Light => Color::from_rgba8(250, 204, 21, 0.38),
            Self::Dark => Color::from_rgba8(250, 204, 21, 0.24),
        }
    }

    /// 当前匹配的底色：橙，和其他匹配分得开。
    pub fn current_match_background(self) -> Color {
        match self {
            Self::Light => Color::from_rgba8(249, 115, 22, 0.55),
            Self::Dark => Color::from_rgba8(251, 146, 60, 0.55),
        }
    }

    /// 行内代码的淡底。
    pub fn inline_code_background(self) -> Color {
        match self {
            Self::Light => Color::from_rgba8(0, 0, 0, 0.06),
            Self::Dark => Color::from_rgba8(255, 255, 255, 0.08),
        }
    }

    pub fn color(self, kind: TokenKind) -> Color {
        let hex = match (self, kind) {
            (Self::Light, TokenKind::Comment) => 0x6a737d,
            (Self::Light, TokenKind::String) => 0x1a7f37,
            (Self::Light, TokenKind::Escape | TokenKind::Number | TokenKind::Attribute) => 0xb35900,
            (Self::Light, TokenKind::Constant | TokenKind::Heading) => 0x0550ae,
            (Self::Light, TokenKind::Keyword) => 0xa626a4,
            (Self::Light, TokenKind::Type) => 0x0e7490,
            (Self::Light, TokenKind::Function | TokenKind::Link) => 0x4f46e5,
            (Self::Light, TokenKind::Tag | TokenKind::Invalid | TokenKind::Deleted) => 0xc4302b,
            (Self::Light, TokenKind::Inserted) => 0x1a7f37,
            (Self::Dark, TokenKind::Comment) => 0x8b949e,
            (Self::Dark, TokenKind::String | TokenKind::Inserted) => 0x98c379,
            (Self::Dark, TokenKind::Escape | TokenKind::Number | TokenKind::Attribute) => 0xd19a66,
            (Self::Dark, TokenKind::Constant | TokenKind::Heading | TokenKind::Link) => 0x61afef,
            (Self::Dark, TokenKind::Keyword) => 0xc678dd,
            (Self::Dark, TokenKind::Type) => 0x56b6c2,
            (Self::Dark, TokenKind::Function) => 0x82aaff,
            (Self::Dark, TokenKind::Tag | TokenKind::Invalid | TokenKind::Deleted) => 0xe06c75,
        };
        Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }
}

#[cfg(test)]
#[path = "text_highlight_tests.rs"]
mod tests;
