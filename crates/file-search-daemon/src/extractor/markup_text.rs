//! 网页与 XML 的正文：标签之间的文字。网页常常不是合法 XML，所以不用 XML 解析器，而是
//! 一个宽容的扫描：注释、`<!…>`、`<?…?>` 跳过；`<script>`、`<style>` 整段跳过；CDATA
//! 留下内文；实体解码（认不出的原样留下）；行内标签（`<b>`、`<span>`…）不断词，其他
//! 标签当作空白；连续空白折成一个，有换行时留换行，让摘要读起来像页面上的字。

/// 不断开文字的行内标签：`he<b>ll</b>o` 在页面上是一个词。
const INLINE_TAGS: [&str; 27] = [
    "a", "abbr", "b", "bdi", "bdo", "big", "cite", "code", "data", "del", "dfn", "em", "font", "i",
    "ins", "kbd", "mark", "q", "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u",
];

/// 内容不是文字、整段跳过的元素。
const SKIPPED_ELEMENTS: [&str; 2] = ["script", "style"];

const NAMED_ENTITIES: [(&str, char); 30] = [
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("middot", '·'),
    ("bull", '•'),
    ("deg", '°'),
    ("times", '×'),
    ("divide", '÷'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
    ("sect", '§'),
    ("para", '¶'),
    ("shy", '\u{ad}'),
];

pub(super) fn markup_text(source: &str) -> String {
    let mut text = TextBuilder::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        let Some(offset) = bytes[position..]
            .iter()
            .position(|byte| matches!(byte, b'<' | b'&'))
        else {
            text.push_text(&source[position..]);
            break;
        };
        text.push_text(&source[position..position + offset]);
        position += offset;
        position = if bytes[position] == b'&' {
            let (decoded, next) = entity(source, position);
            match decoded {
                Some(character) => text.push_char(character),
                None => text.push_text(&source[position..next]),
            }
            next
        } else {
            markup(source, position, &mut text)
        };
    }
    text.finish()
}

/// 从 `<` 开始的一段：返回它之后的位置。不像标签的 `<`（如 `a < b`）原样留下。
fn markup(source: &str, start: usize, text: &mut TextBuilder) -> usize {
    let rest = &source[start..];
    if let Some(body) = rest.strip_prefix("<!--") {
        return start + 4 + end_after(body, "-->");
    }
    if let Some(body) = rest.strip_prefix("<![CDATA[") {
        let end = body.find("]]>").unwrap_or(body.len());
        text.push_text(&body[..end]);
        return start + 9 + (end + 3).min(body.len());
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        text.push_break();
        return start + end_after(rest, ">");
    }
    let closing = rest.as_bytes().get(1) == Some(&b'/');
    let name_start = if closing { 2 } else { 1 };
    let name_length = rest[name_start..]
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
        .count();
    if name_length == 0 || !rest.as_bytes()[name_start].is_ascii_alphabetic() {
        text.push_char('<');
        return start + 1;
    }
    let name = rest[name_start..name_start + name_length].to_ascii_lowercase();
    let tag_end = start + tag_length(rest);
    if !INLINE_TAGS.contains(&name.as_str()) {
        text.push_break();
    }
    let self_closing = source[..tag_end].ends_with("/>");
    if !closing && !self_closing && SKIPPED_ELEMENTS.contains(&name.as_str()) {
        let close = format!("</{name}");
        let body = &source[tag_end..];
        return match find_ascii_case_insensitive(body, &close) {
            Some(close_at) => tag_end + close_at + tag_length(&body[close_at..]),
            None => source.len(),
        };
    }
    tag_end
}

/// 标签到 `>` 为止的长度；引号里的 `>` 不算。没有闭合时吃到结尾。
fn tag_length(tag: &str) -> usize {
    let mut quote = None;
    for (index, byte) in tag.bytes().enumerate() {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), _) if byte == open => quote = None,
            (None, b'>') => return index + 1,
            _ => {}
        }
    }
    tag.len()
}

fn end_after(body: &str, terminator: &str) -> usize {
    body.find(terminator)
        .map_or(body.len(), |at| at + terminator.len())
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// `&name;`、`&#123;`、`&#x7b;`：返回解出的字（认不出时 None）和实体之后的位置。
fn entity(source: &str, start: usize) -> (Option<char>, usize) {
    let rest = &source[start + 1..];
    let Some(semicolon) = rest.bytes().take(32).position(|byte| byte == b';') else {
        return (None, start + 1);
    };
    let name = &rest[..semicolon];
    let next = start + 1 + semicolon + 1;
    let decoded = if let Some(number) = name.strip_prefix('#') {
        let value = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse::<u32>().ok(),
        };
        value
            .and_then(char::from_u32)
            .filter(|character| *character != '\0')
    } else {
        NAMED_ENTITIES
            .iter()
            .find(|(entity, _)| *entity == name)
            .map(|(_, character)| *character)
    };
    match decoded {
        Some(character) => (Some(character), next),
        None => (None, start + 1),
    }
}

/// 累积文字并折叠空白：连续空白只留一个，其中有换行就留换行。
struct TextBuilder {
    text: String,
    pending_space: Option<char>,
}

impl TextBuilder {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity),
            pending_space: None,
        }
    }

    fn push_text(&mut self, text: &str) {
        for character in text.chars() {
            self.push_char(character);
        }
    }

    fn push_char(&mut self, character: char) {
        if character.is_whitespace() {
            let separator = if character == '\n' { '\n' } else { ' ' };
            self.pending_space = Some(match self.pending_space {
                Some('\n') => '\n',
                _ => separator,
            });
            return;
        }
        if let Some(space) = self.pending_space.take() {
            if !self.text.is_empty() {
                self.text.push(space);
            }
        }
        self.text.push(character);
    }

    /// 块级标签处断开文字。
    fn push_break(&mut self) {
        if self.pending_space.is_none() {
            self.pending_space = Some(' ');
        }
    }

    fn finish(self) -> String {
        self.text
    }
}

#[cfg(test)]
#[path = "markup_text_tests.rs"]
mod tests;
