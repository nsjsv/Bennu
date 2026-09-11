use file_operation_store::{
    COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN, COLUMN_WIDTH_ADJUST_MODE_UNIFORM,
};

/// 多栏视图拖动分隔条时的栏宽联动方式:每栏独立调宽,或所有栏保持等宽一起变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnWidthAdjustMode {
    /// 每栏有独立宽度(历史行为)。
    PerColumn,
    /// 所有栏永远等宽,拖任何分隔条整体变宽窄(macOS Finder 式)。
    Uniform,
}

pub(crate) const DEFAULT_COLUMN_WIDTH_ADJUST_MODE: ColumnWidthAdjustMode =
    ColumnWidthAdjustMode::PerColumn;

impl ColumnWidthAdjustMode {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN => Some(Self::PerColumn),
            COLUMN_WIDTH_ADJUST_MODE_UNIFORM => Some(Self::Uniform),
            _ => None,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::PerColumn => COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN,
            Self::Uniform => COLUMN_WIDTH_ADJUST_MODE_UNIFORM,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_round_trip() {
        for mode in [ColumnWidthAdjustMode::PerColumn, ColumnWidthAdjustMode::Uniform] {
            assert_eq!(
                ColumnWidthAdjustMode::from_config_value(mode.config_value()),
                Some(mode)
            );
        }
    }

    #[test]
    fn unknown_config_value_falls_back_through_none() {
        assert_eq!(ColumnWidthAdjustMode::from_config_value("nonsense"), None);
    }
}
