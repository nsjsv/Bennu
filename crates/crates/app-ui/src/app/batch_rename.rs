use iced::Task;
use std::collections::HashSet;
use std::path::PathBuf;

use super::FileBrowser;
use crate::model::{
    same_parent, BatchRenameMessage, BatchRenameSource, BatchRenameSourceNameError,
    BatchRenameState, Message,
};
use crate::operation_queue::QueuedFileOperation;
use crate::view::batch_rename_preview_name_input_id;

impl FileBrowser {
    pub(super) fn handle_batch_rename_message(
        &mut self,
        message: BatchRenameMessage,
    ) -> Task<Message> {
        match message {
            BatchRenameMessage::OpenSelected => self.open_batch_rename_selected(),
            BatchRenameMessage::Apply => self.apply_batch_rename(),
            BatchRenameMessage::Cancel => {
                self.batch_rename = None;
                Task::none()
            }
            BatchRenameMessage::PreviewNameEditStarted(source) => {
                self.start_batch_rename_preview_name_edit(source)
            }
            message => self.update_batch_rename(|state| state.apply_update(message)),
        }
    }

    pub(super) fn batch_rename_available_for_selection(&self) -> bool {
        let paths = self.selected_paths_for_operation();
        !self.is_trash_view && paths.len() > 1 && same_parent(&paths)
    }

    fn open_batch_rename_selected(&mut self) -> Task<Message> {
        if !self.batch_rename_available_for_selection() {
            return Task::none();
        }
        let paths = self.selected_paths_for_operation();
        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.renaming = None;
        let _ = self.cancel_address_editing();
        let sources = match self.batch_rename_sources_for_paths(&paths) {
            Ok(sources) => sources,
            Err(error) => {
                self.batch_rename = None;
                self.show_global_error(error.to_string());
                return Task::none();
            }
        };
        let existing_paths = self.batch_rename_existing_paths(&paths);
        self.batch_rename = BatchRenameState::new_with_existing_sources(sources, existing_paths);
        Task::none()
    }

    fn update_batch_rename(&mut self, update: impl FnOnce(&mut BatchRenameState)) -> Task<Message> {
        if let Some(state) = &mut self.batch_rename {
            update(state);
            state.rebuild_preview();
        }
        Task::none()
    }

    pub(super) fn finish_batch_rename_preview_drag(&mut self) {
        if let Some(state) = &mut self.batch_rename {
            state.finish_preview_drag();
        }
    }

    fn apply_batch_rename(&mut self) -> Task<Message> {
        let Some(items) = self.batch_rename.as_ref().and_then(BatchRenameState::plan) else {
            return Task::none();
        };
        self.batch_rename = None;
        self.enqueue_file_operation(QueuedFileOperation::BatchRename { items })
    }

    fn start_batch_rename_preview_name_edit(&mut self, source: PathBuf) -> Task<Message> {
        let has_row = self
            .batch_rename
            .as_ref()
            .and_then(|state| state.preview_target_name_for_source(&source))
            .is_some();
        let update = self.update_batch_rename(|state| {
            state.apply_update(BatchRenameMessage::PreviewNameEditStarted(source.clone()))
        });
        if !has_row {
            return update;
        }

        let input_id = batch_rename_preview_name_input_id(&source);
        Task::batch([
            update,
            iced::widget::operation::focus(input_id.clone()),
            iced::widget::operation::select_all(input_id),
        ])
    }

    fn batch_rename_existing_paths(&self, selected_paths: &[PathBuf]) -> HashSet<PathBuf> {
        let Some(parent) = selected_paths.first().and_then(|path| path.parent()) else {
            return HashSet::new();
        };

        self.entry_paths_in_directory(parent).into_iter().collect()
    }

    fn batch_rename_sources_for_paths(
        &self,
        selected_paths: &[PathBuf],
    ) -> Result<Vec<BatchRenameSource>, BatchRenameSourceNameError> {
        selected_paths
            .iter()
            .filter_map(|path| self.entry_for_path(path))
            .map(BatchRenameSource::try_from_entry)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    use super::*;
    use crate::config;
    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    fn file_entry(path: PathBuf) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )
    }

    #[cfg(unix)]
    #[test]
    fn opening_non_utf8_selection_rejects_the_entire_batch() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let first_name = OsString::from_vec(b"entry-\x80".to_vec());
        let second_name = OsString::from_vec(b"entry-\x81".to_vec());
        let first_path = directory.path().join(&first_name);
        let second_path = directory.path().join(&second_name);
        fs::write(&first_path, b"first").expect("create first source");
        fs::write(&second_path, b"second").expect("create second source");
        assert_ne!(first_name.as_bytes(), second_name.as_bytes());
        assert_eq!(first_name.to_string_lossy(), second_name.to_string_lossy());
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        browser.entries = vec![
            file_entry(first_path.clone()),
            file_entry(second_path.clone()),
        ]
        .into();
        browser.selected_paths = HashSet::from([first_path.clone(), second_path.clone()]);

        drop(browser.handle_batch_rename_message(BatchRenameMessage::OpenSelected));

        assert!(browser.batch_rename.is_none());
        assert_eq!(browser.operation_queue.task_count(), 0);
        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("non-UTF-8")));
        assert_eq!(fs::read(first_path).unwrap(), b"first");
        assert_eq!(fs::read(second_path).unwrap(), b"second");
    }

    #[test]
    fn canceling_batch_rename_keeps_sources_and_queue_unchanged() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let first_path = directory.path().join("first.txt");
        let second_path = directory.path().join("second.txt");
        fs::write(&first_path, b"first").expect("create first source");
        fs::write(&second_path, b"second").expect("create second source");
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        browser.entries = vec![
            file_entry(first_path.clone()),
            file_entry(second_path.clone()),
        ]
        .into();
        browser.selected_paths = HashSet::from([first_path.clone(), second_path.clone()]);

        drop(browser.handle_batch_rename_message(BatchRenameMessage::OpenSelected));
        assert!(browser.batch_rename.is_some());
        drop(browser.handle_batch_rename_message(BatchRenameMessage::Cancel));

        assert!(browser.batch_rename.is_none());
        assert_eq!(browser.operation_queue.task_count(), 0);
        assert_eq!(fs::read(first_path).unwrap(), b"first");
        assert_eq!(fs::read(second_path).unwrap(), b"second");
    }
}
