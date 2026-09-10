#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    General,
    Appearance,
    Files,
    Search,
    Shortcuts,
    Logs,
    About,
}

impl SettingsCategory {
    pub(crate) const ALL: [Self; 7] = [
        Self::General,
        Self::Appearance,
        Self::Files,
        Self::Search,
        Self::Shortcuts,
        Self::Logs,
        Self::About,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Files => "Files",
            Self::Search => "Search",
            Self::Shortcuts => "Shortcuts",
            Self::Logs => "Logs",
            Self::About => "About",
        }
    }
}

/// 设置详情区的二级页面；一级仍是 SettingsCategory 分类页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSubpage {
    Preview,
}
