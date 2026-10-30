use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use file_core::{
    validate_entry_name, ArchiveCompressionLevel, ArchiveFormat, ArchivePassword, FileKind,
    InvalidEntryName,
};
use iced::Task;

use super::archive_password::ArchivePasswordDraft;
use super::FileBrowser;
use crate::commands::check_archive_target_command;
use crate::model::Message;
use crate::operation_queue::QueuedFileOperation;

pub(crate) const ARCHIVE_FORMATS: [ArchiveFormat; 3] = [
    ArchiveFormat::Zip,
    ArchiveFormat::SevenZip,
    ArchiveFormat::TarGz,
];
pub(crate) const ARCHIVE_COMPRESSION_LEVELS: [ArchiveCompressionLevel; 4] = [
    ArchiveCompressionLevel::Store,
    ArchiveCompressionLevel::Fast,
    ArchiveCompressionLevel::Balanced,
    ArchiveCompressionLevel::Maximum,
];

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ArchiveCreationState {
    sources: Vec<PathBuf>,
    target_directory: PathBuf,
    file_name: String,
    format: ArchiveFormat,
    compression_level: ArchiveCompressionLevel,
    password: ArchivePasswordDraft,
    validation_error: Option<String>,
    checking_target: bool,
}

impl ArchiveCreationState {
    fn new(
        sources: Vec<PathBuf>,
        target_directory: PathBuf,
        single_source_kind: Option<FileKind>,
    ) -> Self {
        let file_name = default_archive_name(&sources, single_source_kind);
        Self {
            sources,
            target_directory,
            file_name,
            format: ArchiveFormat::Zip,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: ArchivePasswordDraft::new(String::new()),
            validation_error: None,
            checking_target: false,
        }
    }

    pub(crate) fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    pub(crate) fn target_directory(&self) -> &Path {
        &self.target_directory
    }

    pub(crate) fn file_name(&self) -> &str {
        &self.file_name
    }

    pub(crate) fn format(&self) -> ArchiveFormat {
        self.format
    }

    pub(crate) fn compression_level(&self) -> ArchiveCompressionLevel {
        self.compression_level
    }

    pub(crate) fn password(&self) -> &ArchivePasswordDraft {
        &self.password
    }

    pub(crate) fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub(crate) fn is_checking_target(&self) -> bool {
        self.checking_target
    }

    pub(crate) fn password_supported(&self) -> bool {
        self.format != ArchiveFormat::TarGz
    }

    pub(crate) fn can_submit(&self) -> bool {
        !self.checking_target && self.target_path().is_ok()
    }

    pub(crate) fn target_path(&self) -> Result<PathBuf, String> {
        let name = self.file_name.trim();
        validate_entry_name(OsStr::new(name)).map_err(archive_name_error)?;

        Ok(self
            .target_directory
            .join(format!("{}.{}", name, self.format.extension())))
    }

    fn update_name(&mut self, name: String) {
        self.file_name = name;
        self.validation_error = None;
        self.checking_target = false;
    }

    fn select_format(&mut self, format: ArchiveFormat) {
        self.format = format;
        if !self.password_supported() {
            self.password = ArchivePasswordDraft::new(String::new());
        }
        self.validation_error = None;
        self.checking_target = false;
    }

    fn select_compression_level(&mut self, compression_level: ArchiveCompressionLevel) {
        self.compression_level = compression_level;
        self.validation_error = None;
        self.checking_target = false;
    }

    fn update_password(&mut self, password: ArchivePasswordDraft) {
        if self.password_supported() {
            self.password = password;
            self.validation_error = None;
            self.checking_target = false;
        }
    }

    fn start_target_check(&mut self) {
        self.validation_error = None;
        self.checking_target = true;
    }

    fn stop_target_check_with_error(&mut self, error: String) {
        self.validation_error = Some(error);
        self.checking_target = false;
    }

    fn archive_password(&self) -> Option<ArchivePassword> {
        self.password_supported()
            .then(|| self.password.to_archive_password())
            .flatten()
    }
}

