//! 编辑页上的表单：用户看得见、能改的值。输入框的字在改的时候检查一次，结果跟着字存着；
//! 界面和保存都信这个结果，不再重新解析。下拉框选的本身就是合法值，不用查。

use std::collections::BTreeSet;

use file_core::mime::{MimeDatabase, ARCHIVE_TYPES, DOCUMENT_TYPES, TEXT_PLAIN};
use file_core::{ArchiveExtractionPlacement, ArchiveFormat};

use super::super::action_file::parse_type_pattern;
use super::super::convert_format::ConvertFormat;
use super::super::definition::{ActionDefinition, ActionStep, TrashScope};
use super::super::notice::{ActionNotice, NoticeTemplate};
use super::super::template::{CommandTemplate, PathTemplate};
use super::super::type_filter::{TypeFilter, TypePattern};

/// 正在打的字和它的检查结果（错时是给用户看的原因）。字一样就算一样。
#[derive(Debug, Clone)]
pub(crate) struct TextField<T> {
    text: String,
    parsed: Result<T, String>,
}

impl<T> TextField<T> {
    fn new(text: String, parse: impl FnOnce(&str) -> Result<T, String>) -> Self {
        let parsed = parse(&text);
        Self { text, parsed }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn value(&self) -> Option<&T> {
        self.parsed.as_ref().ok()
    }

    pub(crate) fn mistake(&self) -> Option<&str> {
        self.parsed.as_ref().err().map(String::as_str)
    }
}

impl<T> PartialEq for TextField<T> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl<T> Eq for TextField<T> {}

fn folder_field(text: String) -> TextField<PathTemplate> {
    TextField::new(text, |text| {
        PathTemplate::parse(text).map_err(|error| error.0)
    })
}

fn command_field(text: String) -> TextField<CommandTemplate> {
    TextField::new(text, |text| {
        CommandTemplate::parse(text).map_err(|error| error.0)
    })
}

/// 常用的几类，点选就行；文件里存的始终是它们展开后的 MIME。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TypeCategory {
    Folders,
    Images,
    Video,
    Audio,
    Documents,
    Archives,
    AllFiles,
}

impl TypeCategory {
    pub(crate) const ALL: [Self; 7] = [
        Self::Folders,
        Self::Images,
        Self::Video,
        Self::Audio,
        Self::Documents,
        Self::Archives,
        Self::AllFiles,
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Folders => tr!("Folders"),
            Self::Images => tr!("Images"),
            Self::Video => tr!("Video"),
            Self::Audio => tr!("Audio"),
            Self::Documents => tr!("Documents"),
            Self::Archives => tr!("Archives"),
            Self::AllFiles => tr!("All Files"),
        }
    }

    /// 写进文件的类型，按类型库取规范名、去掉重复（压缩包表里有别名）。
    fn patterns(self, types: &MimeDatabase) -> Vec<TypePattern> {
        let names: Vec<&str> = match self {
            Self::Folders => vec!["inode/directory"],
            Self::Images => vec!["image/*"],
            Self::Video => vec!["video/*"],
            Self::Audio => vec!["audio/*"],
            Self::Documents => std::iter::once(TEXT_PLAIN)
                .chain(DOCUMENT_TYPES.iter().copied())
                .collect(),
            Self::Archives => ARCHIVE_TYPES.to_vec(),
            Self::AllFiles => vec!["application/octet-stream"],
        };
        let mut patterns = Vec::with_capacity(names.len());
        for pattern in names
            .into_iter()
            .filter_map(|name| TypePattern::parse(name, types))
        {
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
        patterns
    }
}

/// 对哪些项：选中的类别，加上“其他类型”框里空格分开的 MIME。都空 = 所有文件和文件夹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeChoice {
    categories: BTreeSet<TypeCategory>,
    other: TextField<Vec<TypePattern>>,
}

impl TypeChoice {
    fn other_field(text: String, types: &MimeDatabase) -> TextField<Vec<TypePattern>> {
        TextField::new(text, |text| {
            text.split_whitespace()
                .map(|pattern| parse_type_pattern(pattern, types))
                .collect()
        })
    }

