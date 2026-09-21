use file_core::{SortDirection, SortField};

pub(crate) const LIST_COLUMN_MIN_WIDTH: f32 = 72.0;
pub(crate) const LIST_COLUMN_MAX_WIDTH: f32 = 520.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ListColumnKind {
    Name,
    Modified,
    Size,
    Kind,
    Owner,
    Group,
    Permissions,
    Accessed,
    Created,
}

impl ListColumnKind {
    pub(crate) const ALL: [Self; 9] = [
        Self::Name,
        Self::Modified,
        Self::Size,
        Self::Kind,
        Self::Owner,
        Self::Group,
        Self::Permissions,
        Self::Accessed,
        Self::Created,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified Time",
            Self::Size => "Size",
            Self::Kind => "Kind",
            Self::Owner => "Owner",
            Self::Group => "Group",
            Self::Permissions => "Permissions",
            Self::Accessed => "Accessed Time",
            Self::Created => "Created Time",
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Size => "size",
            Self::Kind => "kind",
            Self::Owner => "owner",
            Self::Group => "group",
            Self::Permissions => "permissions",
            Self::Accessed => "accessed",
            Self::Created => "created",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "modified" => Some(Self::Modified),
            "size" => Some(Self::Size),
            "kind" => Some(Self::Kind),
            "owner" => Some(Self::Owner),
            "group" => Some(Self::Group),
            "permissions" => Some(Self::Permissions),
            "accessed" => Some(Self::Accessed),
            "created" => Some(Self::Created),
            _ => None,
        }
    }

    pub(crate) fn sort_field(self) -> Option<SortField> {
        match self {
            Self::Name => Some(SortField::Name),
            Self::Modified => Some(SortField::Modified),
            Self::Size => Some(SortField::Size),
            Self::Kind => Some(SortField::Kind),
            Self::Owner | Self::Group | Self::Permissions | Self::Accessed | Self::Created => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListColumnConfig {
    pub(crate) kind: ListColumnKind,
    pub(crate) width: f32,
    pub(crate) visible: bool,
}

impl ListColumnConfig {
    pub(crate) fn new(kind: ListColumnKind, width: f32, visible: bool) -> Self {
        Self {
            kind,
            width: normalize_list_column_width_for_kind(kind, width),
            visible: kind == ListColumnKind::Name || visible,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListSortPreference {
    pub(crate) field: SortField,
    pub(crate) direction: SortDirection,
}

impl Default for ListSortPreference {
    fn default() -> Self {
        Self {
            field: SortField::Name,
            direction: SortDirection::Ascending,
        }
    }
}

impl ListSortPreference {
    pub(crate) fn select_column(&mut self, column: ListColumnKind) {
        let Some(field) = column.sort_field() else {
            return;
        };
        if self.field == field {
            match self.direction {
                SortDirection::Ascending => {
                    self.direction = SortDirection::Descending;
                }
                SortDirection::Descending => {
                    *self = Self::default();
                }
            }
        } else {
            self.field = field;
            self.direction = SortDirection::Ascending;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListViewPreferences {
    columns: Vec<ListColumnConfig>,
    sort: ListSortPreference,
}

impl Default for ListViewPreferences {
    fn default() -> Self {
        Self {
            columns: default_columns(),
            sort: ListSortPreference::default(),
        }
    }
}

impl ListViewPreferences {
    pub(crate) fn new(columns: Vec<ListColumnConfig>, sort: ListSortPreference) -> Self {
        Self {
            columns: normalize_columns(columns),
            sort,
        }
    }

    pub(crate) fn columns(&self) -> &[ListColumnConfig] {
        &self.columns
    }

    pub(crate) fn visible_columns(&self) -> impl Iterator<Item = &ListColumnConfig> {
        self.columns.iter().filter(|column| column.visible)
    }

    pub(crate) fn sort(&self) -> ListSortPreference {
        self.sort
    }

    pub(crate) fn select_sort_column(&mut self, column: ListColumnKind) {
        self.sort.select_column(column);
    }

    pub(crate) fn set_column_visible(&mut self, kind: ListColumnKind, visible: bool) {
        if kind == ListColumnKind::Name {
            return;
        }
        if let Some(column) = self.column_mut(kind) {
            column.visible = visible;
        }
        if !visible
            && kind
                .sort_field()
                .is_some_and(|field| self.sort.field == field)
        {
            self.sort = ListSortPreference::default();
        }
    }

    pub(crate) fn set_column_width(&mut self, kind: ListColumnKind, width: f32) {
        if let Some(column) = self.column_mut(kind) {
            column.width = normalize_list_column_width_for_kind(kind, width);
        }
    }

    #[cfg(test)]
    pub(crate) fn move_column_left(&mut self, kind: ListColumnKind) {
        let Some(index) = self.columns.iter().position(|column| column.kind == kind) else {
            return;
        };
        if index > 0 {
            self.columns.swap(index, index - 1);
        }
    }

    #[cfg(test)]
    pub(crate) fn move_column_right(&mut self, kind: ListColumnKind) {
        let Some(index) = self.columns.iter().position(|column| column.kind == kind) else {
            return;
        };
        if index + 1 < self.columns.len() {
            self.columns.swap(index, index + 1);
        }
    }

    pub(crate) fn move_column_to(
        &mut self,
        dragged: ListColumnKind,
        target: ListColumnKind,
    ) -> bool {
        if dragged == target {
            return false;
        }
        let Some(dragged_index) = self
            .columns
            .iter()
            .position(|column| column.kind == dragged)
        else {
            return false;
        };
        let Some(target_index) = self.columns.iter().position(|column| column.kind == target)
        else {
            return false;
        };
        let insert_index = if dragged_index < target_index {
            target_index - 1
        } else {
            target_index
        };
        let dragged_column = self.columns.remove(dragged_index);
        self.columns.insert(insert_index, dragged_column);
        true
    }

    fn column_mut(&mut self, kind: ListColumnKind) -> Option<&mut ListColumnConfig> {
        self.columns.iter_mut().find(|column| column.kind == kind)
    }
}

fn normalize_list_column_width_for_kind(kind: ListColumnKind, width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(LIST_COLUMN_MIN_WIDTH, LIST_COLUMN_MAX_WIDTH)
    } else {
        default_column_width(kind)
    }
}

pub(crate) fn list_column_kind_config_value(kind: ListColumnKind) -> &'static str {
    kind.config_value()
}

pub(crate) fn list_column_kind_from_config_value(value: &str) -> Option<ListColumnKind> {
    ListColumnKind::from_config_value(value)
}

fn normalize_columns(columns: Vec<ListColumnConfig>) -> Vec<ListColumnConfig> {
    let mut normalized = Vec::new();
    for mut column in columns {
        if normalized
            .iter()
            .any(|existing: &ListColumnConfig| existing.kind == column.kind)
        {
            continue;
        }
        column.width = normalize_list_column_width_for_kind(column.kind, column.width);
        if column.kind == ListColumnKind::Name {
            column.visible = true;
        }
        normalized.push(column);
    }

    for kind in ListColumnKind::ALL {
        if !normalized.iter().any(|column| column.kind == kind) {
            normalized.push(ListColumnConfig::new(
                kind,
                default_column_width(kind),
                default_column_visible(kind),
            ));
        }
    }

    normalized
}

fn default_columns() -> Vec<ListColumnConfig> {
    ListColumnKind::ALL
        .into_iter()
        .map(|kind| {
            ListColumnConfig::new(
                kind,
                default_column_width(kind),
                default_column_visible(kind),
            )
        })
        .collect()
}

fn default_column_width(kind: ListColumnKind) -> f32 {
    match kind {
        ListColumnKind::Name => 320.0,
        ListColumnKind::Modified => 168.0,
        ListColumnKind::Size => 96.0,
        ListColumnKind::Kind => 96.0,
        ListColumnKind::Owner => 120.0,
        ListColumnKind::Group => 120.0,
        ListColumnKind::Permissions => 128.0,
        ListColumnKind::Accessed => 168.0,
        ListColumnKind::Created => 168.0,
    }
}

fn default_column_visible(kind: ListColumnKind) -> bool {
    matches!(
        kind,
        ListColumnKind::Name
            | ListColumnKind::Modified
            | ListColumnKind::Size
            | ListColumnKind::Kind
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(preferences: &ListViewPreferences) -> Vec<ListColumnKind> {
        preferences
            .columns()
            .iter()
            .map(|column| column.kind)
            .collect()
    }

    #[test]
    fn default_columns_include_supported_columns_in_expected_order() {
        let preferences = ListViewPreferences::default();

        assert_eq!(kinds(&preferences), ListColumnKind::ALL);
        assert_eq!(
            preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
            ]
        );
    }

    #[test]
    fn normalize_columns_keeps_name_visible_and_fills_missing_columns() {
        let preferences = ListViewPreferences::new(
            vec![
                ListColumnConfig::new(ListColumnKind::Size, 120.0, true),
                ListColumnConfig::new(ListColumnKind::Name, 200.0, false),
                ListColumnConfig::new(ListColumnKind::Size, 400.0, true),
            ],
            ListSortPreference::default(),
        );

        assert_eq!(
            kinds(&preferences),
            vec![
                ListColumnKind::Size,
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Kind,
                ListColumnKind::Owner,
                ListColumnKind::Group,
                ListColumnKind::Permissions,
                ListColumnKind::Accessed,
                ListColumnKind::Created,
            ]
        );
        assert!(preferences
            .columns()
            .iter()
            .find(|column| column.kind == ListColumnKind::Name)
            .is_some_and(|column| column.visible));
    }

    #[test]
    fn non_name_column_visibility_can_toggle() {
        let mut preferences = ListViewPreferences::default();

        preferences.set_column_visible(ListColumnKind::Size, false);
        preferences.set_column_visible(ListColumnKind::Name, false);

        assert!(preferences
            .columns()
            .iter()
            .find(|column| column.kind == ListColumnKind::Size)
            .is_some_and(|column| !column.visible));
        assert!(preferences
            .columns()
            .iter()
            .find(|column| column.kind == ListColumnKind::Name)
            .is_some_and(|column| column.visible));
    }

    #[test]
    fn hiding_current_sort_column_falls_back_to_name_sort() {
        let mut preferences = ListViewPreferences::default();

        preferences.select_sort_column(ListColumnKind::Size);
        preferences.set_column_visible(ListColumnKind::Size, false);

        assert_eq!(preferences.sort().field, SortField::Name);
        assert_eq!(preferences.sort().direction, SortDirection::Ascending);
    }

    #[test]
    fn moving_column_preserves_column_identity() {
        let mut preferences = ListViewPreferences::default();

        preferences.move_column_right(ListColumnKind::Name);
        preferences.move_column_left(ListColumnKind::Kind);

        assert_eq!(
            kinds(&preferences),
            vec![
                ListColumnKind::Modified,
                ListColumnKind::Name,
                ListColumnKind::Kind,
                ListColumnKind::Size,
                ListColumnKind::Owner,
                ListColumnKind::Group,
                ListColumnKind::Permissions,
                ListColumnKind::Accessed,
                ListColumnKind::Created,
            ]
        );
    }

    #[test]
    fn dragging_column_to_target_reorders_columns() {
        let mut preferences = ListViewPreferences::default();

        assert!(preferences.move_column_to(ListColumnKind::Owner, ListColumnKind::Name));

        assert_eq!(
            kinds(&preferences),
            vec![
                ListColumnKind::Owner,
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
                ListColumnKind::Group,
                ListColumnKind::Permissions,
                ListColumnKind::Accessed,
                ListColumnKind::Created,
            ]
        );
        assert!(!preferences.move_column_to(ListColumnKind::Name, ListColumnKind::Name));
    }

    #[test]
    fn dragging_column_to_later_target_inserts_before_target() {
        let mut preferences = ListViewPreferences::default();

        assert!(preferences.move_column_to(ListColumnKind::Name, ListColumnKind::Kind));

        assert_eq!(
            kinds(&preferences),
            vec![
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Name,
                ListColumnKind::Kind,
                ListColumnKind::Owner,
                ListColumnKind::Group,
                ListColumnKind::Permissions,
                ListColumnKind::Accessed,
                ListColumnKind::Created,
            ]
        );
    }

    #[test]
    fn selecting_sort_column_cycles_back_to_default_and_ignores_unsortable_columns() {
        let mut sort = ListSortPreference::default();

        sort.select_column(ListColumnKind::Size);
        assert_eq!(sort.field, SortField::Size);
        assert_eq!(sort.direction, SortDirection::Ascending);

        sort.select_column(ListColumnKind::Size);
        assert_eq!(sort.field, SortField::Size);
        assert_eq!(sort.direction, SortDirection::Descending);

        sort.select_column(ListColumnKind::Size);
        assert_eq!(sort.field, SortField::Name);
        assert_eq!(sort.direction, SortDirection::Ascending);

        sort.select_column(ListColumnKind::Owner);
        assert_eq!(sort.field, SortField::Name);
        assert_eq!(sort.direction, SortDirection::Ascending);
    }
}
