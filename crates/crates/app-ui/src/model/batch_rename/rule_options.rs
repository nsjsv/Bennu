use std::fmt;

use super::{
    BatchRenameCaseRule, BatchRenameExtensionMode, BatchRenameExtensionRule, BatchRenameInsertMode,
    BatchRenameInsertRule, BatchRenameListRule, BatchRenameRandomMode, BatchRenameRandomRule,
    BatchRenameRegexRule, BatchRenameRemoveClass, BatchRenameRemoveMode, BatchRenameRemoveRule,
    BatchRenameReplaceRule, BatchRenameReplaceScope, BatchRenameSequenceRule, BatchRenameSliceMode,
    BatchRenameSliceRule, BatchRenameSortMode, BatchRenameSortRule,
};

impl Default for BatchRenameSortRule {
    fn default() -> Self {
        Self {
            mode: BatchRenameSortMode::SelectionOrder,
        }
    }
}

impl BatchRenameSortMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![
            Self::SelectionOrder,
            Self::NaturalAscending,
            Self::NameAscending,
            Self::NameDescending,
            Self::ModifiedAscending,
            Self::ModifiedDescending,
            Self::Random,
            Self::ExtensionAscending,
            Self::ExtensionDescending,
            Self::Reverse,
        ]
    }
}

impl fmt::Display for BatchRenameSortMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::SelectionOrder => "Selection order",
            Self::NaturalAscending => "Natural",
            Self::NameAscending => "Name A-Z",
            Self::NameDescending => "Name Z-A",
            Self::ModifiedAscending => "Modified old-new",
            Self::ModifiedDescending => "Modified new-old",
            Self::Random => "Random",
            Self::ExtensionAscending => "Extension A-Z",
            Self::ExtensionDescending => "Extension Z-A",
            Self::Reverse => "Reverse",
        }))
    }
}

impl Default for BatchRenameExtensionRule {
    fn default() -> Self {
        Self {
            mode: BatchRenameExtensionMode::Preserve,
            replacement: String::new(),
        }
    }
}

impl BatchRenameExtensionMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![
            Self::Preserve,
            Self::Remove,
            Self::Replace,
            Self::Lowercase,
            Self::Uppercase,
        ]
    }
}

impl fmt::Display for BatchRenameExtensionMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::Preserve => "Preserve",
            Self::Remove => "Remove",
            Self::Replace => "Replace",
            Self::Lowercase => "lowercase",
            Self::Uppercase => "UPPERCASE",
        }))
    }
}

impl Default for BatchRenameSequenceRule {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            start_input: "1".to_owned(),
            step_input: "1".to_owned(),
            padding_input: "2".to_owned(),
            include_original_stem: true,
            preserve_extension: true,
        }
    }
}

impl Default for BatchRenameReplaceRule {
    fn default() -> Self {
        Self {
            find: String::new(),
            replacement: String::new(),
            scope: BatchRenameReplaceScope::All,
            range_start_input: String::new(),
            range_length_input: String::new(),
            ignore_case: false,
        }
    }
}

impl BatchRenameReplaceScope {
    pub(crate) fn options() -> Vec<Self> {
        vec![Self::All, Self::First, Self::Last, Self::Range]
    }
}

impl fmt::Display for BatchRenameReplaceScope {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::All => "All",
            Self::First => "First",
            Self::Last => "Last",
            Self::Range => "Range",
        }))
    }
}

impl Default for BatchRenameInsertRule {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: BatchRenameInsertMode::Before,
            position_input: "0".to_owned(),
            anchor: String::new(),
            ignore_extension: false,
        }
    }
}

impl BatchRenameInsertMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![Self::Before, Self::After, Self::Position, Self::AfterAnchor]
    }
}

impl fmt::Display for BatchRenameInsertMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::Before => "Before",
            Self::After => "After",
            Self::Position => "Position",
            Self::AfterAnchor => "After text",
        }))
    }
}

