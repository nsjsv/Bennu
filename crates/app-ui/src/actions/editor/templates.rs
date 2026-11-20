//! 新建动作时的起点。用和界面一样的改动搭出来，不另写一套构造。

use file_core::mime::MimeDatabase;
use file_core::ArchiveFormat;

use super::super::convert_format::ConvertFormat;
use super::super::definition::TrashScope;
use super::draft::{ActionDraft, DraftEdit, StepKind, StepOption, TypeCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionTemplate {
    Blank,
    ExtractAudio,
    ConvertToJpg,
    CompressAndCopyPath,
    ExtractAndTrashArchive,
}

impl ActionTemplate {
    pub(crate) const ALL: [Self; 5] = [
        Self::Blank,
        Self::ExtractAudio,
        Self::ConvertToJpg,
        Self::CompressAndCopyPath,
        Self::ExtractAndTrashArchive,
    ];

    /// 动作页上那一行的标题。
    pub(crate) fn label(self) -> String {
        match self {
            Self::Blank => tr!("Blank Action"),
            _ => self.name(),
        }
    }

    fn name(self) -> String {
        match self {
            Self::Blank => tr!("New Action"),
            Self::ExtractAudio => tr!("Extract Audio"),
            Self::ConvertToJpg => tr!("Convert to JPG"),
            Self::CompressAndCopyPath => tr!("Compress and Copy the Path"),
            Self::ExtractAndTrashArchive => tr!("Extract and Trash the Archive"),
        }
    }

    pub(crate) fn draft(self, types: &MimeDatabase) -> ActionDraft {
        let format = |name| {
            StepOption::Format(ConvertFormat::named(name).expect("the template format is known"))
        };
        let edits = match self {
            Self::Blank => Vec::new(),
            Self::ExtractAudio => vec![
                DraftEdit::Category(TypeCategory::Video, true),
                DraftEdit::StepAdded,
                DraftEdit::StepOption(0, format("m4a")),
                DraftEdit::StepAdded,
                DraftEdit::StepKind(1, StepKind::MoveTo),
                DraftEdit::StepText(1, format!("{{dir}}/{}", tr!("Audio"))),
            ],
            Self::ConvertToJpg => vec![
                DraftEdit::Category(TypeCategory::Images, true),
                DraftEdit::StepAdded,
                DraftEdit::StepOption(0, format("jpg")),
            ],
            Self::CompressAndCopyPath => vec![
                DraftEdit::StepAdded,
                DraftEdit::StepOption(0, StepOption::Archive(ArchiveFormat::Zip)),
                DraftEdit::StepAdded,
            ],
            Self::ExtractAndTrashArchive => vec![
                DraftEdit::Category(TypeCategory::Archives, true),
                DraftEdit::StepAdded,
                DraftEdit::StepKind(0, StepKind::Extract),
                DraftEdit::StepAdded,
                DraftEdit::StepOption(1, StepOption::Scope(TrashScope::Selected)),
            ],
        };
        let mut draft = ActionDraft::empty(types);
        draft.apply(DraftEdit::Name(self.name()), types);
        for edit in edits {
            draft.apply(edit, types);
        }
        draft
    }
}
