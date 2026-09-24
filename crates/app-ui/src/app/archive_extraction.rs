use std::fmt;

use file_core::{ArchiveExtractionRequest, ArchivePassword};
use iced::Task;

use super::archive_password::ArchivePasswordDraft;
use super::FileBrowser;
use crate::commands::inspect_archive_extraction_command;
use crate::model::Message;
use crate::operation_queue::QueuedFileOperation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveExtractionInspection {
    Ready,
    PasswordRequired,
    InvalidPassword,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) enum ArchiveExtractionMessage {
    PasswordChanged(ArchivePasswordDraft),
    Submitted,
    Inspected {
        request: ArchiveExtractionRequest,
        outcome: ArchiveExtractionInspection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArchiveExtractionPhase {
    Inspecting,
    WaitingForPassword,
    CheckingPassword,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ArchiveExtractionState {
    request: ArchiveExtractionRequest,
    password: ArchivePasswordDraft,
    validation_error: Option<String>,
    phase: ArchiveExtractionPhase,
}

impl ArchiveExtractionState {
    pub(super) fn inspecting(request: ArchiveExtractionRequest) -> Self {
        Self {
            request,
            password: ArchivePasswordDraft::new(String::new()),
            validation_error: None,
            phase: ArchiveExtractionPhase::Inspecting,
        }
    }

    pub(crate) fn request(&self) -> &ArchiveExtractionRequest {
        &self.request
    }

    pub(crate) fn password(&self) -> &ArchivePasswordDraft {
        &self.password
    }

    pub(crate) fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub(crate) fn is_waiting_for_password(&self) -> bool {
        self.phase == ArchiveExtractionPhase::WaitingForPassword
    }

    pub(crate) fn is_checking_password(&self) -> bool {
        self.phase == ArchiveExtractionPhase::CheckingPassword
    }

    pub(crate) fn is_inspecting(&self) -> bool {
        self.phase == ArchiveExtractionPhase::Inspecting
    }

    pub(crate) fn can_submit_password(&self) -> bool {
        !self.is_checking_password()
    }

    fn wait_for_password(&mut self, validation_error: Option<String>) {
        self.phase = ArchiveExtractionPhase::WaitingForPassword;
        self.validation_error = validation_error;
    }

    fn update_password(&mut self, password: ArchivePasswordDraft) {
        if self.is_waiting_for_password() {
            self.password = password;
            self.validation_error = None;
        }
    }

    fn archive_password(&self) -> Option<ArchivePassword> {
        self.password.to_archive_password()
    }

    fn check_password(&mut self, request: ArchiveExtractionRequest) {
        self.request = request;
        self.phase = ArchiveExtractionPhase::CheckingPassword;
        self.validation_error = None;
    }
}

impl fmt::Debug for ArchiveExtractionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArchiveExtractionState")
            .field("request", &self.request)
            .field("password", &self.password)
            .field("validation_error", &self.validation_error)
            .field("phase", &self.phase)
            .finish()
    }
}

impl FileBrowser {
    pub(super) fn handle_archive_extraction_message(
        &mut self,
        message: ArchiveExtractionMessage,
    ) -> Task<Message> {
        match message {
            ArchiveExtractionMessage::PasswordChanged(password) => {
                if let Some(state) = self.archive_extraction.as_mut() {
                    state.update_password(password);
                }
                Task::none()
            }
            ArchiveExtractionMessage::Submitted => self.submit_archive_extraction_password(),
            ArchiveExtractionMessage::Inspected { request, outcome } => {
                self.accept_archive_extraction_inspection(request, outcome)
            }
        }
    }

    fn submit_archive_extraction_password(&mut self) -> Task<Message> {
        let Some(state) = self.archive_extraction.as_mut() else {
            return Task::none();
        };
        if state.is_checking_password() {
            return Task::none();
        }
        let Some(password) = state.archive_password() else {
            state.wait_for_password(Some("Enter the archive password.".to_owned()));
            return Task::none();
        };

        let request = state.request.with_password(Some(password));
        state.check_password(request.clone());
        inspect_archive_extraction_command(request)
    }

    fn accept_archive_extraction_inspection(
        &mut self,
        request: ArchiveExtractionRequest,
        outcome: ArchiveExtractionInspection,
    ) -> Task<Message> {
        let Some(mut state) = self.archive_extraction.take() else {
            return Task::none();
        };
        if state.request != request {
            self.archive_extraction = Some(state);
            return Task::none();
        }

        match outcome {
            ArchiveExtractionInspection::Ready => {
                self.enqueue_file_operation(QueuedFileOperation::ExtractArchive { request })
            }
            ArchiveExtractionInspection::PasswordRequired => {
                state.request = request.without_password();
                state.wait_for_password(None);
                self.archive_extraction = Some(state);
                Task::none()
            }
            ArchiveExtractionInspection::InvalidPassword => {
                state.wait_for_password(Some("Incorrect password. Try again.".to_owned()));
                self.archive_extraction = Some(state);
                Task::none()
            }
            ArchiveExtractionInspection::Failed(error) => {
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    pub(super) fn clear_state_for_archive_extraction(&mut self) {
        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.destructive_action_confirmation = None;
        self.transfer_conflict = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        self.renaming = None;
        self.clear_global_error();
        // 成员密码弹窗与整包解压弹窗同为模态,二者互斥:任何一个打开时
        // 都要把另一个清掉。成员密码弹窗入口也复用本方法完成同一清理。
        self.archive_member_password = None;
    }
}