impl Default for BatchRenameSliceRule {
    fn default() -> Self {
        Self {
            mode: BatchRenameSliceMode::Position,
            start_input: String::new(),
            length_input: String::new(),
            anchor: String::new(),
        }
    }
}

impl BatchRenameSliceMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![Self::Position, Self::AfterAnchor]
    }
}

impl fmt::Display for BatchRenameSliceMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::Position => "Position",
            Self::AfterAnchor => "After text",
        }))
    }
}

impl Default for BatchRenameRandomRule {
    fn default() -> Self {
        Self {
            mode: BatchRenameRandomMode::Off,
            length_input: "6".to_owned(),
            alphabet: "abcdefghijklmnopqrstuvwxyz0123456789".to_owned(),
        }
    }
}

impl BatchRenameRandomMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![Self::Off, Self::ReplaceStem, Self::Prefix, Self::Suffix]
    }
}

impl fmt::Display for BatchRenameRandomMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::Off => "Off",
            Self::ReplaceStem => "Replace stem",
            Self::Prefix => "Prefix",
            Self::Suffix => "Suffix",
        }))
    }
}

impl Default for BatchRenameRemoveRule {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: BatchRenameRemoveMode::TextAndRange,
            start_input: String::new(),
            length_input: String::new(),
            classes: Vec::new(),
        }
    }
}

impl BatchRenameRemoveMode {
    pub(crate) fn options() -> Vec<Self> {
        vec![Self::TextAndRange, Self::CharacterClasses]
    }
}

impl fmt::Display for BatchRenameRemoveMode {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(match self {
            Self::TextAndRange => "Text / Range",
            Self::CharacterClasses => "Character classes",
        }))
    }
}

impl BatchRenameRemoveClass {
    fn localized_label(self, language: crate::config::UiLanguage) -> String {
        crate::localization::translate(
            language,
            match self {
                Self::Lowercase => "Lowercase",
                Self::Uppercase => "Uppercase",
                Self::Digits => "Digits",
                Self::Symbols => "Symbols",
                Self::Brackets => "Brackets",
                Self::Whitespace => "Whitespace",
                Self::Hanzi => "Hanzi",
            },
        )
        .into_owned()
    }
}

impl fmt::Display for BatchRenameRemoveClass {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&self.localized_label(crate::localization::current_language()))
    }
}

impl Default for BatchRenameListRule {
    fn default() -> Self {
        Self {
            names: String::new(),
        }
    }
}

impl Default for BatchRenameRegexRule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            replacement: String::new(),
        }
    }
}

impl BatchRenameCaseRule {
    pub(crate) fn options() -> Vec<Self> {
        vec![
            Self::Unchanged,
            Self::Lowercase,
            Self::Uppercase,
            Self::TitleCase,
            Self::InvertCase,
        ]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "Keep case",
            Self::Lowercase => "lowercase",
            Self::Uppercase => "UPPERCASE",
            Self::TitleCase => "Title Case",
            Self::InvertCase => "Invert Case",
        }
    }
}

impl fmt::Display for BatchRenameCaseRule {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&crate::localization::translate_current(self.label()))
    }
}

#[cfg(test)]
mod tests {
    use super::BatchRenameRemoveClass;
    use crate::config::UiLanguage;

    #[test]
    fn batch_rename_remove_class_labels_are_localized() {
        for (class, english, chinese) in [
            (BatchRenameRemoveClass::Lowercase, "Lowercase", "小写字母"),
            (BatchRenameRemoveClass::Uppercase, "Uppercase", "大写字母"),
            (BatchRenameRemoveClass::Digits, "Digits", "数字"),
            (BatchRenameRemoveClass::Symbols, "Symbols", "符号"),
            (BatchRenameRemoveClass::Brackets, "Brackets", "括号"),
            (BatchRenameRemoveClass::Whitespace, "Whitespace", "空白字符"),
            (BatchRenameRemoveClass::Hanzi, "Hanzi", "汉字"),
        ] {
            assert_eq!(class.localized_label(UiLanguage::English), english);
            assert_eq!(class.localized_label(UiLanguage::Chinese), chinese);
        }
    }
}