    /// 文件里的列表：选中了某个类别展开后的全部类型，就显示成选中它；剩下的进“其他类型”。
    fn from_filter(filter: &TypeFilter, types: &MimeDatabase) -> Self {
        let mut rest = match filter {
            TypeFilter::Anything => Vec::new(),
            TypeFilter::Patterns(patterns) => patterns.clone(),
        };
        let mut categories = BTreeSet::new();
        for category in TypeCategory::ALL {
            let patterns = category.patterns(types);
            if !patterns.is_empty() && patterns.iter().all(|pattern| rest.contains(pattern)) {
                rest.retain(|pattern| !patterns.contains(pattern));
                categories.insert(category);
            }
        }
        let other = rest
            .iter()
            .map(TypePattern::text)
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            categories,
            other: Self::other_field(other, types),
        }
    }

    pub(crate) fn contains(&self, category: TypeCategory) -> bool {
        self.categories.contains(&category)
    }

    pub(crate) fn other(&self) -> &TextField<Vec<TypePattern>> {
        &self.other
    }

    /// 写进 `types` 的列表：类别按固定顺序展开，后面接其他类型，去掉重复。空 = 不写 `types`。
    /// “其他类型”有错时为 None。
    pub(crate) fn texts(&self, types: &MimeDatabase) -> Option<Vec<String>> {
        let mut patterns = Vec::new();
        let chosen = self
            .categories
            .iter()
            .flat_map(|category| category.patterns(types));
        for pattern in chosen.chain(self.other.value()?.iter().cloned()) {
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
        Some(patterns.iter().map(TypePattern::text).collect())
    }
}

/// 步骤的种类：卡片上的第一个下拉框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepKind {
    Convert,
    Compress,
    Extract,
    CopyTo,
    MoveTo,
    Trash,
    Run,
    CopyPaths,
}

impl StepKind {
    pub(crate) const ALL: [Self; 8] = [
        Self::Convert,
        Self::Compress,
        Self::Extract,
        Self::CopyTo,
        Self::MoveTo,
        Self::Trash,
        Self::Run,
        Self::CopyPaths,
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Convert => tr!("Convert"),
            Self::Compress => tr!("Compress"),
            Self::Extract => tr!("Extract"),
            Self::CopyTo => tr!("Copy To"),
            Self::MoveTo => tr!("Move To"),
            Self::Trash => tr!("Move to Trash"),
            Self::Run => tr!("Run Command"),
            Self::CopyPaths => tr!("Copy Path"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepDraft {
    Convert(ConvertFormat),
    Compress(ArchiveFormat),
    Extract(ArchiveExtractionPlacement),
    CopyTo(TextField<PathTemplate>),
    MoveTo(TextField<PathTemplate>),
    Trash(TrashScope),
    Run(TextField<CommandTemplate>),
    CopyPaths,
}

/// 卡片上第二个下拉框选的值。每种值只属于一种步骤，所以选了它，这一步就是那一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepOption {
    Format(ConvertFormat),
    Archive(ArchiveFormat),
    Placement(ArchiveExtractionPlacement),
    Scope(TrashScope),
}

impl StepDraft {
    pub(crate) fn kind(&self) -> StepKind {
        match self {
            Self::Convert(_) => StepKind::Convert,
            Self::Compress(_) => StepKind::Compress,
            Self::Extract(_) => StepKind::Extract,
            Self::CopyTo(_) => StepKind::CopyTo,
            Self::MoveTo(_) => StepKind::MoveTo,
            Self::Trash(_) => StepKind::Trash,
            Self::Run(_) => StepKind::Run,
            Self::CopyPaths => StepKind::CopyPaths,
        }
    }

    /// 换成另一种步骤。复制到和移动到之间互换时文件夹的字留着，其余换成这种步骤的默认值。
    fn with_kind(&self, kind: StepKind) -> Self {
        let folder = match self {
            Self::CopyTo(folder) | Self::MoveTo(folder) => folder.text.clone(),
            _ => String::new(),
        };
        match kind {
            StepKind::Convert => {
                Self::Convert(ConvertFormat::named("jpg").expect("jpg is a known format"))
            }
            StepKind::Compress => Self::Compress(ArchiveFormat::Zip),
            StepKind::Extract => Self::Extract(ArchiveExtractionPlacement::Smart),
            StepKind::CopyTo => Self::CopyTo(folder_field(folder)),
            StepKind::MoveTo => Self::MoveTo(folder_field(folder)),
            StepKind::Trash => Self::Trash(TrashScope::Current),
            StepKind::Run => Self::Run(command_field(String::new())),
            StepKind::CopyPaths => Self::CopyPaths,
        }
    }

    fn from_step(step: &ActionStep) -> Self {
        match step {
            ActionStep::Convert(format) => Self::Convert(*format),
            ActionStep::Compress(format) => Self::Compress(*format),
            ActionStep::Extract(placement) => Self::Extract(*placement),
            ActionStep::CopyTo(folder) => Self::CopyTo(folder_field(folder.text().to_owned())),
            ActionStep::MoveTo(folder) => Self::MoveTo(folder_field(folder.text().to_owned())),
            ActionStep::Trash(scope) => Self::Trash(*scope),
            ActionStep::Run(command) => Self::Run(command_field(command.text().to_owned())),
            ActionStep::CopyPaths => Self::CopyPaths,
        }
    }

    /// 输入框里的字（只有复制到、移动到、执行命令有）。
    pub(crate) fn text_field_mistake(&self) -> Option<&str> {
        match self {
            Self::CopyTo(folder) | Self::MoveTo(folder) => folder.mistake(),
            Self::Run(command) => command.mistake(),
            _ => None,
        }
    }

    fn set_text(&mut self, text: String) {
        match self {
            Self::CopyTo(folder) | Self::MoveTo(folder) => *folder = folder_field(text),
            Self::Run(command) => *command = command_field(text),
            _ => {}
        }
    }
}

/// 通知三选一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeChoice {
    Standard,
    Custom,
    Silent,
}

impl NoticeChoice {
    pub(crate) const ALL: [Self; 3] = [Self::Standard, Self::Custom, Self::Silent];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Standard => tr!("Default"),
            Self::Custom => tr!("Write My Own"),
            Self::Silent => tr!("No Notification"),
        }
    }
}

