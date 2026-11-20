//! 框的模式和标签。标签是切换模式的写法：`f:` 文件、`g:` 路径、`n:` 新建、`r:` 改名、`s:` 命令、
//! `=` 计算、`@` 发送，标签本身不留在框里。
//! 历史里存的是“标签 + 字”，和手打标签是同一种写法。

/// Shift+Tab 按这个顺序往后切，到头绕回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxMode {
    /// 找功能、动作、设置、收藏（最后也列索引里名字对上的文件）。
    Search,
    /// 搜索索引，可以写筛选。
    Files,
    /// 跳到一个路径。
    Path,
    /// 在当前文件夹新建文件或文件夹。
    New,
    /// 给选中的项改名（几项时按模板）。
    Rename,
    /// 交给用户的 shell 执行一条命令。
    Shell,
    /// 算一个式子。
    Calculate,
    /// 把选中的项发给一台设备。
    Send,
}

impl BoxMode {
    pub(crate) const ALL: [Self; 8] = [
        Self::Search,
        Self::Files,
        Self::Path,
        Self::New,
        Self::Rename,
        Self::Shell,
        Self::Calculate,
        Self::Send,
    ];

    /// 设置页“默认模式”能选的：新建要先有名字、改名要先选中，不适合当打开时的样子。
    pub(crate) const DEFAULT_CHOICES: [Self; 4] =
        [Self::Search, Self::Files, Self::Path, Self::Shell];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Search => tr!("Search"),
            Self::Files => tr!("Files"),
            Self::Path => tr!("Path"),
            Self::New => tr!("New"),
            Self::Rename => tr!("Rename"),
            Self::Shell => tr!("Command"),
            Self::Calculate => tr!("Calculate"),
            Self::Send => tr!("Send"),
        }
    }

    /// 打在开头切到这个模式的标签（搜索没有）。
    pub(crate) fn tag(self) -> Option<&'static str> {
        match self {
            Self::Search => None,
            Self::Files => Some("f:"),
            Self::Path => Some("g:"),
            Self::New => Some("n:"),
            Self::Rename => Some("r:"),
            Self::Shell => Some("s:"),
            Self::Calculate => Some("="),
            Self::Send => Some("@"),
        }
    }

    pub(crate) fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Files => "files",
            Self::Path => "path",
            Self::New => "new",
            Self::Rename => "rename",
            Self::Shell => "shell",
            Self::Calculate => "calculate",
            Self::Send => "send",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.config_value() == value)
    }
}

/// 开头的标签，大小写、全角半角都认（中文输入法下打出来的 `：` `＝` `＠`）；后面的空白不算。
pub(crate) fn split_prefix(text: &str) -> Option<(BoxMode, &str)> {
    BoxMode::ALL.into_iter().find_map(|mode| {
        let tag = mode.tag()?;
        let length = text
            .char_indices()
            .nth(tag.len())
            .map_or(text.len(), |(at, _)| at);
        (fold(&text[..length]) == tag).then(|| (mode, text[length..].trim_start()))
    })
}

/// 历史里的一条：模式的标签 + 字（搜索没有标签）。
pub(crate) fn history_text(mode: BoxMode, text: &str) -> String {
    match mode.tag() {
        Some(tag) => format!("{tag}{text}"),
        None => text.to_owned(),
    }
}

/// 一条历史属于哪个模式、框里该显示的字（不带前缀：模式显示成胶囊）。
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
