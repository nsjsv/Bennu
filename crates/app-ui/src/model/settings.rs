#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    General,
    Appearance,
    Files,
    Transfer,
    Search,
    Shortcuts,
    Logs,
    About,
}

impl SettingsCategory {
    pub(crate) const ALL: [Self; 8] = [
        Self::General,
        Self::Appearance,
        Self::Files,
        Self::Transfer,
        Self::Search,
        Self::Shortcuts,
        Self::Logs,
        Self::About,
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::General => tr!("General"),
            Self::Appearance => tr!("Appearance"),
            Self::Files => tr!("Files"),
            Self::Transfer => tr!("Transfer"),
            Self::Search => tr!("Search"),
            Self::Shortcuts => tr!("Shortcuts"),
            Self::Logs => tr!("Logs"),
            Self::About => tr!("About"),
        }
    }
}

/// 设置详情区的二级页面；一级仍是 SettingsCategory 分类页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSubpage {
    Preview,
}