/// 不是“自己写”时，标题和正文的字也留着：切回来还在。比较时只比会写进文件的部分。
#[derive(Debug, Clone)]
pub(crate) struct NoticeDraft {
    choice: NoticeChoice,
    title: TextField<NoticeTemplate>,
    /// 空 = 不写 `body`。
    body: TextField<Option<NoticeTemplate>>,
}

impl PartialEq for NoticeDraft {
    fn eq(&self, other: &Self) -> bool {
        self.choice == other.choice
            && (self.choice != NoticeChoice::Custom
                || (self.title == other.title && self.body == other.body))
    }
}

impl Eq for NoticeDraft {}

fn title_field(text: String) -> TextField<NoticeTemplate> {
    TextField::new(text, |text| {
        if text.trim().is_empty() {
            return Err(tr!("The notification needs a title"));
        }
        NoticeTemplate::parse(text).map_err(|error| error.0)
    })
}

fn body_field(text: String) -> TextField<Option<NoticeTemplate>> {
    TextField::new(text, |text| {
        if text.trim().is_empty() {
            return Ok(None);
        }
        NoticeTemplate::parse(text)
            .map(Some)
            .map_err(|error| error.0)
    })
}

impl NoticeDraft {
    fn standard() -> Self {
        Self {
            choice: NoticeChoice::Standard,
            title: title_field(String::new()),
            body: body_field(String::new()),
        }
    }

    fn from_notice(notice: &ActionNotice) -> Self {
        match notice {
            ActionNotice::Standard => Self::standard(),
            ActionNotice::Silent => Self {
                choice: NoticeChoice::Silent,
                ..Self::standard()
            },
            ActionNotice::Custom { title, body } => Self {
                choice: NoticeChoice::Custom,
                title: title_field(title.text().to_owned()),
                body: body_field(
                    body.as_ref()
                        .map(|body| body.text().to_owned())
                        .unwrap_or_default(),
                ),
            },
        }
    }

    pub(crate) fn choice(&self) -> NoticeChoice {
        self.choice
    }

    pub(crate) fn title(&self) -> &TextField<NoticeTemplate> {
        &self.title
    }

    pub(crate) fn body(&self) -> &TextField<Option<NoticeTemplate>> {
        &self.body
    }

    fn is_complete(&self) -> bool {
        self.choice != NoticeChoice::Custom
            || (self.title.mistake().is_none() && self.body.mistake().is_none())
    }
}

