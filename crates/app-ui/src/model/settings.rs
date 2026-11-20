#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCategory {
    General,
    Appearance,
    Files,
    Transfer,
    Search,
    Shortcuts,
    Actions,
    Logs,
    About,
}

impl SettingsCategory {
    pub(crate) const ALL: [Self; 9] = [
        Self::General,
        Self::Appearance,
        Self::Files,
        Self::Transfer,
        Self::Search,
        Self::Shortcuts,
        Self::Actions,
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
            Self::Actions => tr!("Actions"),
            Self::Logs => tr!("Logs"),
            Self::About => tr!("About"),
        }
    }
}

/// 设置详情区的二级页面；一级仍是 SettingsCategory 分类页。二级页自己的状态放在这里，
/// “在哪一页”和“那一页的状态”只有一个主人。
#[derive(Debug, Clone)]
pub(crate) enum SettingsSubpage {
    Preview,
    /// “动作”分类下编辑一个动作。
    ActionEditor(Box<crate::actions::editor::ActionEditor>),
}
