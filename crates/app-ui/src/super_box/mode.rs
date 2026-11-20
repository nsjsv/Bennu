//! 框的模式和前缀。前缀是切换模式的快捷写法：`g:` 切到路径、`n:` 切到新建，前缀本身不留在框里。
//! 历史里存的是“前缀 + 字”，和手打前缀是同一种写法。

/// Tab 按这个顺序往后切，到头绕回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxMode {
    /// 找功能、动作、设置、收藏。
    Search,
    /// 跳到一个路径。
    Path,
    /// 在当前文件夹新建文件或文件夹。
    New,
}

impl BoxMode {
    pub(crate) const ALL: [Self; 3] = [Self::Search, Self::Path, Self::New];

    /// 设置页“默认模式”能选的：新建要先有名字，不适合当打开时的样子。
    pub(crate) const DEFAULT_CHOICES: [Self; 2] = [Self::Search, Self::Path];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Search => tr!("Search"),
            Self::Path => tr!("Path"),
            Self::New => tr!("New"),
        }
    }

    fn prefix(self) -> Option<char> {
        match self {
            Self::Search => None,
            Self::Path => Some('g'),
            Self::New => Some('n'),
        }
    }

    pub(crate) fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub(crate) fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Path => "path",
            Self::New => "new",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.config_value() == value)
    }
}

/// 开头的前缀：一个 ASCII 字母加半角或全角冒号，后面的空白不算。只认得各模式自己的字母。
pub(crate) fn split_prefix(text: &str) -> Option<(BoxMode, &str)> {
    let mut characters = text.char_indices();
    let (_, letter) = characters.next()?;
    let (colon_at, colon) = characters.next()?;
    if colon != ':' && colon != '：' {
        return None;
    }
    let letter = letter.to_ascii_lowercase();
    let mode = BoxMode::ALL
        .into_iter()
        .find(|mode| mode.prefix() == Some(letter))?;
    Some((mode, text[colon_at + colon.len_utf8()..].trim_start()))
}

/// 历史里的一条：模式的前缀 + 字（搜索没有前缀）。
pub(crate) fn history_text(mode: BoxMode, text: &str) -> String {
    match mode.prefix() {
        Some(prefix) => format!("{prefix}:{text}"),
        None => text.to_owned(),
    }
}

/// 一条历史属于哪个模式、框里该显示的字。
pub(crate) fn read_history(entry: &str) -> (BoxMode, &str) {
    split_prefix(entry).unwrap_or((BoxMode::Search, entry))
}

/// 比较用的写法：小写，全角的 ASCII 字符和全角空格换成半角，中文输入法下打出来的也能匹配。
pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => {
                char::from_u32(character as u32 - 0xfee0).unwrap_or(character)
            }
            other => other,
        })
        .flat_map(char::to_lowercase)
        .collect()
}
