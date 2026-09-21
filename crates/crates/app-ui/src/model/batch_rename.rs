use file_core::{BatchRenameItem, DirectoryEntry};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod preview;
mod rule_options;
mod transforms;
use preview::build_batch_rename_preview;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub(crate) enum BatchRenameMessage {
    OpenSelected,
    AddRuleMenuToggled,
    AddRuleSelected(BatchRenameRuleKind),
    RuleSelected(u64),
    RuleEnabledToggled(u64),
    RuleMoved(u64, i32),
    RuleRemoved(u64),
    SortModeSelected(BatchRenameSortMode),
    TemplateChanged(u64, String),
    TemplateTokenMenuToggled(u64),
    TemplateTokenSelected(u64, BatchRenameTemplateToken),
    ExtensionModeSelected(u64, BatchRenameExtensionMode),
    ExtensionReplacementChanged(u64, String),
    SequencePrefixChanged(u64, String),
    SequenceStartChanged(u64, String),
    SequenceStepChanged(u64, String),
    SequencePaddingChanged(u64, String),
    SequenceIncludeOriginalToggled(u64, bool),
    SequencePreserveExtensionToggled(u64, bool),
    ReplaceFindChanged(u64, String),
    ReplaceWithChanged(u64, String),
    ReplaceScopeSelected(u64, BatchRenameReplaceScope),
    ReplaceRangeStartChanged(u64, String),
    ReplaceRangeLengthChanged(u64, String),
    ReplaceIgnoreCaseToggled(u64, bool),
    InsertTextChanged(u64, String),
    InsertPositionChanged(u64, String),
    InsertModeSelected(u64, BatchRenameInsertMode),
    InsertAnchorChanged(u64, String),
    InsertIgnoreExtensionToggled(u64, bool),
    SliceStartChanged(u64, String),
    SliceLengthChanged(u64, String),
    SliceModeSelected(u64, BatchRenameSliceMode),
    SliceAnchorChanged(u64, String),
    CaseSelected(u64, BatchRenameCaseRule),
    RandomModeSelected(u64, BatchRenameRandomMode),
    RandomLengthChanged(u64, String),
    RandomAlphabetChanged(u64, String),
    RemoveTextChanged(u64, String),
    RemoveStartChanged(u64, String),
    RemoveLengthChanged(u64, String),
    RemoveModeSelected(u64, BatchRenameRemoveMode),
    RemoveClassToggled(u64, BatchRenameRemoveClass, bool),
    ListNamesChanged(u64, String),
    RegexPatternChanged(u64, String),
    RegexReplacementChanged(u64, String),
    PreviewNameEditStarted(PathBuf),
    PreviewNameChanged(String),
    PreviewNameEditCommitted,
    PreviewDragStarted(PathBuf),
    PreviewDragEntered(PathBuf),
    PreviewDragFinished,
    Apply,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameRuleKind {
    Template,
    Replace,
    Insert,
    Slice,
    Remove,
    Case,
    Sequence,
    Random,
    Extension,
    Regex,
    List,
}

impl BatchRenameRuleKind {
    pub(crate) const ALL: [Self; 11] = [
        Self::Template,
        Self::Replace,
        Self::Insert,
        Self::Slice,
        Self::Remove,
        Self::Case,
        Self::Sequence,
        Self::Random,
        Self::Extension,
        Self::Regex,
        Self::List,
    ];

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Template => "Template",
            Self::Replace => "Replace",
            Self::Insert => "Insert",
            Self::Slice => "Slice",
            Self::Remove => "Remove characters",
            Self::Case => "Letter case",
            Self::Sequence => "Numbering",
            Self::Random => "Random",
            Self::Extension => "Extension",
            Self::Regex => "Regex",
            Self::List => "List",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BatchRenameRule {
    pub(crate) id: u64,
    pub(crate) enabled: bool,
    pub(crate) params: BatchRenameRuleParams,
}

#[derive(Debug, Clone)]
pub(crate) enum BatchRenameRuleParams {
    Template(BatchRenameTemplateRule),
    Replace(BatchRenameReplaceRule),
    Insert(BatchRenameInsertRule),
    Slice(BatchRenameSliceRule),
    Remove(BatchRenameRemoveRule),
    Case(BatchRenameCaseRule),
    Sequence(BatchRenameSequenceRule),
    Random(BatchRenameRandomRule),
    Extension(BatchRenameExtensionRule),
    Regex(BatchRenameRegexRule),
    List(BatchRenameListRule),
}

impl BatchRenameRuleParams {
    pub(crate) fn kind(&self) -> BatchRenameRuleKind {
        match self {
            Self::Template(_) => BatchRenameRuleKind::Template,
            Self::Replace(_) => BatchRenameRuleKind::Replace,
            Self::Insert(_) => BatchRenameRuleKind::Insert,
            Self::Slice(_) => BatchRenameRuleKind::Slice,
            Self::Remove(_) => BatchRenameRuleKind::Remove,
            Self::Case(_) => BatchRenameRuleKind::Case,
            Self::Sequence(_) => BatchRenameRuleKind::Sequence,
            Self::Random(_) => BatchRenameRuleKind::Random,
            Self::Extension(_) => BatchRenameRuleKind::Extension,
            Self::Regex(_) => BatchRenameRuleKind::Regex,
            Self::List(_) => BatchRenameRuleKind::List,
        }
    }

    pub(crate) fn default_for(kind: BatchRenameRuleKind) -> Self {
        match kind {
            BatchRenameRuleKind::Template => Self::Template(BatchRenameTemplateRule::default()),
            BatchRenameRuleKind::Replace => Self::Replace(BatchRenameReplaceRule::default()),
            BatchRenameRuleKind::Insert => Self::Insert(BatchRenameInsertRule::default()),
            BatchRenameRuleKind::Slice => Self::Slice(BatchRenameSliceRule::default()),
            BatchRenameRuleKind::Remove => Self::Remove(BatchRenameRemoveRule::default()),
            BatchRenameRuleKind::Case => Self::Case(BatchRenameCaseRule::Unchanged),
            BatchRenameRuleKind::Sequence => Self::Sequence(BatchRenameSequenceRule::default()),
            BatchRenameRuleKind::Random => Self::Random(BatchRenameRandomRule::default()),
            BatchRenameRuleKind::Extension => Self::Extension(BatchRenameExtensionRule::default()),
            BatchRenameRuleKind::Regex => Self::Regex(BatchRenameRegexRule::default()),
            BatchRenameRuleKind::List => Self::List(BatchRenameListRule::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameSource {
    pub(crate) path: PathBuf,
    pub(crate) source_name_text: String,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameSourceNameError {
    pub(crate) path: PathBuf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameSortMode {
    SelectionOrder,
    NameAscending,
    NameDescending,
    NaturalAscending,
    ModifiedAscending,
    ModifiedDescending,
    Random,
    ExtensionAscending,
    ExtensionDescending,
    Reverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameExtensionMode {
    Preserve,
    Remove,
    Replace,
    Lowercase,
    Uppercase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameSortRule {
    pub(crate) mode: BatchRenameSortMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameSequenceRule {
    pub(crate) prefix: String,
    pub(crate) start_input: String,
    pub(crate) step_input: String,
    pub(crate) padding_input: String,
    pub(crate) include_original_stem: bool,
    pub(crate) preserve_extension: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameExtensionRule {
    pub(crate) mode: BatchRenameExtensionMode,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameReplaceRule {
    pub(crate) find: String,
    pub(crate) replacement: String,
    pub(crate) scope: BatchRenameReplaceScope,
    pub(crate) range_start_input: String,
    pub(crate) range_length_input: String,
    pub(crate) ignore_case: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameReplaceScope {
    All,
    First,
    Last,
    Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameInsertRule {
    pub(crate) text: String,
    pub(crate) position_input: String,
    pub(crate) mode: BatchRenameInsertMode,
    pub(crate) anchor: String,
    pub(crate) ignore_extension: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameInsertMode {
    Before,
    After,
    Position,
    AfterAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameSliceRule {
    pub(crate) start_input: String,
    pub(crate) length_input: String,
    pub(crate) mode: BatchRenameSliceMode,
    pub(crate) anchor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameSliceMode {
    Position,
    AfterAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameCaseRule {
    Unchanged,
    Lowercase,
    Uppercase,
    TitleCase,
    InvertCase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameRandomRule {
    pub(crate) mode: BatchRenameRandomMode,
    pub(crate) length_input: String,
    pub(crate) alphabet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameRandomMode {
    Off,
    ReplaceStem,
    Prefix,
    Suffix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameRemoveRule {
    pub(crate) text: String,
    pub(crate) start_input: String,
    pub(crate) length_input: String,
    pub(crate) mode: BatchRenameRemoveMode,
    pub(crate) classes: Vec<BatchRenameRemoveClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameRemoveMode {
    TextAndRange,
    CharacterClasses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameRemoveClass {
    Lowercase,
    Uppercase,
    Digits,
    Symbols,
    Brackets,
    Whitespace,
    Hanzi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameListRule {
    pub(crate) names: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameRegexRule {
    pub(crate) pattern: String,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenameTemplateRule {
    pub(crate) template: String,
    pub(crate) token_menu_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenameTemplateToken {
    CurrentName,
    CurrentStem,
    CurrentExtension,
    OriginalName,
    OriginalNameWithoutExtension,
    OriginalExtension,
    Number1,
    Number01,
    Number001,
    Random,
}

impl BatchRenameTemplateToken {
    pub(crate) const ALL: [Self; 10] = [
        Self::CurrentName,
        Self::CurrentStem,
        Self::CurrentExtension,
        Self::OriginalName,
        Self::OriginalNameWithoutExtension,
        Self::OriginalExtension,
        Self::Number1,
        Self::Number01,
        Self::Number001,
        Self::Random,
    ];

    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::CurrentName => "Current name",
            Self::CurrentStem => "Current name without extension",
            Self::CurrentExtension => "Current extension",
            Self::OriginalName => "Original name",
            Self::OriginalNameWithoutExtension => "Name without extension",
            Self::OriginalExtension => "Extension",
            Self::Number1 => "Number 1,2,3",
            Self::Number01 => "Number 01,02,03",
            Self::Number001 => "Number 001,002,003",
            Self::Random => "Random characters",
        }
    }

    pub(crate) fn engine_token(self) -> &'static str {
        match self {
            Self::CurrentName => "{name}",
            Self::CurrentStem => "{stem}",
            Self::CurrentExtension => "{ext}",
            Self::OriginalName => "{original}",
            Self::OriginalNameWithoutExtension => "{original_stem}",
            Self::OriginalExtension => ".{original_ext}",
            Self::Number1 => "{index}",
            Self::Number01 => "{n2}",
            Self::Number001 => "{n3}",
            Self::Random => "{random}",
        }
    }

    pub(crate) fn label(self) -> String {
        format!(
            "[{}]",
            crate::localization::translate_current(self.label_key())
        )
    }

    // 模板串按插入时的界面语言存储，渲染端必须同时识别中英两种标签
    pub(crate) fn localized_labels(self) -> [String; 2] {
        [
            format!("[{}]", self.label_key()),
            format!(
                "[{}]",
                crate::localization::translate(
                    crate::config::UiLanguage::Chinese,
                    self.label_key(),
                )
            ),
        ]
    }
}

impl Default for BatchRenameTemplateRule {
    fn default() -> Self {
        Self {
            template: BatchRenameTemplateToken::OriginalName.label(),
            token_menu_open: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenamePreview {
    pub(crate) rows: Vec<BatchRenamePreviewRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRenamePreviewRow {
    pub(crate) source: PathBuf,
    pub(crate) original_name: String,
    pub(crate) target: PathBuf,
    pub(crate) target_name: String,
    pub(crate) status: BatchRenamePreviewStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRenamePreviewStatus {
    Ready,
    Unchanged,
    DuplicateTarget,
    ExistingTarget,
    EmptyName,
    RuleError,
}

impl BatchRenameState {
    pub(crate) fn new_with_existing_sources(
        items: Vec<BatchRenameSource>,
        existing_paths: HashSet<PathBuf>,
    ) -> Option<Self> {
        if items.len() < 2 {
            return None;
        }

        let mut state = Self {
            items,
            rules: Vec::new(),
            next_rule_id: 1,
            selected_rule: None,
            add_rule_menu_open: false,
            sort: BatchRenameSortRule::default(),
            manual_target_name_overrides: HashMap::new(),
            editing_target_name_source: None,
            editing_target_name_input: String::new(),
            dragging_preview_source: None,
            existing_paths,
            preview: BatchRenamePreview { rows: Vec::new() },
        };
        state.rebuild_preview();
        Some(state)
    }

    pub(crate) fn rebuild_preview(&mut self) {
        self.preview = build_batch_rename_preview(self);
    }

    pub(crate) fn apply_update(&mut self, message: BatchRenameMessage) {
        match message {
            BatchRenameMessage::AddRuleMenuToggled => {
                self.add_rule_menu_open = !self.add_rule_menu_open;
            }
            BatchRenameMessage::AddRuleSelected(kind) => self.add_rule(kind),
            BatchRenameMessage::RuleSelected(id) => {
                self.add_rule_menu_open = false;
                self.selected_rule = Some(id);
            }
            BatchRenameMessage::RuleEnabledToggled(id) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.enabled = !rule.enabled;
                }
            }
            BatchRenameMessage::RuleMoved(id, delta) => self.move_rule(id, delta),
            BatchRenameMessage::RuleRemoved(id) => self.remove_rule(id),
            BatchRenameMessage::SortModeSelected(mode) => self.sort.mode = mode,
            BatchRenameMessage::TemplateChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Template(params)) = self.rule_params_mut(id) {
                    params.template = value;
                }
            }
            BatchRenameMessage::TemplateTokenMenuToggled(id) => {
                if let Some(BatchRenameRuleParams::Template(params)) = self.rule_params_mut(id) {
                    params.token_menu_open = !params.token_menu_open;
                }
            }
            BatchRenameMessage::TemplateTokenSelected(id, token) => {
                if let Some(BatchRenameRuleParams::Template(params)) = self.rule_params_mut(id) {
                    params.template.push_str(&token.label());
                    params.token_menu_open = false;
                }
            }
            BatchRenameMessage::ExtensionModeSelected(id, mode) => {
                if let Some(BatchRenameRuleParams::Extension(params)) = self.rule_params_mut(id) {
                    params.mode = mode;
                }
            }
            BatchRenameMessage::ExtensionReplacementChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Extension(params)) = self.rule_params_mut(id) {
                    params.replacement = value;
                }
            }
            BatchRenameMessage::SequencePrefixChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.prefix = value;
                }
            }
            BatchRenameMessage::SequenceStartChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.start_input = value;
                }
            }
            BatchRenameMessage::SequenceStepChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.step_input = value;
                }
            }
            BatchRenameMessage::SequencePaddingChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.padding_input = value;
                }
            }
            BatchRenameMessage::SequenceIncludeOriginalToggled(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.include_original_stem = value;
                }
            }
            BatchRenameMessage::SequencePreserveExtensionToggled(id, value) => {
                if let Some(BatchRenameRuleParams::Sequence(params)) = self.rule_params_mut(id) {
                    params.preserve_extension = value;
                }
            }
            BatchRenameMessage::ReplaceFindChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.find = value;
                }
            }
            BatchRenameMessage::ReplaceWithChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.replacement = value;
                }
            }
            BatchRenameMessage::ReplaceScopeSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.scope = value;
                }
            }
            BatchRenameMessage::ReplaceRangeStartChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.range_start_input = value;
                }
            }
            BatchRenameMessage::ReplaceRangeLengthChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.range_length_input = value;
                }
            }
            BatchRenameMessage::ReplaceIgnoreCaseToggled(id, value) => {
                if let Some(BatchRenameRuleParams::Replace(params)) = self.rule_params_mut(id) {
                    params.ignore_case = value;
                }
            }
            BatchRenameMessage::InsertTextChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Insert(params)) = self.rule_params_mut(id) {
                    params.text = value;
                }
            }
            BatchRenameMessage::InsertPositionChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Insert(params)) = self.rule_params_mut(id) {
                    params.position_input = value;
                }
            }
            BatchRenameMessage::InsertModeSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Insert(params)) = self.rule_params_mut(id) {
                    params.mode = value;
                }
            }
            BatchRenameMessage::InsertAnchorChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Insert(params)) = self.rule_params_mut(id) {
                    params.anchor = value;
                }
            }
            BatchRenameMessage::InsertIgnoreExtensionToggled(id, value) => {
                if let Some(BatchRenameRuleParams::Insert(params)) = self.rule_params_mut(id) {
                    params.ignore_extension = value;
                }
            }
            BatchRenameMessage::SliceStartChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Slice(params)) = self.rule_params_mut(id) {
                    params.start_input = value;
                }
            }
            BatchRenameMessage::SliceLengthChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Slice(params)) = self.rule_params_mut(id) {
                    params.length_input = value;
                }
            }
            BatchRenameMessage::SliceModeSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Slice(params)) = self.rule_params_mut(id) {
                    params.mode = value;
                }
            }
            BatchRenameMessage::SliceAnchorChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Slice(params)) = self.rule_params_mut(id) {
                    params.anchor = value;
                }
            }
            BatchRenameMessage::CaseSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Case(params)) = self.rule_params_mut(id) {
                    *params = value;
                }
            }
            BatchRenameMessage::RandomModeSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Random(params)) = self.rule_params_mut(id) {
                    params.mode = value;
                }
            }
            BatchRenameMessage::RandomLengthChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Random(params)) = self.rule_params_mut(id) {
                    params.length_input = value;
                }
            }
            BatchRenameMessage::RandomAlphabetChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Random(params)) = self.rule_params_mut(id) {
                    params.alphabet = value;
                }
            }
            BatchRenameMessage::RemoveTextChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Remove(params)) = self.rule_params_mut(id) {
                    params.text = value;
                }
            }
            BatchRenameMessage::RemoveStartChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Remove(params)) = self.rule_params_mut(id) {
                    params.start_input = value;
                }
            }
            BatchRenameMessage::RemoveLengthChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Remove(params)) = self.rule_params_mut(id) {
                    params.length_input = value;
                }
            }
            BatchRenameMessage::RemoveModeSelected(id, value) => {
                if let Some(BatchRenameRuleParams::Remove(params)) = self.rule_params_mut(id) {
                    params.mode = value;
                }
            }
            BatchRenameMessage::RemoveClassToggled(id, class, enabled) => {
                if let Some(BatchRenameRuleParams::Remove(params)) = self.rule_params_mut(id) {
                    if enabled {
                        if !params.classes.contains(&class) {
                            params.classes.push(class);
                        }
                    } else {
                        params.classes.retain(|candidate| *candidate != class);
                    }
                }
            }
            BatchRenameMessage::ListNamesChanged(id, value) => {
                if let Some(BatchRenameRuleParams::List(params)) = self.rule_params_mut(id) {
                    params.names = value;
                }
            }
            BatchRenameMessage::RegexPatternChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Regex(params)) = self.rule_params_mut(id) {
                    params.pattern = value;
                }
            }
            BatchRenameMessage::RegexReplacementChanged(id, value) => {
                if let Some(BatchRenameRuleParams::Regex(params)) = self.rule_params_mut(id) {
                    params.replacement = value;
                }
            }
            BatchRenameMessage::PreviewNameEditStarted(source) => {
                self.start_preview_name_edit(source)
            }
            BatchRenameMessage::PreviewNameChanged(value) => self.update_preview_name_edit(value),
            BatchRenameMessage::PreviewNameEditCommitted => self.finish_preview_name_edit(),
            BatchRenameMessage::PreviewDragStarted(source) => self.start_preview_drag(source),
            BatchRenameMessage::PreviewDragEntered(target) => {
                self.reorder_dragging_preview_source(target)
            }
            BatchRenameMessage::PreviewDragFinished => self.finish_preview_drag(),
            BatchRenameMessage::OpenSelected
            | BatchRenameMessage::Apply
            | BatchRenameMessage::Cancel => {}
        }
    }

    fn add_rule(&mut self, kind: BatchRenameRuleKind) {
        let id = self.next_rule_id;
        self.next_rule_id += 1;
        self.rules.push(BatchRenameRule {
            id,
            enabled: true,
            params: BatchRenameRuleParams::default_for(kind),
        });
        self.selected_rule = Some(id);
        self.add_rule_menu_open = false;
    }

    fn remove_rule(&mut self, id: u64) {
        self.rules.retain(|rule| rule.id != id);
        if self.selected_rule == Some(id) {
            self.selected_rule = self.rules.last().map(|rule| rule.id);
        }
    }

    fn move_rule(&mut self, id: u64, delta: i32) {
        let Some(index) = self.rules.iter().position(|rule| rule.id == id) else {
            return;
        };
        let Some(target) = index.checked_add_signed(delta as isize) else {
            return;
        };
        if target >= self.rules.len() {
            return;
        }
        self.rules.swap(index, target);
    }

    pub(crate) fn selected_rule(&self) -> Option<&BatchRenameRule> {
        self.rules
            .iter()
            .find(|rule| Some(rule.id) == self.selected_rule)
    }

    fn rule_mut(&mut self, id: u64) -> Option<&mut BatchRenameRule> {
        self.rules.iter_mut().find(|rule| rule.id == id)
    }

    fn rule_params_mut(&mut self, id: u64) -> Option<&mut BatchRenameRuleParams> {
        self.rule_mut(id).map(|rule| &mut rule.params)
    }

    pub(crate) fn dragging_preview_source(&self) -> Option<&Path> {
        self.dragging_preview_source.as_deref()
    }

    pub(crate) fn editing_target_name_source(&self) -> Option<&Path> {
        self.editing_target_name_source.as_deref()
    }

    pub(crate) fn editing_target_name_input(&self) -> &str {
        &self.editing_target_name_input
    }

    pub(crate) fn preview_target_name_for_source(&self, source: &Path) -> Option<&str> {
        self.preview
            .rows
            .iter()
            .find(|row| row.source == source)
            .map(|row| row.target_name.as_str())
    }

    pub(crate) fn finish_preview_drag(&mut self) {
        self.dragging_preview_source = None;
    }

    pub(crate) fn can_apply(&self) -> bool {
        let has_problem = self.preview.rows.iter().any(|row| {
            matches!(
                row.status,
                BatchRenamePreviewStatus::EmptyName
                    | BatchRenamePreviewStatus::DuplicateTarget
                    | BatchRenamePreviewStatus::ExistingTarget
                    | BatchRenamePreviewStatus::RuleError
            )
        });
        let has_change = self
            .preview
            .rows
            .iter()
            .any(|row| row.status == BatchRenamePreviewStatus::Ready);
        !has_problem && has_change
    }

    pub(crate) fn plan(&self) -> Option<Vec<BatchRenameItem>> {
        self.can_apply().then(|| {
            self.preview
                .rows
                .iter()
                .map(|row| BatchRenameItem {
                    from: row.source.clone(),
                    to: row.target.clone(),
                })
                .collect()
        })
    }

    fn start_preview_drag(&mut self, source: PathBuf) {
        if self.items.iter().any(|item| item.path == source) {
            self.dragging_preview_source = Some(source);
        }
    }

    fn start_preview_name_edit(&mut self, source: PathBuf) {
        let Some(target_name) = self
            .preview_target_name_for_source(&source)
            .map(ToOwned::to_owned)
        else {
            return;
        };
        self.editing_target_name_source = Some(source);
        self.editing_target_name_input = target_name;
    }

    fn update_preview_name_edit(&mut self, value: String) {
        let Some(source) = self.editing_target_name_source.clone() else {
            return;
        };
        self.editing_target_name_input = value.clone();
        self.manual_target_name_overrides.insert(source, value);
    }

    fn finish_preview_name_edit(&mut self) {
        self.editing_target_name_source = None;
        self.editing_target_name_input.clear();
    }

    pub(super) fn manual_target_name_override(&self, source: &Path) -> Option<&str> {
        self.manual_target_name_overrides
            .get(source)
            .map(String::as_str)
    }

    fn reorder_dragging_preview_source(&mut self, target: PathBuf) {
        let Some(source) = self.dragging_preview_source.as_ref() else {
            return;
        };
        if *source == target {
            return;
        }

        let Some(source_index) = self.items.iter().position(|item| item.path == *source) else {
            self.dragging_preview_source = None;
            return;
        };
        let Some(target_index) = self.items.iter().position(|item| item.path == target) else {
            return;
        };

        let item = self.items.remove(source_index);
        let insertion_index = target_index.min(self.items.len());
        self.items.insert(insertion_index, item);
        self.sort.mode = BatchRenameSortMode::SelectionOrder;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BatchRenameState {
    pub(crate) items: Vec<BatchRenameSource>,
    pub(crate) rules: Vec<BatchRenameRule>,
    next_rule_id: u64,
    pub(crate) selected_rule: Option<u64>,
    pub(crate) add_rule_menu_open: bool,
    pub(crate) sort: BatchRenameSortRule,
    manual_target_name_overrides: HashMap<PathBuf, String>,
    editing_target_name_source: Option<PathBuf>,
    editing_target_name_input: String,
    dragging_preview_source: Option<PathBuf>,
    existing_paths: HashSet<PathBuf>,
    pub(crate) preview: BatchRenamePreview,
}

impl BatchRenameSource {
    pub(crate) fn try_from_entry(
        entry: &DirectoryEntry,
    ) -> Result<Self, BatchRenameSourceNameError> {
        let source_name_text = entry
            .name()
            .to_str()
            .ok_or_else(|| BatchRenameSourceNameError {
                path: entry.path.clone(),
            })?;
        Ok(Self {
            path: entry.path.clone(),
            source_name_text: source_name_text.to_owned(),
            modified: entry.metadata.modified,
        })
    }
}

impl fmt::Display for BatchRenameSourceNameError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "Batch rename does not support non-UTF-8 file names: {:?}",
            self.path
        )
    }
}

impl BatchRenamePreviewStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Unchanged => "Unchanged",
            Self::EmptyName => "Empty name",
            Self::DuplicateTarget => "Duplicate target",
            Self::ExistingTarget => "Already exists",
            Self::RuleError => "Rule error",
        }
    }
}

pub(crate) fn same_parent(paths: &[PathBuf]) -> bool {
    let mut parents = paths.iter().filter_map(|path| path.parent());
    let Some(parent) = parents.next() else {
        return false;
    };
    parents.all(|candidate| candidate == parent)
}
