//! 动作编辑器：设置里不写文件就能新建、修改动作。这里是编辑页的状态和纯模型——表单草稿、
//! 往 TOML 文档里写改动、模板、写文件；打开和保存在后台线程跑，由 `app/actions/editor.rs` 接上。
//!
//! 编辑器写的就是手写的那种动作文件：只改动过的值，注释和格式留着；写出来的文本先用读动作文件的
//! 同一套检查过一遍，过了才写盘。

mod document;
mod draft;
mod saving;
mod templates;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use file_core::mime::MimeDatabase;
use toml_edit::DocumentMut;

pub(crate) use draft::{
    ActionDraft, DraftEdit, NoticeChoice, StepDraft, StepKind, StepOption, TypeCategory,
};
pub(crate) use saving::{save, SaveOutcome, SaveTarget};
pub(crate) use templates::ActionTemplate;

use super::action_file::parse_action_file;
use super::definition::{ActionDefinition, ActionId};
use super::library::read_trusted_text;
use super::template::RunValues;
use crate::model::SettingsCategory;

/// 正在编辑哪个文件。
#[derive(Debug, Clone)]
pub(crate) enum EditTarget {
    /// 还没有文件：保存时在动作文件夹里按名字建。
    New,
    File {
        path: PathBuf,
        id: ActionId,
        /// 打开或上次保存时文件里的内容；保存前文件要还是这样。`None` = 文件在别处被删了，
        /// 用户选了“仍然保存”。
        on_disk: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum EditorStatus {
    Editing,
    /// 正在写盘。带着这次要写的草稿和文档，写成了它们就是新的“已写下的内容”。
    Saving {
        draft: Box<ActionDraft>,
        document: Box<DocumentMut>,
    },
    Saved,
    /// 文件在别处改过了（`found` 是现在的内容，被删了是 `None`），这次没写。
    Changed {
        found: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ActionEditor {
    /// 每开一个编辑页一个新号。后台写完的结果带着它回来，只交给发出请求的那个编辑页。
    pub(crate) session: u64,
    pub(crate) target: EditTarget,
    pub(crate) draft: ActionDraft,
    /// `document` 现在写着的内容：保存时只把和它不同的地方写进文档。
    written: ActionDraft,
    document: DocumentMut,
    pub(crate) status: EditorStatus,
    /// 返回或切换分类时有未保存的修改：要去哪，等用户选“放弃”还是“继续编辑”。
    /// 返回动作列表就是去“动作”分类。
    pub(crate) leaving: Option<SettingsCategory>,
    /// 预览里 `{date}` 用的值，打开时取一次。
    pub(crate) run_values: RunValues,
}

/// 后台写盘要的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveRequest {
    pub(crate) target: SaveTarget,
    pub(crate) text: String,
}

impl ActionEditor {
    /// 新动作：模板先写进一份空文档，所以没动过的模板离开时不用问。
    pub(crate) fn from_template(
        template: ActionTemplate,
        types: &MimeDatabase,
        run_values: RunValues,
    ) -> Self {
        let draft = template.draft(types);
        let mut document = DocumentMut::new();
        document::write_changes(&mut document, &ActionDraft::empty(types), &draft, types);
        Self::with(EditTarget::New, draft, document, run_values)
    }

    /// 读一个动作文件来编辑：和动作库读文件同样的信任检查和校验。会读盘，只在后台线程调用。
    pub(crate) fn open(
        path: PathBuf,
        id: ActionId,
        types: &MimeDatabase,
        run_values: RunValues,
    ) -> Result<Self, String> {
        let (text, _) = read_trusted_text(&path)?;
        let definition = parse_action_file(id, &path, &text, types).map_err(located)?;
        Self::from_file(path, text, &definition, types, run_values)
    }

    /// 打开一个动作文件。`definition` 是同一份 `text` 读出来的。
    pub(crate) fn from_file(
        path: PathBuf,
        text: String,
        definition: &ActionDefinition,
        types: &MimeDatabase,
        run_values: RunValues,
    ) -> Result<Self, String> {
        let document = text
            .parse::<DocumentMut>()
            .map_err(|error| error.message().to_owned())?;
        let target = EditTarget::File {
            path,
            id: definition.id.clone(),
            on_disk: Some(text),
        };
        let draft = ActionDraft::from_definition(definition, types);
        Ok(Self::with(target, draft, document, run_values))
    }

    fn with(
        target: EditTarget,
        draft: ActionDraft,
        document: DocumentMut,
        run_values: RunValues,
    ) -> Self {
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
        Self {
            session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            target,
            written: draft.clone(),
            draft,
            document,
            status: EditorStatus::Editing,
            leaving: None,
            run_values,
        }
    }

    /// 用现在的内容开一个新动作（注释也带上），名字后面加“副本”。
    pub(crate) fn duplicate(&self, types: &MimeDatabase) -> Self {
        let mut copy = Self::with(
            EditTarget::New,
            self.written.clone(),
            self.document.clone(),
            self.run_values.clone(),
        );
        copy.draft.apply(
            DraftEdit::Name(tr!("%1 Copy", self.draft.name.trim())),
            types,
        );
        copy
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.draft != self.written
    }

    pub(crate) fn is_saving(&self) -> bool {
        matches!(self.status, EditorStatus::Saving { .. })
    }

    /// 没有错、不在保存中，而且有东西要写（新动作总有：文件还没有）。
    pub(crate) fn can_save(&self) -> bool {
        !self.is_saving()
            && self.draft.is_complete()
            && (matches!(self.target, EditTarget::New) || self.has_unsaved_changes())
    }

    pub(crate) fn edit(&mut self, edit: DraftEdit, types: &MimeDatabase) {
        self.draft.apply(edit, types);
        self.leaving = None;
        if matches!(self.status, EditorStatus::Saved | EditorStatus::Failed(_)) {
            self.status = EditorStatus::Editing;
        }
    }

    /// 生成要写的文本并进入“保存中”。写出来的文本先用读动作文件的那套检查过一遍：
    /// 不过就不写，原因带行号（不该发生，是兜底）。
    pub(crate) fn begin_save(
        &mut self,
        folder: &Path,
        types: &MimeDatabase,
    ) -> Result<SaveRequest, String> {
        let mut document = self.document.clone();
        document::write_changes(&mut document, &self.written, &self.draft, types);
        let text = document.to_string();
        let (target, checked_as) = match &self.target {
            EditTarget::New => {
                let name = self.draft.name.trim().to_owned();
                let stem = saving::file_stem_for(&name);
                let checked_as = folder.join(format!("{stem}.toml"));
                (
                    SaveTarget::New {
                        folder: folder.to_path_buf(),
                        name,
                    },
                    checked_as,
                )
            }
            EditTarget::File { path, on_disk, .. } => (
                SaveTarget::Existing {
                    path: path.clone(),
                    expected: on_disk.clone(),
                },
                path.clone(),
            ),
        };
        let stem = checked_as
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        if let Err(mistake) =
            parse_action_file(ActionId::from_file_stem(stem), &checked_as, &text, types)
        {
            let reason = located(mistake);
            self.status = EditorStatus::Failed(reason.clone());
            return Err(reason);
        }
        self.status = EditorStatus::Saving {
            draft: Box::new(self.draft.clone()),
            document: Box::new(document),
        };
        Ok(SaveRequest { target, text })
    }

    pub(crate) fn finish_save(&mut self, outcome: SaveOutcome) {
        let EditorStatus::Saving { draft, document } =
            std::mem::replace(&mut self.status, EditorStatus::Editing)
        else {
            return;
        };
        self.status = match outcome {
            SaveOutcome::Saved { path, text } => {
                let id = match &self.target {
                    EditTarget::File { id, .. } => id.clone(),
                    EditTarget::New => ActionId::from_file_stem(
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .expect("new action files are named from text"),
                    ),
                };
                self.target = EditTarget::File {
                    path,
                    id,
                    on_disk: Some(text),
                };
                self.written = *draft;
                self.document = *document;
                EditorStatus::Saved
            }
            // 没写成：先让用户看到为什么，要离开的话等他再选。
            SaveOutcome::Changed { found } => {
                self.leaving = None;
                EditorStatus::Changed { found }
            }
            SaveOutcome::Failed(reason) => {
                self.leaving = None;
                EditorStatus::Failed(reason)
            }
        };
    }

    /// 问“放弃修改？”的时候用户点了保存，而且存好了：接着去原来要去的地方。
    pub(crate) fn saved_on_the_way_out(&self) -> Option<SettingsCategory> {
        self.leaving
            .filter(|_| matches!(self.status, EditorStatus::Saved) && !self.has_unsaved_changes())
    }

    /// “仍然保存”：把别处改过的内容当作打开时的内容，再保存一次就会盖掉它。
    pub(crate) fn accept_change_on_disk(&mut self) {
        if let (EditorStatus::Changed { found }, EditTarget::File { on_disk, .. }) =
            (&self.status, &mut self.target)
        {
            *on_disk = found.clone();
            self.status = EditorStatus::Editing;
        }
    }

    /// 已经有文件时的身份（快捷键按它记）。
    pub(crate) fn action_id(&self) -> Option<&ActionId> {
        match &self.target {
            EditTarget::File { id, .. } => Some(id),
            EditTarget::New => None,
        }
    }

    pub(crate) fn file(&self) -> Option<&Path> {
        match &self.target {
            EditTarget::File { path, .. } => Some(path),
            EditTarget::New => None,
        }
    }
}

fn located(mistake: super::action_file::FileMistake) -> String {
    match mistake.line {
        Some(line) => tr!("Line %1: %2", line, mistake.reason),
        None => mistake.reason,
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
