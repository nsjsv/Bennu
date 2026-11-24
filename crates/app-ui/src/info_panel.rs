//! 信息面板显示哪些字段、跟不跟鼠标悬停：用户在面板右键菜单里选，存盘。

/// 信息面板里的一行（详细信息是一整组）。顺序就是面板里的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InfoField {
    Type,
    Size,
    DiskSize,
    Location,
    LinkTarget,
    Modified,
    Created,
    Accessed,
    StatusChanged,
    Permissions,
    Owner,
    /// 图片尺寸、相机、拍摄时间；音视频时长、编码、分辨率、码率。
    Details,
    /// PDF 页数。
    Pages,
}

impl InfoField {
    pub(crate) const ALL: [Self; 13] = [
        Self::Type,
        Self::Size,
        Self::DiskSize,
        Self::Location,
        Self::LinkTarget,
        Self::Modified,
        Self::Created,
        Self::Accessed,
        Self::StatusChanged,
        Self::Permissions,
        Self::Owner,
        Self::Details,
        Self::Pages,
    ];

    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Size => "size",
            Self::DiskSize => "disk_size",
            Self::Location => "location",
            Self::LinkTarget => "link_target",
            Self::Modified => "modified",
            Self::Created => "created",
            Self::Accessed => "accessed",
            Self::StatusChanged => "status_changed",
            Self::Permissions => "permissions",
            Self::Owner => "owner",
            Self::Details => "details",
            Self::Pages => "pages",
        }
    }

    pub(crate) fn from_storage_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|field| field.storage_name() == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InfoPanelPreferences {
    /// 用户关掉的字段，按 [`InfoField::ALL`] 的顺序。
    hidden_fields: Vec<InfoField>,
    /// 指针停在某个条目上时临时显示它（Dolphin 的默认）。
    pub(crate) follow_hover: bool,
}

impl Default for InfoPanelPreferences {
    /// 窄栏里最常用的在前面：占用空间、访问时间、状态改变时间默认不显示，右键勾出来。
    fn default() -> Self {
        Self {
            hidden_fields: vec![
                InfoField::DiskSize,
                InfoField::Accessed,
                InfoField::StatusChanged,
            ],
            follow_hover: true,
        }
    }
}

impl InfoPanelPreferences {
    pub(crate) fn new(
        hidden_fields: impl IntoIterator<Item = InfoField>,
        follow_hover: bool,
    ) -> Self {
        let hidden: Vec<InfoField> = hidden_fields.into_iter().collect();
        Self {
            hidden_fields: InfoField::ALL
                .into_iter()
                .filter(|field| hidden.contains(field))
                .collect(),
            follow_hover,
        }
    }

    pub(crate) fn hidden_fields(&self) -> &[InfoField] {
        &self.hidden_fields
    }

    pub(crate) fn shows(&self, field: InfoField) -> bool {
        !self.hidden_fields.contains(&field)
    }

    pub(crate) fn toggle(&mut self, field: InfoField) {
        let hidden = self
            .hidden_fields
            .iter()
            .copied()
            .filter(|hidden| *hidden != field)
            .collect::<Vec<_>>();
        *self = if self.shows(field) {
            Self::new(hidden.into_iter().chain([field]), self.follow_hover)
        } else {
            Self::new(hidden, self.follow_hover)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_a_field_twice_gives_back_the_same_preferences() {
        let mut preferences = InfoPanelPreferences::default();
        let before = preferences.clone();
        preferences.toggle(InfoField::Accessed);
        assert!(preferences.shows(InfoField::Accessed));
        preferences.toggle(InfoField::Owner);
        assert!(!preferences.shows(InfoField::Owner));
        preferences.toggle(InfoField::Owner);
        preferences.toggle(InfoField::Accessed);
        assert_eq!(preferences, before);
    }

    #[test]
    fn hidden_fields_keep_the_panel_order_and_storage_names_roundtrip() {
        let preferences =
            InfoPanelPreferences::new([InfoField::Owner, InfoField::Type, InfoField::Owner], false);
        assert_eq!(
            preferences.hidden_fields(),
            &[InfoField::Type, InfoField::Owner]
        );
        for field in InfoField::ALL {
            assert_eq!(
                InfoField::from_storage_name(field.storage_name()),
                Some(field)
            );
        }
    }
}