impl fmt::Debug for ArchiveCreationState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArchiveCreationState")
            .field("sources", &self.sources)
            .field("target_directory", &self.target_directory)
            .field("file_name", &self.file_name)
            .field("format", &self.format)
            .field("compression_level", &self.compression_level)
            .field("password", &self.password)
            .field("validation_error", &self.validation_error)
            .field("checking_target", &self.checking_target)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ArchiveCreationMessage {
    OpenSelected,
    NameChanged(String),
    FormatSelected(ArchiveFormat),
    CompressionLevelSelected(ArchiveCompressionLevel),
    PasswordChanged(ArchivePasswordDraft),
    Submitted,
    TargetChecked {
        state: ArchiveCreationState,
        target: PathBuf,
        available: Result<bool, String>,
    },
}

impl FileBrowser {
    pub(super) fn handle_archive_creation_message(
        &mut self,
        message: ArchiveCreationMessage,
    ) -> Task<Message> {
        match message {
            ArchiveCreationMessage::OpenSelected => self.open_archive_creation(),
            ArchiveCreationMessage::NameChanged(name) => {
                if let Some(state) = &mut self.archive_creation {
                    state.update_name(name);
                }
                Task::none()
            }
            ArchiveCreationMessage::FormatSelected(format) => {
                if let Some(state) = &mut self.archive_creation {
                    state.select_format(format);
                }
                Task::none()
            }
            ArchiveCreationMessage::CompressionLevelSelected(compression_level) => {
                if let Some(state) = &mut self.archive_creation {
                    state.select_compression_level(compression_level);
                }
                Task::none()
            }
            ArchiveCreationMessage::PasswordChanged(password) => {
                if let Some(state) = &mut self.archive_creation {
                    state.update_password(password);
                }
                Task::none()
            }
            ArchiveCreationMessage::Submitted => self.submit_archive_creation(),
            ArchiveCreationMessage::TargetChecked {
                state,
                target,
                available,
            } => self.accept_archive_creation_target_check(state, target, available),
        }
    }

    fn open_archive_creation(&mut self) -> Task<Message> {
        if self.tab().is_trash_view {
            return Task::none();
        }

        let sources = self.selected_paths_for_operation();
        if sources.is_empty() {
            return Task::none();
        }

        let target_directory = archive_target_directory(&sources, &self.tab().current_dir);
        let single_source_kind = match sources.as_slice() {
            [source] => self.entry_kind(source),
            _ => None,
        };
        self.context_menu = None;
        self.open_with = None;
        self.archive_extraction = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        self.archive_creation = Some(ArchiveCreationState::new(
            sources,
            target_directory,
            single_source_kind,
        ));
        Task::none()
    }

    fn submit_archive_creation(&mut self) -> Task<Message> {
        let Some(mut state) = self.archive_creation.take() else {
            return Task::none();
        };

        if state.is_checking_target() {
            self.archive_creation = Some(state);
            return Task::none();
        }

        let target = match state.target_path() {
            Ok(target) => target,
            Err(error) => {
                state.stop_target_check_with_error(error);
                self.archive_creation = Some(state);
                return Task::none();
            }
        };

        state.start_target_check();
        let issued_state = state.clone();
        self.archive_creation = Some(state);
        check_archive_target_command(issued_state, target)
    }

    fn accept_archive_creation_target_check(
        &mut self,
        mut state: ArchiveCreationState,
        target: PathBuf,
        available: Result<bool, String>,
    ) -> Task<Message> {
        if self.archive_creation.as_ref() != Some(&state) {
            return Task::none();
        }

        match available {
            Ok(true) => {
                let operation = QueuedFileOperation::CreateArchive {
                    sources: state.sources.clone(),
                    target,
                    format: state.format,
                    compression_level: state.compression_level,
                    password: state.archive_password(),
                };
                self.archive_creation = None;
                self.enqueue_file_operation(operation)
            }
            Ok(false) => {
                state.stop_target_check_with_error(tr!(
                    "That archive already exists. Choose another name."
                ));
                self.archive_creation = Some(state);
                Task::none()
            }
            Err(error) => {
                state
                    .stop_target_check_with_error(tr!("Could not check archive target: %1", error));
                self.archive_creation = Some(state);
                Task::none()
            }
        }
    }
}