/// 表单上的一次改动。界面消息只带原始输入，由 [`ActionDraft::apply`] 检查并应用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftEdit {
    Name(String),
    Category(TypeCategory, bool),
    OtherTypes(String),
    StepKind(usize, StepKind),
    StepOption(usize, StepOption),
    StepText(usize, String),
    StepMovedUp(usize),
    StepMovedDown(usize),
    StepRemoved(usize),
    StepAdded,
    Notice(NoticeChoice),
    NoticeTitle(String),
    NoticeBody(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionDraft {
    pub(crate) name: String,
    pub(crate) types: TypeChoice,
    pub(crate) steps: Vec<StepDraft>,
    pub(crate) notice: NoticeDraft,
}

impl ActionDraft {
    /// 什么都没写的动作：新文档的起点。
    pub(crate) fn empty(types: &MimeDatabase) -> Self {
        Self {
            name: String::new(),
            types: TypeChoice::from_filter(&TypeFilter::Anything, types),
            steps: Vec::new(),
            notice: NoticeDraft::standard(),
        }
    }

    /// 读文件时校验过的定义，模板和类型都带着原文。
    pub(crate) fn from_definition(definition: &ActionDefinition, types: &MimeDatabase) -> Self {
        Self {
            name: definition.name.clone(),
            types: TypeChoice::from_filter(&definition.types, types),
            steps: definition.steps.iter().map(StepDraft::from_step).collect(),
            notice: NoticeDraft::from_notice(&definition.notice),
        }
    }

    /// 消息来自界面上同一帧的控件，下标对不上（连点两下删除）时什么都不做。
    pub(crate) fn apply(&mut self, edit: DraftEdit, types: &MimeDatabase) {
        match edit {
            DraftEdit::Name(name) => self.name = name,
            DraftEdit::Category(category, true) => {
                self.types.categories.insert(category);
            }
            DraftEdit::Category(category, false) => {
                self.types.categories.remove(&category);
            }
            DraftEdit::OtherTypes(text) => self.types.other = TypeChoice::other_field(text, types),
            DraftEdit::StepKind(index, kind) => {
                if let Some(step) = self.steps.get_mut(index).filter(|step| step.kind() != kind) {
                    *step = step.with_kind(kind);
                }
            }
            DraftEdit::StepOption(index, option) => {
                if let Some(step) = self.steps.get_mut(index) {
                    *step = match option {
                        StepOption::Format(format) => StepDraft::Convert(format),
                        StepOption::Archive(format) => StepDraft::Compress(format),
                        StepOption::Placement(placement) => StepDraft::Extract(placement),
                        StepOption::Scope(scope) => StepDraft::Trash(scope),
                    };
                }
            }
            DraftEdit::StepText(index, text) => {
                if let Some(step) = self.steps.get_mut(index) {
                    step.set_text(text);
                }
            }
            DraftEdit::StepMovedUp(index) => {
                if index > 0 && index < self.steps.len() {
                    self.steps.swap(index - 1, index);
                }
            }
            DraftEdit::StepMovedDown(index) => {
                if index + 1 < self.steps.len() {
                    self.steps.swap(index, index + 1);
                }
            }
            DraftEdit::StepRemoved(index) => {
                if index < self.steps.len() {
                    self.steps.remove(index);
                }
            }
            // 新的一步默认“复制路径”：不用填任何东西，换种类也只要一下。
            DraftEdit::StepAdded => self.steps.push(StepDraft::CopyPaths),
            DraftEdit::Notice(choice) => self.notice.choice = choice,
            DraftEdit::NoticeTitle(text) => self.notice.title = title_field(text),
            DraftEdit::NoticeBody(text) => self.notice.body = body_field(text),
        }
    }

    pub(crate) fn name_mistake(&self) -> Option<String> {
        self.name
            .trim()
            .is_empty()
            .then(|| tr!("The name cannot be empty"))
    }

    pub(crate) fn steps_mistake(&self) -> Option<String> {
        self.steps.is_empty().then(|| tr!("Add at least one step"))
    }

    /// 这一步的问题：输入框里的错，或者“移到回收站”不在最后。
    pub(crate) fn step_mistake(&self, index: usize) -> Option<String> {
        let step = self.steps.get(index)?;
        if let Some(mistake) = step.text_field_mistake() {
            return Some(mistake.to_owned());
        }
        (step.kind() == StepKind::Trash && index + 1 != self.steps.len())
            .then(|| tr!("Moving to the trash has to be the last step"))
    }

    /// 没有任何错，可以保存。
    pub(crate) fn is_complete(&self) -> bool {
        self.name_mistake().is_none()
            && self.steps_mistake().is_none()
            && (0..self.steps.len()).all(|index| self.step_mistake(index).is_none())
            && self.types.other.mistake().is_none()
            && self.notice.is_complete()
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
