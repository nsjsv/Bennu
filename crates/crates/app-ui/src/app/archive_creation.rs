use std::fmt;
use std::path::{Path, PathBuf};

use file_core::{ArchiveCompressionLevel, ArchiveFormat, ArchivePassword, FileKind};
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

const DEFAULT_ARCHIVE_NAME: &str = "Archive";

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
        if name.is_empty() {
            return Err("Enter an archive name.".to_owned());
        }
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err("Archive name cannot contain path separators.".to_owned());
        }

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
        if self.is_trash_view {
            return Task::none();
        }

        let sources = self.selected_paths_for_operation();
        if sources.is_empty() {
            return Task::none();
        }

        let target_directory = archive_target_directory(&sources, &self.current_dir);
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
                state.stop_target_check_with_error(
                    "That archive already exists. Choose another name.".to_owned(),
                );
                self.archive_creation = Some(state);
                Task::none()
            }
            Err(error) => {
                state.stop_target_check_with_error(format!(
                    "Could not check archive target: {error}"
                ));
                self.archive_creation = Some(state);
                Task::none()
            }
        }
    }
}

pub(crate) fn archive_format_label(format: ArchiveFormat) -> &'static str {
    match format {
        ArchiveFormat::Zip => "zip",
        ArchiveFormat::SevenZip => "7z",
        ArchiveFormat::TarGz => "tar.gz",
    }
}

pub(crate) fn archive_compression_level_label(
    compression_level: ArchiveCompressionLevel,
) -> &'static str {
    match compression_level {
        ArchiveCompressionLevel::Store => "No compression",
        ArchiveCompressionLevel::Fast => "Fast",
        ArchiveCompressionLevel::Balanced => "Balanced",
        ArchiveCompressionLevel::Maximum => "Maximum",
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
            .unwrap_or(DEFAULT_ARCHIVE_NAME)
            .to_owned()
    } else {
        DEFAULT_ARCHIVE_NAME.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