pub(crate) fn archive_format_label(format: ArchiveFormat) -> String {
    match format {
        ArchiveFormat::Zip => "zip",
        ArchiveFormat::SevenZip => "7z",
        ArchiveFormat::TarGz => "tar.gz",
    }
    .to_owned()
}

pub(crate) fn archive_compression_level_label(
    compression_level: ArchiveCompressionLevel,
) -> String {
    match compression_level {
        ArchiveCompressionLevel::Store => tr!("No compression"),
        ArchiveCompressionLevel::Fast => tr!("Fast"),
        ArchiveCompressionLevel::Balanced => tr!("Balanced"),
        ArchiveCompressionLevel::Maximum => tr!("Maximum"),
    }
}

fn archive_target_directory(sources: &[PathBuf], current_dir: &Path) -> PathBuf {
    sources
        .first()
        .and_then(|source| source.parent())
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| current_dir.to_path_buf())
}

fn default_archive_name(sources: &[PathBuf], single_source_kind: Option<FileKind>) -> String {
    if let [source] = sources {
        if single_source_kind == Some(FileKind::File) {
            if let Some(stem) = source
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
            {
                return stem.to_owned();
            }
        }
        source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(default_archive_base_name)
    } else {
        default_archive_base_name()
    }
}

/// 多选或取不到名字时的默认名，跟界面语言（同副本、新建文件夹）。
fn default_archive_base_name() -> String {
    tr!("Archive")
}

/// 名字规则与重命名、新建一致（`validate_entry_name`）：Linux 上 `\` 是普通字符，
/// 不当作路径分隔符拒绝。校验的是用户输入的基名，所以 `.`、`..` 仍然被拒。
fn archive_name_error(reason: InvalidEntryName) -> String {
    match reason {
        InvalidEntryName::Empty => tr!("Enter an archive name."),
        InvalidEntryName::CurrentDirectory | InvalidEntryName::ParentDirectory => {
            tr!("\".\" and \"..\" cannot be used as names.")
        }
        InvalidEntryName::ContainsSlash => tr!("Archive name cannot contain path separators."),
        InvalidEntryName::ContainsNul => tr!("Names cannot contain NUL characters."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_named(name: &str) -> ArchiveCreationState {
        let mut state = ArchiveCreationState::new(
            vec![PathBuf::from("/docs/report.pdf")],
            PathBuf::from("/docs"),
            Some(FileKind::File),
        );
        state.update_name(name.to_owned());
        state
    }

    #[test]
    fn archive_names_follow_the_shared_entry_name_rules() {
        assert_eq!(
            state_named(r"Q1\Q2").target_path(),
            Ok(PathBuf::from(r"/docs/Q1\Q2.zip"))
        );
        for (name, error) in [
            ("  ", "Enter an archive name."),
            ("..", "\".\" and \"..\" cannot be used as names."),
            ("a/b", "Archive name cannot contain path separators."),
            ("a\0b", "Names cannot contain NUL characters."),
        ] {
            assert_eq!(
                state_named(name).target_path(),
                Err(error.to_owned()),
                "{name:?}"
            );
        }
    }

    #[test]
    fn single_file_default_archive_name_uses_file_stem() {
        let sources = [PathBuf::from("/home/yuanming/test/2291443ff6b5e0ea.png")];

        assert_eq!(
            default_archive_name(&sources, Some(FileKind::File)),
            "2291443ff6b5e0ea"
        );
    }

    #[test]
    fn single_directory_default_archive_name_keeps_directory_name() {
        let sources = [PathBuf::from("/home/yuanming/test/photos.v1")];

        assert_eq!(
            default_archive_name(&sources, Some(FileKind::Directory)),
            "photos.v1"
        );
    }
}
